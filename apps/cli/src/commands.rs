//! Shared CLI commands for the standalone `aka` binary and desktop executable.

use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::{run_analyze, run_index, AkaBackend};
use aka_core::{Registry, RepoPaths};
use aka_mcp::Backend;

#[derive(Parser)]
#[command(name = "aka", version, about = "aka - code omniscience engine")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Analyze a repository: engine parse -> artifacts -> graph/search indexes.
    Analyze {
        path: PathBuf,
        /// Engine directory. Defaults to $AKA_ENGINE_DIR or bundled/workspace engine.
        #[arg(long)]
        engine_dir: Option<PathBuf>,
        /// Skip embedding chunks.
        #[arg(long)]
        no_chunks: bool,
    },
    /// Rebuild indexes from existing artifacts without rerunning the engine.
    Index { path: PathBuf },
    /// List registered repositories.
    Repos,
    /// Full-text search.
    Search {
        query: String,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Source line search.
    SearchCode {
        query: String,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long, default_value_t = 1)]
        context: usize,
        #[arg(long)]
        regex: bool,
        #[arg(long)]
        path_filter: Option<String>,
    },
    /// Symbol context: definitions, callers, callees, and references.
    Context {
        symbol: String,
        #[arg(long)]
        repo: Option<String>,
    },
    /// Export LOD graph JSON.
    Lod {
        #[arg(long)]
        repo: String,
        #[arg(long, default_value_t = 50000)]
        max_nodes: usize,
        /// Output file. Defaults to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// MCP server over stdio.
    Mcp,
    /// HTTP server for REST desktop data source.
    Serve {
        #[arg(long, default_value = "127.0.0.1:4111")]
        addr: SocketAddr,
    },
}

pub fn run_from_env() -> Result<()> {
    run_from(std::env::args_os())
}

pub fn run_from<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = Cli::parse_from(args);
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
        Cmd::SearchCode {
            query,
            repo,
            limit,
            context,
            regex,
            path_filter,
        } => run_search_code(query, repo, limit, context, regex, path_filter),
        Cmd::Context { symbol, repo } => run_context(symbol, repo),
        Cmd::Lod {
            repo,
            max_nodes,
            out,
        } => run_lod(repo, max_nodes, out),
        Cmd::Mcp => tokio_rt()?.block_on(async {
            let backend = AkaBackend::new().with_workspace_auto_index();
            match backend.auto_index_current_workspace() {
                Ok(Some(name)) => {
                    eprintln!("aka ▸ MCP detected current workspace; indexing queued as {name}");
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("aka ▸ MCP workspace auto-index skipped: {err:#}");
                }
            }
            backend.start_auto_indexer();
            aka_mcp::serve_stdio(Arc::new(backend) as Arc<dyn Backend>).await
        }),
        Cmd::Serve { addr } => tokio_rt()?.block_on(async {
            eprintln!("aka -> http://{addr}");
            let backend = AkaBackend::new();
            backend.start_auto_indexer();
            aka_server::serve(Arc::new(backend) as Arc<dyn Backend>, addr).await
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
        eprintln!("empty; run `aka analyze <path>` first");
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
        eprintln!("no results");
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

fn run_search_code(
    query: String,
    repo: Option<String>,
    limit: usize,
    context: usize,
    regex: bool,
    path_filter: Option<String>,
) -> Result<()> {
    let backend = AkaBackend::new();
    let result = backend.search_code(
        repo.as_deref(),
        &query,
        limit,
        context.min(aka_mcp::ops::MAX_CODE_CONTEXT),
        regex,
        path_filter.as_deref(),
    )?;
    if result.hits.is_empty() {
        eprintln!("no results");
        return Ok(());
    }
    println!("-- directories");
    for d in &result.directories {
        println!("  {:5} {}", d.count, d.dir);
    }
    println!("-- matches");
    for h in result.hits {
        println!(
            "{:8.3}  {:10} {:24} {}:{}",
            h.score, h.label, h.name, h.file_path, h.start_line
        );
        for m in h.matches {
            let mark = if m.matched { "*" } else { " " };
            println!("        {mark} {:>5}: {}", m.line, m.text);
        }
    }
    Ok(())
}

fn run_context(symbol: String, repo: Option<String>) -> Result<()> {
    let backend = AkaBackend::new();
    let repo = repo.as_deref();

    let defs = backend.find_definition(repo, &symbol)?;
    println!("-- definitions ({})", defs.len());
    for d in &defs {
        println!(
            "  {:10} {:24} {}:{}",
            d.label, d.name, d.file_path, d.start_line
        );
    }
    let callers = backend.callers(repo, &symbol, 1)?;
    println!("-- callers ({})", callers.len());
    for c in &callers {
        println!("  {:24} {}:{}", c.name, c.file_path, c.start_line);
    }
    let callees = backend.callees(repo, &symbol, 1)?;
    println!("-- callees ({})", callees.len());
    for c in &callees {
        println!("  {:24} {}:{}", c.name, c.file_path, c.start_line);
    }
    let refs = backend.references(repo, &symbol, 20)?;
    println!("-- references ({})", refs.len());
    for r in &refs {
        println!(
            "  [{}] {:20} {}:{}",
            r.edge_type, r.name, r.file_path, r.start_line
        );
    }
    Ok(())
}

fn run_lod(repo: String, max_nodes: usize, out: Option<PathBuf>) -> Result<()> {
    let registry = Registry::load()?;
    let entry = registry
        .repos
        .iter()
        .find(|r| r.name == repo || r.repo_path.to_string_lossy() == repo)
        .with_context(|| format!("unregistered repository: {repo}"))?;
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
                "aka -> LOD exported {} ({} nodes / {} edge pairs)",
                p.display(),
                lod.nodes.len(),
                lod.edges.len() / 2
            );
        }
        None => println!("{json}"),
    }
    Ok(())
}
