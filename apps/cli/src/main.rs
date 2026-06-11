//! `aka` CLI — analyze / search / context / mcp / serve / lod 全量入口。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use aka_cli::{run_analyze, run_index, AkaBackend};
use aka_core::{Registry, RepoPaths};
use aka_mcp::Backend;

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
