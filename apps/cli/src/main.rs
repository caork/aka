//! `aka` CLI — 分析、检索、服务三类入口的薄壳。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use aka_core::{
    registry::now_unix, ArtifactDir, EngineEvent, EngineRunner, Registry, RepoEntry, RepoPaths,
};

#[derive(Parser)]
#[command(name = "aka", version, about = "aka — 感知所有代码的知识引擎")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 分析一个仓库：engine 解析 → NDJSON 工件 → 注册
    Analyze {
        /// 仓库路径
        path: PathBuf,
        /// engine 目录（默认 $AKA_ENGINE_DIR 或 workspace 的 engine/）
        #[arg(long)]
        engine_dir: Option<PathBuf>,
        /// 跳过 embedding 切块（更快；语义检索开启前用不到切块）
        #[arg(long)]
        no_chunks: bool,
    },
    /// 列出已注册仓库
    Repos,
    /// 检索（默认纯 BM25；语义混排需先手动开启 embedding）
    Search {
        query: String,
        /// 限定某个已注册仓库（默认全部）
        #[arg(long)]
        repo: Option<PathBuf>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Analyze {
            path,
            engine_dir,
            no_chunks,
        } => analyze(path, engine_dir, no_chunks),
        Cmd::Repos => repos(),
        Cmd::Search { query, repo, limit } => search(query, repo, limit),
    }
}

fn analyze(path: PathBuf, engine_dir: Option<PathBuf>, no_chunks: bool) -> Result<()> {
    let repo = path
        .canonicalize()
        .with_context(|| format!("仓库路径不存在: {}", path.display()))?;
    let paths = RepoPaths::for_repo(&repo);
    let artifact_dir = paths.artifact_dir();
    std::fs::create_dir_all(&artifact_dir)?;

    let runner = EngineRunner::discover(engine_dir.as_deref())?;
    eprintln!("aka ▸ engine 解析 {} …", repo.display());

    let mut last_phase = String::new();
    runner.analyze(&repo, &artifact_dir, no_chunks, |ev| match ev {
        EngineEvent::Phase {
            phase,
            current,
            total,
        } => {
            if *phase != last_phase {
                eprintln!("  · {phase}");
                last_phase = phase.clone();
            } else if *total > 0 {
                eprint!("\r  · {phase} {current}/{total}    ");
            }
        }
        EngineEvent::Warning { message } => eprintln!("  ! {message}"),
        EngineEvent::Done { stats } => {
            eprintln!(
                "\naka ▸ 解析完成：{} 文件 / {} 节点 / {} 边 / {} 切块",
                stats.files, stats.nodes, stats.edges, stats.chunks
            );
        }
    })?;

    // 完整性校验：manifest 存在 + 合同版本匹配。
    let artifact = ArtifactDir::open(&artifact_dir)?;

    let mut registry = Registry::load()?;
    registry.upsert(RepoEntry {
        name: repo
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".into()),
        repo_path: repo.clone(),
        data_dir: paths.root.clone(),
        indexed_at: Some(now_unix()),
        engine_sha: None,
        stats: artifact.manifest.stats.clone(),
        embeddings_enabled: false,
    });
    registry.save()?;

    eprintln!(
        "aka ▸ 工件就绪 {}（图/搜索索引接线随集成批次启用）",
        artifact_dir.display()
    );
    Ok(())
}

fn repos() -> Result<()> {
    let registry = Registry::load()?;
    if registry.repos.is_empty() {
        eprintln!("（空）先 `aka analyze <path>` 注册一个仓库");
        return Ok(());
    }
    for r in &registry.repos {
        println!(
            "{:24} {}  nodes={} edges={} chunks={}  embeddings={}",
            r.name,
            r.repo_path.display(),
            r.stats.nodes,
            r.stats.edges,
            r.stats.chunks,
            if r.embeddings_enabled { "on" } else { "off" }
        );
    }
    Ok(())
}

fn search(_query: String, _repo: Option<PathBuf>, _limit: usize) -> Result<()> {
    bail!("search 接线中——aka-search/aka-graph 并行开发中，集成批次启用");
}
