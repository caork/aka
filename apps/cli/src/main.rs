//! `aka` CLI — analyze / search / context / mcp / serve / lod 全量入口。

mod backend;
mod indexer;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use aka_core::{
    registry::now_unix, ArtifactDir, EngineEvent, EngineRunner, Registry, RepoEntry, RepoPaths,
};
use aka_mcp::Backend;
use backend::AkaBackend;

#[derive(Parser)]
#[command(name = "aka", version, about = "aka — 感知所有代码的知识引擎")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 分析仓库：engine 解析 → 工件 → 图/搜索索引 → 注册
    Analyze {
        path: PathBuf,
        /// engine 目录（默认 $AKA_ENGINE_DIR 或 workspace 的 engine/）
        #[arg(long)]
        engine_dir: Option<PathBuf>,
        /// 跳过 embedding 切块
        #[arg(long)]
        no_chunks: bool,
    },
    /// 从已有工件重建索引（不重跑 engine）
    Index { path: PathBuf },
    /// 列出已注册仓库
    Repos,
    /// 全文检索（BM25；embedding 开启后混排）
    Search {
        query: String,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// 符号 360°：定义 + callers + callees + 引用
    Context {
        symbol: String,
        #[arg(long)]
        repo: Option<String>,
    },
    /// 导出 LOD 图谱 JSON（给桌面端/调试）
    Lod {
        #[arg(long)]
        repo: String,
        #[arg(long, default_value_t = 50000)]
        max_nodes: usize,
        /// 输出文件（缺省打到 stdout）
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// MCP 服务（stdio，给 Claude Code / Cursor）
    Mcp,
    /// HTTP 服务（远程模式 / 桌面端数据源）
    Serve {
        #[arg(long, default_value = "127.0.0.1:4111")]
        addr: SocketAddr,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Analyze {
            path,
            engine_dir,
            no_chunks,
        } => {
            let summary = run_analyze(path, engine_dir, no_chunks)?;
            eprintln!("{summary}");
            Ok(())
        }
        Cmd::Index { path } => run_index(path),
        Cmd::Repos => run_repos(),
        Cmd::Search { query, repo, limit } => run_search(query, repo, limit),
        Cmd::Context { symbol, repo } => run_context(symbol, repo),
        Cmd::Lod {
            repo,
            max_nodes,
            out,
        } => run_lod(repo, max_nodes, out),
        Cmd::Mcp => tokio_rt()?.block_on(async {
            aka_mcp::serve_stdio(Arc::new(AkaBackend::new()) as Arc<dyn Backend>).await
        }),
        Cmd::Serve { addr } => tokio_rt()?.block_on(async {
            eprintln!("aka ▸ http://{addr}");
            aka_server::serve(Arc::new(AkaBackend::new()) as Arc<dyn Backend>, addr).await
        }),
    }
}

fn tokio_rt() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio runtime")
}

/// 完整分析管线（CLI 与 Backend::analyze 共用）。返回人类可读摘要。
pub fn run_analyze(
    path: PathBuf,
    engine_dir: Option<PathBuf>,
    no_chunks: bool,
) -> Result<String> {
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
        EngineEvent::Phase { phase, .. } => {
            if *phase != last_phase {
                eprintln!("  · {phase}");
                last_phase = phase.clone();
            }
        }
        EngineEvent::Warning { message } => eprintln!("  ! {message}"),
        EngineEvent::Done { stats } => {
            eprintln!(
                "  ✓ 解析完成：{} 文件 / {} 节点 / {} 边 / {} 切块",
                stats.files, stats.nodes, stats.edges, stats.chunks
            );
        }
    })?;

    let artifact = ArtifactDir::open(&artifact_dir)?;
    eprintln!("aka ▸ 构建索引 …");
    let idx = indexer::index_artifact(&artifact, &paths)?;

    register(&repo, &paths, &artifact)?;

    let summary = format!(
        "aka ▸ {} 就绪：{} 节点 / {} 边（悬空跳过 {}）/ {} 切块入索引{}",
        repo.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        idx.nodes,
        idx.edges,
        idx.dangling_edges,
        idx.chunks,
        if idx.bad_lines > 0 {
            format!("；坏行 {}", idx.bad_lines)
        } else {
            String::new()
        }
    );
    Ok(summary)
}

