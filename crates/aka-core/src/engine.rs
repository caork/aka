//! Engine sidecar 运行器 — spawn TS 解析引擎的 emit-cli，流式消费 stdout 进度事件。
//!
//! 合同见 docs/contracts/artifacts.md：
//! `<runner> --repo <path> --out <dir> [--no-chunks]`，stdout NDJSON 事件，
//! 退出码 0 = 工件完整。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::types::{ArtifactStats, EngineEvent};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("engine dir not found: {0} (set --engine-dir or AKA_ENGINE_DIR)")]
    EngineDirMissing(PathBuf),
    #[error("failed to spawn engine ({cmd}): {source}")]
    Spawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },
    #[error("engine exited with {code:?}; stderr tail:\n{stderr_tail}")]
    Failed {
        code: Option<i32>,
        stderr_tail: String,
    },
    #[error("engine io error: {0}")]
    Io(#[from] std::io::Error),
}

/// 引擎安装形态：开发期 = engine 源码目录（经 npx tsx 运行），
/// 发布期 = 编译后的单二进制 sidecar。
pub struct EngineRunner {
    engine_dir: PathBuf,
}

impl EngineRunner {
    /// `engine_dir` = aka 仓库的 engine/ 目录（含 gitnexus/ 与 gitnexus-shared/）。
    pub fn new(engine_dir: impl Into<PathBuf>) -> Result<Self, EngineError> {
        let engine_dir = engine_dir.into();
        let pkg = engine_dir.join("gitnexus");
        if !pkg.join("package.json").exists() {
            return Err(EngineError::EngineDirMissing(pkg));
        }
        Ok(Self { engine_dir })
    }

    pub fn dir(&self) -> &Path {
        &self.engine_dir
    }

    /// 从环境变量或默认位置发现 engine 目录。
    pub fn discover(explicit: Option<&Path>) -> Result<Self, EngineError> {
        if let Some(dir) = explicit {
            return Self::new(dir);
        }
        if let Ok(env_dir) = std::env::var("AKA_ENGINE_DIR") {
            return Self::new(PathBuf::from(env_dir));
        }
        // 开发期兜底：可执行文件所在 workspace 的 engine/
        let mut candidates: Vec<PathBuf> = vec![PathBuf::from("engine")];
        if let Ok(exe) = std::env::current_exe() {
            // target/debug/aka → ../../engine
            if let Some(ws) = exe.ancestors().nth(3) {
                candidates.push(ws.join("engine"));
            }
        }
        for c in &candidates {
            if c.join("gitnexus/package.json").exists() {
                return Self::new(c.clone());
            }
        }
        Err(EngineError::EngineDirMissing(
            candidates.last().cloned().unwrap_or_default(),
        ))
    }

    /// 运行分析，把进度事件回调给 `on_event`，成功返回 done 事件的统计。
    pub fn analyze(
        &self,
        repo: &Path,
        out_dir: &Path,
        no_chunks: bool,
        mut on_event: impl FnMut(&EngineEvent),
    ) -> Result<ArtifactStats, EngineError> {
        let pkg_dir = self.engine_dir.join("gitnexus");
        let mut cmd = Command::new("npx");
        cmd.arg("tsx")
            .arg("src/export/emit-cli.ts")
            .arg("--repo")
            .arg(repo)
            .arg("--out")
            .arg(out_dir)
            .current_dir(&pkg_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if no_chunks {
            cmd.arg("--no-chunks");
        }

        let mut child = cmd.spawn().map_err(|source| EngineError::Spawn {
            cmd: format!("npx tsx src/export/emit-cli.ts (cwd {})", pkg_dir.display()),
            source,
        })?;

        // stderr 在后台线程收尾部（失败时给人看）。
        let stderr = child.stderr.take().expect("piped stderr");
        let stderr_handle = std::thread::spawn(move || {
            let mut tail: Vec<String> = Vec::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                tail.push(line);
                if tail.len() > 40 {
                    tail.remove(0);
                }
            }
            tail.join("\n")
        });

        let stdout = child.stdout.take().expect("piped stdout");
        let mut done_stats: Option<ArtifactStats> = None;
        for line in BufReader::new(stdout).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            // 非事件行（引擎内部杂音）容忍跳过。
            if let Ok(ev) = serde_json::from_str::<EngineEvent>(&line) {
                if let EngineEvent::Done { stats } = &ev {
                    done_stats = Some(stats.clone());
                }
                on_event(&ev);
            }
        }

        let status = child.wait()?;
        let stderr_tail = stderr_handle.join().unwrap_or_default();
        if !status.success() {
            return Err(EngineError::Failed {
                code: status.code(),
                stderr_tail,
            });
        }
        Ok(done_stats.unwrap_or_default())
    }
}
