//! Shared aka runtime used by the CLI binary and the Tauri desktop shell.

pub mod backend;
pub mod commands;
pub mod indexer;
mod rename;

use std::path::{Path, PathBuf};

use aka_core::{
    build_parse_cache_manifest_from_facts, load_index_state, load_parse_cache_manifest,
    registry::now_unix, save_index_state, save_parse_cache_manifest, user_facing_path,
    AnalyzeFactsOptions, EngineEvent, EngineRunner, FactSource, FactStats, IndexDelta, IndexState,
    IndexingDeadline, LspEnrichmentPolicy, PipelineProgress, PipelineStage, Registry, RepoEntry,
    RepoPaths,
};
use anyhow::{Context, Result};

pub use backend::AkaBackend;

pub type AnalyzeProgress<'a> = dyn FnMut(&EngineEvent) + 'a;

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
    let deadline = IndexingDeadline::from_env();
    let repo = path
        .canonicalize()
        .with_context(|| format!("仓库路径不存在: {}", path.display()))?;
    let repo = user_facing_path(&repo);
    let paths = RepoPaths::for_repo(&repo);
    let runner = EngineRunner::discover(engine_dir.as_deref())?;
    let engine_sha = std::fs::read_to_string(runner.dir().join("ENGINE_SHA"))
        .ok()
        .map(|s| s.trim().to_string());

    if let Some(cb) = progress.as_mut() {
        cb(&EngineEvent::Progress {
            progress: PipelineProgress::new(
                PipelineStage::Prepare,
                format!("Preparing index state (max {}s)", deadline.max_secs()),
            ),
        });
    }
    deadline.check("prepare")?;
    let current_state = IndexState::compute(&repo, engine_sha.clone(), no_chunks)
        .with_context(|| format!("compute file hashes for {}", repo.display()))?;
    let previous_state = load_index_state(&paths.index_state_path())
        .with_context(|| format!("load index state {}", paths.index_state_path().display()))?;
    let delta = current_state.delta_from(previous_state.as_ref());
    if let Some(stats) =
        reusable_existing_index_stats(&repo, &paths, previous_state.as_ref(), &current_state)?
    {
        if let Some(cb) = progress.as_mut() {
            cb(&EngineEvent::Progress {
                progress: PipelineProgress::new(PipelineStage::Done, "Reusing unchanged index")
                    .counts(1, 1)
                    .stats(&stats),
            });
        }
        register(&repo, &paths, &stats, engine_sha)?;
        return Ok(format!(
            "aka ▸ {} 未变化：复用现有索引（{} 节点 / {} 边 / {} 切块；delta {}）",
            repo.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default(),
            stats.nodes,
            stats.edges,
            stats.chunks,
            delta.summary(),
        ));
    }

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
    let facts = runner.analyze_facts(
        &repo,
        AnalyzeFactsOptions {
            cache_dir: Some(&engine_cache_dir),
            no_chunks,
            deadline: Some(deadline),
        },
        |ev| match ev {
            EngineEvent::Progress {
                progress: event_progress,
            } => {
                eprintln!(
                    "  · {}: {}",
                    event_progress.stage.as_str(),
                    event_progress.message
                );
                if let Some(cb) = progress.as_deref_mut() {
                    cb(ev);
                }
            }
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
            EngineEvent::Log { stream, line } => {
                // Surface engine progress plus adapter diagnostics (per-stage
                // done/timeout/skipped lines) so optional OSS analyzer passes
                // are visible in the headless path, not just in the desktop.
                if stream == "engine" || stream == "adapter" {
                    eprintln!("  · {line}");
                }
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
        cb(&EngineEvent::Progress {
            progress: PipelineProgress::new(
                PipelineStage::GraphNodes,
                "Building graph and search indexes",
            ),
        });
    }
    if let Some(cb) = progress.as_mut() {
        cb(&EngineEvent::Log {
            stream: "runtime".into(),
            line: format!("facts transport {}", facts.transport_name()),
        });
    }
    eprintln!("aka ▸ 构建索引 …");
    let mut index_progress = |ev: indexer::IndexProgressEvent| {
        eprintln!("  · index {}: {}", ev.stage, ev.message);
        if let Some(cb) = progress.as_deref_mut() {
            cb(&EngineEvent::Progress {
                progress: PipelineProgress::new(index_stage(ev.stage), ev.message.clone())
                    .counts(ev.current.unwrap_or(0), ev.total.unwrap_or(0)),
            });
            cb(&EngineEvent::Phase {
                phase: format!("index:{}", ev.stage),
                current: ev.current.unwrap_or(0),
                total: ev.total.unwrap_or(0),
            });
            cb(&EngineEvent::Log {
                stream: "index".into(),
                line: format!("{}: {}", ev.stage, ev.message),
            });
        }
    };
    let idx = match previous_state.as_ref() {
        Some(previous) if !delta.is_empty() => {
            match indexer::index_facts_incremental_with_progress(
                &facts,
                &paths,
                &delta,
                previous,
                &current_state,
                Some(&mut index_progress),
            )? {
                indexer::IncrementalIndexOutcome::Applied(idx) => {
                    eprintln!("  ✓ 增量替换完成");
                    idx
                }
                indexer::IncrementalIndexOutcome::FullRebuildRequired(reason) => {
                    eprintln!("  · 增量不可用，回退全量：{reason}");
                    indexer::index_facts_with_progress(&facts, &paths, Some(&mut index_progress))?
                }
            }
        }
        _ => indexer::index_facts_with_progress(&facts, &paths, Some(&mut index_progress))?,
    };
    if let Some(cb) = progress.as_mut() {
        cb(&EngineEvent::Progress {
            progress: PipelineProgress::new(
                PipelineStage::ParseCache,
                format!(
                    "save parse cache manifest {}",
                    paths.parse_cache_manifest_path().display()
                ),
            ),
        });
    }
    save_parse_cache_snapshot(&paths, &facts, &current_state, delta.clone())?;

    if let Some(cb) = progress.as_mut() {
        cb(&EngineEvent::Progress {
            progress: PipelineProgress::new(PipelineStage::Register, "Registering repository"),
        });
    }
    register(&repo, &paths, facts.stats(), engine_sha)?;
    if let Some(cb) = progress.as_mut() {
        cb(&EngineEvent::Log {
            stream: "runtime".into(),
            line: format!("save index state {}", paths.index_state_path().display()),
        });
    }
    save_index_state(&paths.index_state_path(), &current_state)
        .with_context(|| format!("save index state {}", paths.index_state_path().display()))?;

    let lsp_policy = aka_core::AkaSettings::load()
        .map(LspEnrichmentPolicy::from_settings)
        .unwrap_or_default();
    let mut lsp_progress = |ev: &EngineEvent| {
        if let Some(cb) = progress.as_deref_mut() {
            cb(ev);
        }
    };
    let lsp_outcome = aka_core::run_optional_lsp_enrichment(&repo, lsp_policy, &mut lsp_progress);
    if let Some(cb) = progress.as_mut() {
        cb(&EngineEvent::Log {
            stream: "runtime".into(),
            line: format!("lsp enrichment outcome {lsp_outcome:?}"),
        });
    }

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

fn index_stage(stage: &str) -> PipelineStage {
    match stage {
        "graph:nodes" => PipelineStage::GraphNodes,
        "graph:edges" => PipelineStage::GraphEdges,
        "graph:layout" => PipelineStage::GraphLayout,
        "search:nodes" => PipelineStage::SearchNodes,
        "search:chunks" => PipelineStage::SearchChunks,
        "search:commit" => PipelineStage::SearchCommit,
        "lsp-enrichment" => PipelineStage::LspEnrichment,
        _ if stage.starts_with("incremental:graph") => PipelineStage::GraphNodes,
        _ if stage.starts_with("incremental:layout") => PipelineStage::GraphLayout,
        _ if stage.starts_with("incremental:search") => PipelineStage::SearchNodes,
        _ if stage.starts_with("incremental:commit") => PipelineStage::SearchCommit,
        _ => PipelineStage::GraphNodes,
    }
}

fn reusable_existing_index_stats(
    repo: &Path,
    paths: &RepoPaths,
    previous: Option<&IndexState>,
    current_state: &IndexState,
) -> Result<Option<FactStats>> {
    let Some(previous) = previous else {
        return Ok(None);
    };
    if !previous.is_reusable_for(current_state) {
        return Ok(None);
    }
    if !paths.graph_db().is_file() || !paths.search_dir().is_dir() {
        return Ok(None);
    }
    if let Some(manifest) = load_parse_cache_manifest(&paths.parse_cache_manifest_path())
        .with_context(|| {
            format!(
                "load parse-cache manifest {}",
                paths.parse_cache_manifest_path().display()
            )
        })?
    {
        if manifest.contract_version == current_state.contract_version
            && manifest.engine_sha == current_state.engine_sha
            && manifest.no_chunks == current_state.no_chunks
        {
            return Ok(Some(manifest.totals));
        }
    }
    if let Some(entry) = Registry::load()?
        .find(repo)
        .filter(|entry| entry.indexed_at.is_some())
    {
        return Ok(Some(entry.stats.clone()));
    }
    Ok(None)
}

fn save_parse_cache_snapshot(
    paths: &RepoPaths,
    source: &impl FactSource,
    current_state: &IndexState,
    delta: IndexDelta,
) -> Result<()> {
    let manifest = build_parse_cache_manifest_from_facts(source, current_state, delta)?;
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
    stats: &FactStats,
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
        stats: stats.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use aka_core::{save_index_state, ParseCacheManifest};

    #[test]
    fn reusable_index_reads_parse_cache_stats_from_parse_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "fn main() {}\n").unwrap();
        let paths = RepoPaths {
            root: tmp.path().join("aka-data"),
        };
        std::fs::create_dir_all(paths.search_dir()).unwrap();
        std::fs::write(paths.graph_db(), "").unwrap();

        let current = IndexState::compute(&repo, Some("engine-sha".into()), false).unwrap();
        save_index_state(&paths.index_state_path(), &current).unwrap();
        let previous = load_index_state(&paths.index_state_path()).unwrap();
        let expected = FactStats {
            files: 1,
            nodes: 3,
            edges: 2,
            chunks: 1,
        };
        save_parse_cache_manifest(
            &paths.parse_cache_manifest_path(),
            &ParseCacheManifest {
                version: 1,
                contract_version: current.contract_version,
                engine_sha: current.engine_sha.clone(),
                no_chunks: current.no_chunks,
                totals: expected.clone(),
                last_delta: current.delta_from(None),
                files: Default::default(),
            },
        )
        .unwrap();

        let stats =
            reusable_existing_index_stats(&repo, &paths, previous.as_ref(), &current).unwrap();

        assert_eq!(stats, Some(expected));
    }
}