fn register(repo: &std::path::Path, paths: &RepoPaths, artifact: &ArtifactDir) -> Result<()> {
    let engine_sha = EngineRunner::discover(None)
        .ok()
        .and_then(|r| std::fs::read_to_string(r.dir().join("ENGINE_SHA")).ok())
        .map(|s| s.trim().to_string());

    let mut registry = Registry::load()?;
    // 同路径重新分析（后台 update 任务也走这里）：继承旧条目的
    // name / source_kind / source_url / embeddings_enabled，
    // 不能把 git/zip 来源覆写回 local、也不能丢用户设置。
    let prev = registry.find(repo).cloned();
    registry.upsert(RepoEntry {
        name: prev
            .as_ref()
            .map(|e| e.name.clone())
            .unwrap_or_else(|| {
                repo.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "repo".into())
            }),
        repo_path: repo.to_path_buf(),
        data_dir: paths.root.clone(),
        indexed_at: Some(now_unix()),
        engine_sha,
        stats: artifact.manifest.stats.clone(),
        embeddings_enabled: prev.as_ref().is_some_and(|e| e.embeddings_enabled),
        source_kind: prev
            .as_ref()
            .map(|e| e.source_kind.clone())
            .unwrap_or_else(|| "local".into()),
        source_url: prev.as_ref().and_then(|e| e.source_url.clone()),
    });
    registry.save()?;
    Ok(())
}

fn run_index(path: PathBuf) -> Result<()> {
    let repo = path.canonicalize()?;
    let paths = RepoPaths::for_repo(&repo);
    let artifact = ArtifactDir::open(paths.artifact_dir())
        .context("工件不存在——先 aka analyze")?;
    let idx = indexer::index_artifact(&artifact, &paths)?;
    register(&repo, &paths, &artifact)?;
    eprintln!(
        "aka ▸ 重建索引完成：{} 节点 / {} 边 / {} 切块",
        idx.nodes, idx.edges, idx.chunks
    );
    Ok(())
}

fn run_repos() -> Result<()> {
    let backend = AkaBackend::new();
    let repos = backend.list_repos()?;
    if repos.is_empty() {
        eprintln!("（空）先 `aka analyze <path>` 注册一个仓库");
        return Ok(());
    }
    for r in repos {
        println!(
            "{:24} {}  nodes={} edges={}  embeddings={}",
            r.name,
            r.path,
            r.nodes,
            r.edges,
            if r.embeddings_enabled { "on" } else { "off" }
        );
    }
    Ok(())
}

fn run_search(query: String, repo: Option<String>, limit: usize) -> Result<()> {
    let backend = AkaBackend::new();
    let hits = backend.search(repo.as_deref(), &query, limit)?;
    if hits.is_empty() {
        eprintln!("无结果");
        return Ok(());
    }
    for h in hits {
        println!(
            "{:8.3}  {:10} {:24} {}:{}",
            h.score, h.label, h.name, h.file_path, h.start_line
        );
        if let Some(snip) = h.snippet {
            let plain = snip.replace("<b>", "\x1b[1m").replace("</b>", "\x1b[0m");
            println!("          {plain}");
        }
    }
    Ok(())
}

fn run_context(symbol: String, repo: Option<String>) -> Result<()> {
    let backend = AkaBackend::new();
    let repo = repo.as_deref();

    let defs = backend.find_definition(repo, &symbol)?;
    println!("── 定义 ({})", defs.len());
    for d in &defs {
        println!("  {:10} {:24} {}:{}", d.label, d.name, d.file_path, d.start_line);
    }
    let callers = backend.callers(repo, &symbol, 1)?;
    println!("── callers ({})", callers.len());
    for c in &callers {
        println!("  {:24} {}:{}", c.name, c.file_path, c.start_line);
    }
    let callees = backend.callees(repo, &symbol, 1)?;
    println!("── callees ({})", callees.len());
    for c in &callees {
        println!("  {:24} {}:{}", c.name, c.file_path, c.start_line);
    }
    let refs = backend.references(repo, &symbol, 20)?;
    println!("── 引用 ({})", refs.len());
    for r in &refs {
        println!("  [{}] {:20} {}:{}", r.edge_type, r.name, r.file_path, r.start_line);
    }
    Ok(())
}

fn run_lod(repo: String, max_nodes: usize, out: Option<PathBuf>) -> Result<()> {
    let registry = Registry::load()?;
    let entry = registry
        .repos
        .iter()
        .find(|r| r.name == repo || r.repo_path.to_string_lossy() == repo)
        .with_context(|| format!("未注册的仓库: {repo}"))?;
    let paths = RepoPaths {
        root: entry.data_dir.clone(),
    };
    let store = aka_graph::GraphStore::open(&paths.graph_db())?;
    let lod = store.lod_snapshot(max_nodes)?;
    let json = serde_json::to_string(&lod)?;
    match out {
        Some(p) => {
            std::fs::write(&p, &json)?;
            eprintln!(
                "aka ▸ LOD 已导出 {}（{} 节点 / {} 边对）",
                p.display(),
                lod.nodes.len(),
                lod.edges.len() / 2
            );
        }
        None => println!("{json}"),
    }
    Ok(())
}
