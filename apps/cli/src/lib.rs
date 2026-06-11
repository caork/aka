//! Shared aka runtime used by the CLI binary and the Tauri desktop shell.

pub mod backend;
pub mod indexer;

use std::path::PathBuf;

use aka_core::{
    registry::now_unix, ArtifactDir, EngineEvent, EngineRunner, Registry, RepoEntry, RepoPaths,
};
use anyhow::{Context, Result};

pub use backend::AkaBackend;

/// Full analysis pipeline: engine parse -> graph/search index -> registry.
pub fn run_analyze(path: PathBuf, engine_dir: Option<PathBuf>, no_chunks: bool) -> Result<String> {
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
        repo.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
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
    // Re-analysis and background updates inherit display/source/settings fields.
    let prev = registry.find(repo).cloned();
    registry.upsert(RepoEntry {
        name: prev.as_ref().map(|e| e.name.clone()).unwrap_or_else(|| {
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
        render_max_nodes: prev.as_ref().and_then(|e| e.render_max_nodes),
    });
    registry.save()?;
    Ok(())
}

pub fn run_index(path: PathBuf) -> Result<()> {
    let repo = path.canonicalize()?;
    let paths = RepoPaths::for_repo(&repo);
    let artifact = ArtifactDir::open(paths.artifact_dir()).context("工件不存在——先 aka analyze")?;
    let idx = indexer::index_artifact(&artifact, &paths)?;
    register(&repo, &paths, &artifact)?;
    eprintln!(
        "aka ▸ 重建索引完成：{} 节点 / {} 边 / {} 切块",
        idx.nodes, idx.edges, idx.chunks
    );
    Ok(())
}
