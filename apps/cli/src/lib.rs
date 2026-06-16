//! Shared aka runtime used by the CLI binary and the Tauri desktop shell.

pub mod backend;
pub mod commands;
pub mod indexer;
mod rename;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aka_core::{
    build_parse_cache_manifest, load_index_state, registry::now_unix, save_index_state,
    save_parse_cache_manifest, user_facing_path, ArtifactDir, EngineEvent, EngineRunner,
    IndexDelta, IndexState, Registry, RepoEntry, RepoPaths,
};
use anyhow::{Context, Result};

pub use backend::AkaBackend;

pub type AnalyzeProgress<'a> = dyn FnMut(&EngineEvent) + 'a;

fn open_artifact_after_emit(
    artifact_dir: &Path,
    expected_stats: &aka_core::ArtifactStats,
) -> Result<ArtifactDir> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_err = anyhow::anyhow!("artifact open failed");
    loop {
        match ArtifactDir::open(artifact_dir) {
            Ok(artifact) => {
                let stats = &artifact.manifest.stats;
                if stats.files == expected_stats.files
                    && stats.nodes == expected_stats.nodes
                    && stats.edges == expected_stats.edges
                    && stats.chunks == expected_stats.chunks
                {
                    return Ok(artifact);
                }
                last_err = anyhow::anyhow!(
                    "manifest stats do not match engine done event (manifest: files={} nodes={} edges={} chunks={}, done: files={} nodes={} edges={} chunks={})",
                    stats.files,
                    stats.nodes,
                    stats.edges,
                    stats.chunks,
                    expected_stats.files,
                    expected_stats.nodes,
                    expected_stats.edges,
                    expected_stats.chunks,
                );
            }
            Err(err) => {
                last_err = err.into();
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(last_err).with_context(|| {
        format!(
            "artifact not complete after engine emit: {}",
            artifact_dir.display()
        )
    })
}

/// Full analysis pipeline: engine parse -> graph/search index -> registry.
pub fn run_analyze(path: PathBuf, engine_dir: Option<PathBuf>, no_chunks: bool) -> Result<String> {
    run_analyze_with_progress(path, engine_dir, no_chunks, None)
}

pub fn run_analyze_with_progress(
    path: PathBuf,
    engine_dir: Option<PathBuf>,
    no_chunks: bool,
    mut progress: Option<&mut AnalyzeProgress<'_>>,
) -> Result<String> {
    let repo = path
        .canonicalize()
        .with_context(|| format!("仓库路径不存在: {}", path.display()))?;
    let repo = user_facing_path(&repo);
    let paths = RepoPaths::for_repo(&repo);
    let artifact_dir = paths.artifact_dir();
    let runner = EngineRunner::discover(engine_dir.as_deref())?;
    let engine_sha = std::fs::read_to_string(runner.dir().join("ENGINE_SHA"))
        .ok()
        .map(|s| s.trim().to_string());

    let current_state = IndexState::compute(&repo, engine_sha.clone(), no_chunks)
        .with_context(|| format!("compute file hashes for {}", repo.display()))?;
    let previous_state = load_index_state(&paths.index_state_path())
        .with_context(|| format!("load index state {}", paths.index_state_path().display()))?;
    let delta = current_state.delta_from(previous_state.as_ref());
    if can_reuse_existing_index(&paths, previous_state.as_ref(), &current_state)? {
        if let Some(cb) = progress.as_mut() {
            cb(&EngineEvent::Phase {
                phase: "Reusing unchanged index".into(),
                current: 1,
                total: 1,
            });
        }
        let artifact = ArtifactDir::open(&artifact_dir)?;
        save_parse_cache_snapshot(&paths, &artifact, &current_state, delta.clone())?;
        register(&repo, &paths, &artifact, engine_sha)?;
        return Ok(format!(
            "aka ▸ {} 未变化：复用现有索引（{} 节点 / {} 边 / {} 切块；delta {}）",
            repo.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default(),
            artifact.manifest.stats.nodes,
            artifact.manifest.stats.edges,
            artifact.manifest.stats.chunks,
            delta.summary(),
        ));
    }

    if artifact_dir.exists() {
        std::fs::remove_dir_all(&artifact_dir)
            .with_context(|| format!("clear stale artifact dir {}", artifact_dir.display()))?;
    }
    std::fs::create_dir_all(&artifact_dir)?;
    let engine_cache_dir = paths.engine_cache_dir();
    std::fs::create_dir_all(&engine_cache_dir)
        .with_context(|| format!("create engine cache dir {}", engine_cache_dir.display()))?;
    std::fs::create_dir_all(paths.parse_cache_dir())?;
    eprintln!(
        "aka ▸ engine 解析 {} …（文件 delta {}）",
        repo.display(),
        delta.summary()
    );

    let mut last_phase = String::new();
    let stats = runner.analyze(
        &repo,
        &artifact_dir,
        Some(&engine_cache_dir),
        no_chunks,
        |ev| match ev {
            EngineEvent::Phase { phase, .. } => {
                if *phase != last_phase {
                    eprintln!("  · {phase}");
                    last_phase = phase.clone();
                }
                if let Some(cb) = progress.as_deref_mut() {
                    cb(ev);
                }
            }
            EngineEvent::Warning { message } => {
                eprintln!("  ! {message}");
                if let Some(cb) = progress.as_deref_mut() {
                    cb(ev);
                }
            }
            EngineEvent::Done { stats } => {
                eprintln!(
                    "  ✓ 解析完成：{} 文件 / {} 节点 / {} 边 / {} 切块",
                    stats.files, stats.nodes, stats.edges, stats.chunks
                );
                if let Some(cb) = progress.as_deref_mut() {
                    cb(ev);
                }
            }
        },
    )?;

    if let Some(cb) = progress.as_mut() {
        cb(&EngineEvent::Phase {
            phase: "Building graph and search indexes".into(),
            current: 0,
            total: 0,
        });
    }
    let artifact = open_artifact_after_emit(&artifact_dir, &stats)?;
    eprintln!("aka ▸ 构建索引 …");
    let idx = match previous_state.as_ref() {
        Some(previous) if !delta.is_empty() => match indexer::index_artifact_incremental(
            &artifact,
            &paths,
            &delta,
            previous,
            &current_state,
        )? {
            indexer::IncrementalIndexOutcome::Applied(idx) => {
                eprintln!("  ✓ 增量替换完成");
                idx
            }
            indexer::IncrementalIndexOutcome::FullRebuildRequired(reason) => {
                eprintln!("  · 增量不可用，回退全量：{reason}");
                indexer::index_artifact(&artifact, &paths)?
            }
        },
        _ => indexer::index_artifact(&artifact, &paths)?,
    };
    save_parse_cache_snapshot(&paths, &artifact, &current_state, delta.clone())?;

    if let Some(cb) = progress.as_mut() {
        cb(&EngineEvent::Phase {
            phase: "Registering repository".into(),
            current: 0,
            total: 0,
        });
    }
    register(&repo, &paths, &artifact, engine_sha)?;
    save_index_state(&paths.index_state_path(), &current_state)
        .with_context(|| format!("save index state {}", paths.index_state_path().display()))?;

    let summary = format!(
        "aka ▸ {} 就绪：{} 节点 / {} 边（悬空跳过 {}）/ {} 切块{}；delta {}{}",
        repo.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        idx.nodes,
        idx.edges,
        idx.dangling_edges,
        idx.chunks,
        if idx.incremental {
            "保留/替换"
        } else {
            "入索引"
        },
        delta.summary(),
        if idx.bad_lines > 0 {
            format!("；坏行 {}", idx.bad_lines)
        } else {
            String::new()
        }
    );
    Ok(summary)
}

fn can_reuse_existing_index(
    paths: &RepoPaths,
    previous: Option<&IndexState>,
    current_state: &IndexState,
) -> Result<bool> {
    let Some(previous) = previous else {
        return Ok(false);
    };
    if !previous.is_reusable_for(current_state) {
        return Ok(false);
    }
    if !paths.graph_db().is_file() || !paths.search_dir().is_dir() {
        return Ok(false);
    }
    Ok(ArtifactDir::open(paths.artifact_dir()).is_ok())
}

fn save_parse_cache_snapshot(
    paths: &RepoPaths,
    artifact: &ArtifactDir,
    current_state: &IndexState,
    delta: IndexDelta,
) -> Result<()> {
    let manifest = build_parse_cache_manifest(artifact, current_state, delta)?;
    save_parse_cache_manifest(&paths.parse_cache_manifest_path(), &manifest).with_context(
        || {
            format!(
                "save parse-cache manifest {}",
                paths.parse_cache_manifest_path().display()
            )
        },
    )?;
    Ok(())
}

fn register(
    repo: &std::path::Path,
    paths: &RepoPaths,
    artifact: &ArtifactDir,
    engine_sha: Option<String>,
) -> Result<()> {
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
        engine_sha: engine_sha.or_else(|| prev.as_ref().and_then(|e| e.engine_sha.clone())),
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
    let repo = user_facing_path(&path.canonicalize()?);
    let paths = RepoPaths::for_repo(&repo);
    let artifact = ArtifactDir::open(paths.artifact_dir()).context("工件不存在——先 aka analyze")?;
    let idx = indexer::index_artifact(&artifact, &paths)?;
    register(&repo, &paths, &artifact, None)?;
    eprintln!(
        "aka ▸ 重建索引完成：{} 节点 / {} 边 / {} 切块",
        idx.nodes, idx.edges, idx.chunks
    );
    Ok(())
}
