//! Engine runner backed by AKA engine.
//!
//! Engine runner backed by AKA engine.
//!
//! This file still contains the legacy binary + SQLite -> artifact adapter.
//! The primary indexing seam is now `aka_facts::FactSource`; `ArtifactDir`
//! adapts the legacy transport into that seam while the embedded/direct engine
//! API is built out.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use chrono::Utc;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};
use serde_json::{json, Map, Value};

use aka_facts::FactBatch;

use crate::artifact::ArtifactDir;
use crate::types::{
    ArtifactStats, ChunkRec, EdgeRec, EngineEvent, Manifest, NodeRec, CONTRACT_VERSION,
};
use crate::user_facing_path;

mod build_config_scan;
mod cache_synth;
mod command_synth;
mod config_synth;
mod dependency_synth;
mod event_synth;
mod fact_producer;
mod graphql_synth;
mod job_synth;
mod migration_synth;
mod persistence_access_synth;
mod persistence_java_synth;
mod persistence_model_synth;
mod persistence_mybatis_synth;
mod persistence_pymongo_synth;
mod persistence_synth;
mod policy_synth;
mod property_synth;
mod resource_synth;
mod route_annotated_synth;
mod route_consumer_synth;
mod route_django_synth;
mod route_python_prefix_synth;
mod route_realtime_synth;
mod route_shape;
mod route_shape_drf;
mod route_shape_java;
mod route_shape_java_builder;
mod route_shape_python;
mod route_shape_python_calls;
mod route_spring_functional_synth;
mod source_scan;
mod source_symbol_synth;
mod tool_synth;
mod topic_synth;
mod transaction_synth;
use cache_synth::{synthesize_caches_from_sources, SynthCache};
use command_synth::{
    command_entry_hints_from_sources, synthesize_commands_from_sources, CommandEntryHint,
    SynthCommand,
};
use config_synth::{synthesize_configs_from_sources, SynthConfig};
use dependency_synth::{
    synthesize_dependency_edges_from_sources, DependencyProgress, DependencyProgressPhase,
};
use event_synth::{synthesize_events_from_sources, SynthEvent};
use fact_producer::{EngineFactProducer, ProducedEngineFacts, SidecarEngineFactProducer};
use graphql_synth::{synthesize_graphql_from_sources, SynthGraphqlOperation};
use job_synth::{job_entry_hints_from_sources, synthesize_jobs_from_sources, SynthJob};
use persistence_synth::{synthesize_persistence_from_sources, SynthPersistenceGraph};
use policy_synth::{synthesize_policies_from_sources, SynthPolicy};
#[cfg(test)]
use property_synth::extract_python_class_properties;
use property_synth::{synthesize_python_properties, SynthProperty};
use resource_synth::{synthesize_resources_from_sources, SynthResource};
use route_annotated_synth::{
    dedup_route_candidates, extract_annotated_routes, java_interface_routes_by_method,
};
use route_consumer_synth::attach_route_consumers_with_progress;
use route_django_synth::django_urlconf_routes_from_repo;
use route_python_prefix_synth::{python_router_prefixes_by_file, PythonRoutePrefixes};
use route_realtime_synth::realtime_routes_by_file;
use route_shape::{
    extract_error_keys, extract_middleware, extract_response_keys_for_file, literal_occurrences,
};
use route_spring_functional_synth::spring_functional_routes_from_repo;
use source_scan::{
    find_call_args, find_matching_paren, is_business_language, is_ident_continue,
    is_noisy_source_path, is_project_code_source_path, node_at_offset, nodes_by_file,
    pick_handler_node, project_code_nodes_by_file, read_repo_text, skip_ws,
    source_annotations_before_node, split_top_level_commas, stable_hash, ProjectSourceSet,
};
use source_symbol_synth::{synthesize_source_symbols_from_sources, SynthSourceSymbol};
use tool_synth::{synthesize_tools_from_sources, SynthTool};
use topic_synth::{
    merge_native_channel_topics, synthesize_topics_from_sources, NativeTopicDetection, SynthTopic,
    TopicEndpointKind,
};
use transaction_synth::{synthesize_transactions_from_sources, SynthTransaction};

const DEFAULT_ENGINE_MODE: &str = "fast";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ENGINE_SILENCE_LOG_INTERVAL: Duration = Duration::from_secs(10);
const PROCESS_MAX_STARTS: usize = 200;
const PROCESS_MIN_COUNT: usize = 20;
const PROCESS_MAX_COUNT: usize = 300;
const PROCESS_MAX_STEPS: usize = 10;
const PROCESS_BRANCH_LIMIT: usize = 4;
const PROCESS_MIN_STEPS: usize = 3;
const MIN_SYNTH_COMMUNITY_SIZE: usize = 2;
const MIN_TRACE_CONFIDENCE: f64 = 0.5;
const COMMUNITY_LABEL_PROPAGATION_PASSES: usize = 4;
const DEFAULT_SYNTH_STAGE_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(windows)]
fn hide_child_console(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_child_console(_cmd: &mut Command) {}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("AKA engine not found: {0} (set --engine-dir, AKA_ENGINE_DIR, or AKA_ENGINE_BIN)")]
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
    #[error("engine sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("engine json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("engine facts error: {0}")]
    Facts(#[from] aka_facts::FactSourceError),
    #[error("engine produced no project row in {0}")]
    MissingProject(PathBuf),
}

/// Native AKA engine runner.
pub struct EngineRunner {
    engine_dir: PathBuf,
    engine_bin: PathBuf,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AnalyzeFactsOptions<'a> {
    pub cache_dir: Option<&'a Path>,
    pub debug_artifact_dir: Option<&'a Path>,
    pub no_chunks: bool,
}

/// Replayable facts emitted by the engine.
///
/// The indexing hot path consumes direct engine facts. The legacy artifact
/// variant is kept for compatibility/debug callers that still need files on disk.
pub enum EngineFacts {
    DirectBatch(FactBatch),
    LegacyArtifact(ArtifactDir),
}

impl EngineFacts {
    pub fn stats(&self) -> &ArtifactStats {
        match self {
            Self::DirectBatch(batch) => &batch.stats,
            Self::LegacyArtifact(artifact) => &artifact.manifest.stats,
        }
    }

    pub fn transport_name(&self) -> &'static str {
        match self {
            Self::DirectBatch(_) => "engine-direct-facts",
            Self::LegacyArtifact(_) => "legacy-artifact",
        }
    }
}

impl aka_facts::FactSource for EngineFacts {
    fn stats(&self) -> &aka_facts::FactStats {
        self.stats()
    }

    fn nodes(
        &self,
    ) -> Result<
        Box<dyn Iterator<Item = aka_facts::FactItem<NodeRec>> + '_>,
        aka_facts::FactSourceError,
    > {
        match self {
            Self::DirectBatch(batch) => aka_facts::FactSource::nodes(batch),
            Self::LegacyArtifact(artifact) => aka_facts::FactSource::nodes(artifact),
        }
    }

    fn edges(
        &self,
    ) -> Result<
        Box<dyn Iterator<Item = aka_facts::FactItem<EdgeRec>> + '_>,
        aka_facts::FactSourceError,
    > {
        match self {
            Self::DirectBatch(batch) => aka_facts::FactSource::edges(batch),
            Self::LegacyArtifact(artifact) => aka_facts::FactSource::edges(artifact),
        }
    }

    fn chunks(
        &self,
    ) -> Result<
        Option<Box<dyn Iterator<Item = aka_facts::FactItem<ChunkRec>> + '_>>,
        aka_facts::FactSourceError,
    > {
        match self {
            Self::DirectBatch(batch) => aka_facts::FactSource::chunks(batch),
            Self::LegacyArtifact(artifact) => aka_facts::FactSource::chunks(artifact),
        }
    }
}

enum EngineLine {
    Stdout(String),
    Stderr(String),
}

impl EngineRunner {
    const DONE_EXIT_GRACE: Duration = Duration::from_secs(5);

    /// `engine_dir` may be a directory containing `aka-engine`, an AKA engine
    /// source checkout with `build/c/aka-engine`, or the binary path.
    pub fn new(engine_dir: impl Into<PathBuf>) -> Result<Self, EngineError> {
        let requested = engine_dir.into();
        let engine_bin = resolve_engine_binary(&requested)
            .ok_or_else(|| EngineError::EngineDirMissing(requested.clone()))?;
        let engine_dir = if requested.is_file() {
            requested
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            requested
        };
        Ok(Self {
            engine_dir,
            engine_bin,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.engine_dir
    }

    /// Discover the native AKA engine from explicit path, env, local engine/,
    /// source checkout, or PATH.
    pub fn discover(explicit: Option<&Path>) -> Result<Self, EngineError> {
        if let Some(dir) = explicit {
            return Self::new(dir);
        }
        if let Ok(bin) = std::env::var("AKA_ENGINE_BIN") {
            return Self::new(PathBuf::from(bin));
        }
        if let Ok(env_dir) = std::env::var("AKA_ENGINE_DIR") {
            return Self::new(PathBuf::from(env_dir));
        }

        let mut candidates: Vec<PathBuf> = vec![
            PathBuf::from("engine"),
            PathBuf::from("/tmp/aka-engine-src"),
        ];
        if let Ok(cwd) = std::env::current_dir() {
            candidates.extend(cwd.ancestors().map(|p| p.join("engine")));
        }
        if let Ok(exe) = std::env::current_exe() {
            candidates.extend(exe.ancestors().skip(1).map(|p| p.join("engine")));
        }
        for c in &candidates {
            if resolve_engine_binary(c).is_some() {
                return Self::new(c.clone());
            }
        }

        for name in engine_exe_names() {
            if let Some(path_bin) = find_in_path(name) {
                return Self::new(path_bin);
            }
        }

        Err(EngineError::EngineDirMissing(
            candidates.last().cloned().unwrap_or_default(),
        ))
    }

    /// Run AKA engine, convert its SQLite graph into aka artifacts, and
    /// stream progress events to callers.
    pub fn analyze(
        &self,
        repo: &Path,
        out_dir: &Path,
        cache_dir: Option<&Path>,
        no_chunks: bool,
        mut on_event: impl FnMut(&EngineEvent),
    ) -> Result<ArtifactStats, EngineError> {
        std::fs::create_dir_all(out_dir)?;
        let (cache_root, engine_repo) =
            self.run_engine_index(repo, Some(out_dir), cache_dir, None, &mut on_event)?;

        emit_phase(&mut on_event, "aka-engine:export-artifacts", 0, 0);
        let (project, db_path) = find_single_project_db(&cache_root, &engine_repo)?;
        on_event(&EngineEvent::Log {
            stream: "adapter".into(),
            line: format!(
                "using engine db project={project} path={}",
                db_path.display()
            ),
        });
        let stats = export_artifacts(
            &engine_repo,
            out_dir,
            &db_path,
            &project,
            no_chunks,
            &mut on_event,
        )?;
        let done = EngineEvent::Done {
            stats: stats.clone(),
        };
        on_event(&done);
        Ok(stats)
    }

    fn run_engine_index(
        &self,
        repo: &Path,
        fallback_out_dir: Option<&Path>,
        cache_dir: Option<&Path>,
        facts_output_path: Option<&Path>,
        on_event: &mut impl FnMut(&EngineEvent),
    ) -> Result<(PathBuf, PathBuf), EngineError> {
        let cache_root = engine_cache_root(repo, fallback_out_dir, cache_dir);
        std::fs::create_dir_all(&cache_root)?;

        emit_phase(on_event, "aka-engine:index", 0, 0);
        let mode = engine_mode();
        let engine_repo = user_facing_path(repo);
        let engine_repo = engine_repo
            .canonicalize()
            .map(|path| user_facing_path(&path))
            .unwrap_or(engine_repo);
        let mut args = json!({
            "repo_path": engine_repo.display().to_string(),
            "mode": mode,
            "persistence": false,
        });
        if let Some(path) = facts_output_path {
            args["facts_output_path"] = Value::String(path.display().to_string());
        }
        let args = args.to_string();

        let mut cmd = Command::new(&self.engine_bin);
        cmd.arg("cli")
            .arg("--progress")
            .arg("--json")
            .arg("index_repository")
            .arg(&args)
            .env("AKA_ENGINE_CACHE_DIR", &cache_root)
            .env("CBM_CACHE_DIR", &cache_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_child_console(&mut cmd);
        let cmd_display = format!(
            "{} cli --progress --json index_repository <args>",
            self.engine_bin.display()
        );
        on_event(&EngineEvent::Log {
            stream: "engine".into(),
            line: format!(
                "spawn repo_path={} cache_dir={} mode={} bin={}",
                engine_repo.display(),
                cache_root.display(),
                mode,
                self.engine_bin.display()
            ),
        });
        let mut child = cmd.spawn().map_err(|source| EngineError::Spawn {
            cmd: cmd_display,
            source,
        })?;

        let (line_tx, line_rx) = mpsc::channel();
        let stderr = child.stderr.take().expect("piped stderr");
        let stderr_tx = line_tx.clone();
        let stderr_handle = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = stderr_tx.send(EngineLine::Stderr(line));
            }
        });

        let stdout = child.stdout.take().expect("piped stdout");
        let stdout_tx = line_tx.clone();
        let stdout_handle = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = stdout_tx.send(EngineLine::Stdout(line));
            }
        });
        drop(line_tx);

        let mut stdout_tail: Vec<String> = Vec::new();
        let mut stderr_tail: Vec<String> = Vec::new();
        let child_id = child.id();
        let started = Instant::now();
        let mut last_line_at = started;
        let mut last_silence_log_at = started;
        let status = loop {
            match line_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(EngineLine::Stdout(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    last_line_at = Instant::now();
                    push_tail(&mut stdout_tail, line.clone(), 80);
                    on_event(&EngineEvent::Log {
                        stream: "stdout".into(),
                        line: line.clone(),
                    });
                    if line.contains("\"status\":\"error\"") || line.contains("Pipeline failed") {
                        let _ = child.kill();
                        break child.wait()?;
                    }
                }
                Ok(EngineLine::Stderr(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    last_line_at = Instant::now();
                    if let Some(phase) = parse_engine_progress_phase(&line) {
                        emit_phase(on_event, phase, 0, 0);
                    }
                    push_tail(&mut stderr_tail, line.clone(), 120);
                    on_event(&EngineEvent::Log {
                        stream: "stderr".into(),
                        line,
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if now.duration_since(last_line_at) >= ENGINE_SILENCE_LOG_INTERVAL
                        && now.duration_since(last_silence_log_at) >= ENGINE_SILENCE_LOG_INTERVAL
                    {
                        last_silence_log_at = now;
                        on_event(&EngineEvent::Log {
                            stream: "engine".into(),
                            line: format!(
                                "waiting for engine output pid={} silent_for_ms={} elapsed_ms={}",
                                child_id,
                                now.duration_since(last_line_at).as_millis(),
                                now.duration_since(started).as_millis()
                            ),
                        });
                    }
                    if let Some(done) = child.try_wait()? {
                        break done;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break wait_for_done_exit(&mut child, Self::DONE_EXIT_GRACE)?;
                }
            }
        };
        for line in line_rx.try_iter() {
            match line {
                EngineLine::Stdout(line) if !line.trim().is_empty() => {
                    push_tail(&mut stdout_tail, line.clone(), 80);
                    on_event(&EngineEvent::Log {
                        stream: "stdout".into(),
                        line,
                    });
                }
                EngineLine::Stderr(line) if !line.trim().is_empty() => {
                    if let Some(phase) = parse_engine_progress_phase(&line) {
                        emit_phase(on_event, phase, 0, 0);
                    }
                    push_tail(&mut stderr_tail, line.clone(), 120);
                    on_event(&EngineEvent::Log {
                        stream: "stderr".into(),
                        line,
                    });
                }
                _ => {}
            }
        }
        let _ = stdout_handle.join();
        let _ = stderr_handle.join();
        if !status.success() {
            return Err(EngineError::Failed {
                code: status.code(),
                stderr_tail: engine_failure_context(
                    &engine_repo,
                    &cache_root,
                    &self.engine_bin,
                    &mode,
                    &stdout_tail,
                    &stderr_tail,
                ),
            });
        }

        Ok((cache_root, engine_repo))
    }

    /// Run the engine and return replayable facts for graph/search indexing.
    ///
    /// This is the runtime-facing API for the fused pipeline. It bypasses
    /// NDJSON artifact export and returns a replayable fact source directly to
    /// callers.
    pub fn analyze_facts(
        &self,
        repo: &Path,
        options: AnalyzeFactsOptions<'_>,
        mut on_event: impl FnMut(&EngineEvent),
    ) -> Result<EngineFacts, EngineError> {
        let cache_root = engine_cache_root(repo, options.debug_artifact_dir, options.cache_dir);
        std::fs::create_dir_all(&cache_root)?;
        let producer = SidecarEngineFactProducer;
        let request = producer.prepare(&cache_root)?;
        let (cache_root, engine_repo) = self.run_engine_index(
            repo,
            options.debug_artifact_dir,
            options.cache_dir,
            request.facts_output_path.as_deref(),
            &mut on_event,
        )?;
        let produced =
            producer.finish(&cache_root, &engine_repo, options.no_chunks, &mut on_event)?;
        let batch = match produced {
            ProducedEngineFacts::DirectBatch(batch) => batch,
            ProducedEngineFacts::EngineDbFallback { project, db_path } => collect_engine_facts(
                &engine_repo,
                &db_path,
                &project,
                options.no_chunks,
                &mut on_event,
            )?,
        };
        let done = EngineEvent::Done {
            stats: batch.stats.clone(),
        };
        on_event(&done);
        Ok(EngineFacts::DirectBatch(batch))
    }
}

fn engine_cache_root(
    repo: &Path,
    fallback_out_dir: Option<&Path>,
    cache_dir: Option<&Path>,
) -> PathBuf {
    cache_dir.map(|p| p.join("aka-engine")).unwrap_or_else(|| {
        fallback_out_dir
            .map(|p| p.join(".aka-engine-cache"))
            .unwrap_or_else(|| repo.join(".aka-engine-cache"))
    })
}

fn engine_exe_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["aka-engine.exe"]
    } else {
        &["aka-engine"]
    }
}

fn resolve_engine_binary(base: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if base.is_file() {
        candidates.push(base.to_path_buf());
    } else {
        for name in engine_exe_names() {
            candidates.extend([
                base.join(name),
                base.join("bin").join(name),
                base.join("build/c").join(name),
            ]);
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

fn engine_mode() -> String {
    match std::env::var("AKA_ENGINE_MODE") {
        Ok(mode) if matches!(mode.as_str(), "fast" | "moderate" | "full") => mode,
        _ => DEFAULT_ENGINE_MODE.to_string(),
    }
}

fn emit_phase(
    on_event: &mut (impl FnMut(&EngineEvent) + ?Sized),
    phase: impl Into<String>,
    current: u64,
    total: u64,
) {
    on_event(&EngineEvent::Phase {
        phase: phase.into(),
        current,
        total,
    });
}

fn synth_stage_timeout() -> Duration {
    std::env::var("AKA_SYNTH_STAGE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_SYNTH_STAGE_TIMEOUT)
}

enum SynthWorkerMessage<T> {
    Progress {
        phase: String,
        current: u64,
        total: u64,
    },
    Done(T),
}

fn synthesize_with_timeout_and_progress<T, F>(
    on_event: &mut impl FnMut(&EngineEvent),
    phase: &str,
    timeout: Duration,
    default: T,
    f: F,
) -> T
where
    T: Send + 'static,
    F: FnOnce(&mut dyn FnMut(String, u64, u64)) -> T + Send + 'static,
{
    emit_phase(on_event, phase, 0, 0);
    let started = Instant::now();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let progress_tx = tx.clone();
        let mut progress = move |phase: String, current: u64, total: u64| {
            let _ = progress_tx.send(SynthWorkerMessage::Progress {
                phase,
                current,
                total,
            });
        };
        let result = f(&mut progress);
        let _ = tx.send(SynthWorkerMessage::Done(result));
    });

    loop {
        let elapsed = started.elapsed();
        let Some(remaining) = timeout.checked_sub(elapsed) else {
            on_event(&EngineEvent::Log {
                stream: "adapter".into(),
                line: format!(
                    "{phase}:timeout elapsed_ms={} skipped=true",
                    started.elapsed().as_millis()
                ),
            });
            return default;
        };
        match rx.recv_timeout(remaining.min(Duration::from_secs(1))) {
            Ok(SynthWorkerMessage::Progress {
                phase,
                current,
                total,
            }) => emit_phase(on_event, phase, current, total),
            Ok(SynthWorkerMessage::Done(result)) => {
                on_event(&EngineEvent::Log {
                    stream: "adapter".into(),
                    line: format!("{phase}:done elapsed_ms={}", started.elapsed().as_millis()),
                });
                return result;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if started.elapsed() >= timeout {
                    on_event(&EngineEvent::Log {
                        stream: "adapter".into(),
                        line: format!(
                            "{phase}:timeout elapsed_ms={} skipped=true",
                            started.elapsed().as_millis()
                        ),
                    });
                    return default;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                on_event(&EngineEvent::Warning {
                    message: format!(
                        "{phase} worker disconnected; continuing without this synthesis"
                    ),
                });
                return default;
            }
        }
    }
}

/// Run a single source-synthesis stage on a worker thread with a hard timeout.
/// On timeout (or worker panic) the stage is skipped, a `skipped=true` line is
/// logged to the adapter stream, and `default` is returned so indexing keeps
/// going instead of hanging in the enrichment stage. This is the no-progress
/// sibling of [`synthesize_with_timeout_and_progress`].
fn run_synth_stage<T, F>(
    on_event: &mut impl FnMut(&EngineEvent),
    phase: &str,
    timeout: Duration,
    default: T,
    f: F,
) -> T
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    emit_phase(on_event, phase, 0, 0);
    let started = Instant::now();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });

    loop {
        let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
            on_event(&EngineEvent::Log {
                stream: "adapter".into(),
                line: format!(
                    "{phase}:timeout elapsed_ms={} skipped=true",
                    started.elapsed().as_millis()
                ),
            });
            return default;
        };
        match rx.recv_timeout(remaining.min(Duration::from_secs(1))) {
            Ok(result) => {
                on_event(&EngineEvent::Log {
                    stream: "adapter".into(),
                    line: format!("{phase}:done elapsed_ms={}", started.elapsed().as_millis()),
                });
                return result;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if started.elapsed() >= timeout {
                    on_event(&EngineEvent::Log {
                        stream: "adapter".into(),
                        line: format!(
                            "{phase}:timeout elapsed_ms={} skipped=true",
                            started.elapsed().as_millis()
                        ),
                    });
                    return default;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                on_event(&EngineEvent::Warning {
                    message: format!("{phase} worker panicked; continuing without this synthesis"),
                });
                return default;
            }
        }
    }
}

fn push_tail(tail: &mut Vec<String>, line: String, limit: usize) {
    tail.push(line);
    if tail.len() > limit {
        let drop_count = tail.len() - limit;
        tail.drain(0..drop_count);
    }
}

fn engine_failure_context(
    repo: &Path,
    cache_root: &Path,
    engine_bin: &Path,
    mode: &str,
    stdout_tail: &[String],
    stderr_tail: &[String],
) -> String {
    let mut out = vec![
        format!("repo_path={}", repo.display()),
        format!("cache_dir={}", cache_root.display()),
        format!("mode={mode}"),
        format!("engine_bin={}", engine_bin.display()),
    ];
    if !stdout_tail.is_empty() {
        out.push("stdout tail:".into());
        out.extend(stdout_tail.iter().cloned());
    }
    if !stderr_tail.is_empty() {
        out.push("stderr tail:".into());
        out.extend(stderr_tail.iter().cloned());
    }
    out.join("\n")
}

fn parse_engine_progress_phase(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "Starting incremental index" {
        return Some("aka-engine:incremental-index".into());
    }
    if trimmed == "Starting full index" {
        return Some("aka-engine:full-index".into());
    }
    if trimmed.starts_with('[') {
        return Some(format!("aka-engine:{trimmed}"));
    }
    if trimmed.starts_with("Discovering files") || trimmed.starts_with("Extracting:") {
        return Some(format!("aka-engine:{trimmed}"));
    }
    None
}

fn find_single_project_db(
    cache_root: &Path,
    repo: &Path,
) -> Result<(String, PathBuf), EngineError> {
    let mut candidates = Vec::new();
    for root in project_db_search_roots(cache_root) {
        collect_engine_db_candidates(&root, &mut candidates)?;
    }
    candidates.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });

    let expected_root = normalize_project_root(repo);
    let mut fallback = None;
    for candidate in candidates {
        let conn = match open_engine_db(&candidate.path) {
            Ok(conn) => conn,
            Err(_) => continue,
        };
        let project = conn.query_row("SELECT name, root_path FROM projects LIMIT 1", [], |row| {
            Ok(ProjectDbRow {
                name: row.get(0)?,
                root_path: row.get(1).ok(),
            })
        });
        match project {
            Ok(project) => {
                if project
                    .root_path
                    .as_deref()
                    .map(normalize_root_str)
                    .as_deref()
                    == Some(expected_root.as_str())
                {
                    return Ok((project.name, candidate.path));
                }
                fallback.get_or_insert((project.name, candidate.path));
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(err) => return Err(err.into()),
        }
    }

    fallback.ok_or_else(|| EngineError::MissingProject(cache_root.to_path_buf()))
}

fn project_db_search_roots(cache_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![cache_root.to_path_buf()];
    if let Some(home) = home_dir() {
        roots.push(home.join(".cache").join("aka-engine"));
        roots.push(home.join(".cache").join("codebase-memory-mcp"));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|home| !home.is_empty()))
        .map(PathBuf::from)
}

#[derive(Debug)]
struct ProjectDbCandidate {
    path: PathBuf,
    modified: SystemTime,
}

#[derive(Debug)]
struct ProjectDbRow {
    name: String,
    root_path: Option<String>,
}

fn collect_engine_db_candidates(
    dir: &Path,
    candidates: &mut Vec<ProjectDbCandidate>,
) -> Result<(), EngineError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            collect_engine_db_candidates(&path, candidates)?;
        } else if metadata.is_file()
            && path.extension().and_then(|e| e.to_str()) == Some("db")
            && !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("_config.db"))
        {
            candidates.push(ProjectDbCandidate {
                path,
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(())
}

fn normalize_project_root(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
        .replace('\\', "/")
}

fn normalize_root_str(path: &str) -> String {
    path.replace('\\', "/")
}

fn open_engine_db(path: &Path) -> Result<Connection, rusqlite::Error> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
}

fn export_artifacts(
    repo: &Path,
    out_dir: &Path,
    db_path: &Path,
    project: &str,
    no_chunks: bool,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<ArtifactStats, EngineError> {
    let conn = open_engine_db(db_path)?;
    emit_phase(on_event, "aka-engine:export-artifacts:inspect-db", 0, 0);
    let db_counts = ArtifactStats {
        files: count_files(&conn, project)?,
        nodes: count_nodes(&conn, project)?,
        edges: count_edges(&conn, project)?,
        chunks: count_chunkable_nodes(&conn, project)?,
    };
    if let Err(err) = warn_missing_source_extensions(repo, &conn, project, on_event) {
        on_event(&EngineEvent::Warning {
            message: format!("source language coverage check failed: {err}"),
        });
    }

    emit_phase(
        on_event,
        format!(
            "aka-core:enrichment-graph ({} nodes / {} edges)",
            db_counts.nodes, db_counts.edges
        ),
        0,
        0,
    );
    let synth = synthesize_graph_with_progress(&conn, project, repo, on_event)?;

    emit_phase(
        on_event,
        "aka-engine:export-artifacts:nodes",
        0,
        db_counts.nodes,
    );
    let nodes = export_nodes(
        &conn,
        project,
        &out_dir.join("nodes.ndjson"),
        &synth,
        db_counts.nodes,
        on_event,
    )?;
    emit_phase(
        on_event,
        "aka-engine:export-artifacts:edges",
        0,
        db_counts.edges,
    );
    let edges = export_edges(
        &conn,
        project,
        &out_dir.join("edges.ndjson"),
        &synth,
        db_counts.edges,
        on_event,
    )?;
    let mut stats = ArtifactStats {
        files: db_counts.files,
        nodes,
        edges,
        chunks: 0,
    };
    if no_chunks {
        let _ = std::fs::remove_file(out_dir.join("chunks.ndjson"));
    } else {
        emit_phase(
            on_event,
            "aka-engine:export-artifacts:chunks",
            0,
            db_counts.chunks,
        );
        stats.chunks = export_chunks(
            &conn,
            project,
            repo,
            &out_dir.join("chunks.ndjson"),
            db_counts.chunks,
            on_event,
        )?;
    }

    emit_phase(on_event, "aka-engine:export-artifacts:manifest", 0, 0);
    let manifest = Manifest {
        contract_version: CONTRACT_VERSION,
        engine_version: format!("aka-engine ({})", db_path.display()),
        repo_path: repo.display().to_string(),
        commit: git_head(repo),
        generated_at: Utc::now().to_rfc3339(),
        stats: stats.clone(),
    };
    let manifest_path = out_dir.join("manifest.json");
    let file = File::create(manifest_path)?;
    serde_json::to_writer_pretty(BufWriter::new(file), &manifest)?;
    Ok(stats)
}

fn collect_engine_facts(
    repo: &Path,
    db_path: &Path,
    project: &str,
    no_chunks: bool,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<FactBatch, EngineError> {
    let conn = open_engine_db(db_path)?;
    emit_phase(on_event, "aka-engine:facts:inspect-db", 0, 0);
    let db_counts = ArtifactStats {
        files: count_files(&conn, project)?,
        nodes: count_nodes(&conn, project)?,
        edges: count_edges(&conn, project)?,
        chunks: count_chunkable_nodes(&conn, project)?,
    };
    if let Err(err) = warn_missing_source_extensions(repo, &conn, project, on_event) {
        on_event(&EngineEvent::Warning {
            message: format!("source language coverage check failed: {err}"),
        });
    }

    emit_phase(
        on_event,
        format!(
            "aka-engine:facts:synthesize-graph ({} nodes / {} edges)",
            db_counts.nodes, db_counts.edges
        ),
        0,
        0,
    );
    let synth = synthesize_graph_with_progress(&conn, project, repo, on_event)?;

    let nodes = collect_nodes(&conn, project, &synth, db_counts.nodes, on_event)?;
    let edges = collect_edges(&conn, project, &synth, db_counts.edges, on_event)?;
    let chunks = if no_chunks {
        Vec::new()
    } else {
        collect_chunks(&conn, project, repo, db_counts.chunks, on_event)?
    };
    let stats = ArtifactStats {
        files: db_counts.files,
        nodes: nodes.len() as u64,
        edges: edges.len() as u64,
        chunks: chunks.len() as u64,
    };
    Ok(FactBatch::new(stats, nodes, edges, chunks))
}

fn count_nodes(conn: &Connection, project: &str) -> Result<u64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) FROM nodes WHERE project = ?1",
        [project],
        |row| row.get(0),
    )
}

fn count_edges(conn: &Connection, project: &str) -> Result<u64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) FROM edges WHERE project = ?1",
        [project],
        |row| row.get(0),
    )
}

fn count_chunkable_nodes(conn: &Connection, project: &str) -> Result<u64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) \
         FROM nodes \
         WHERE project = ?1 AND file_path != '' AND label NOT IN ('File','Folder','Project','Package','Module')",
        [project],
        |row| row.get(0),
    )
}

fn count_files(conn: &Connection, project: &str) -> Result<u64, rusqlite::Error> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM file_hashes WHERE project = ?1",
        [project],
        |row| row.get(0),
    )?;
    if count > 0 {
        return Ok(count);
    }
    conn.query_row(
        "SELECT COUNT(DISTINCT file_path) FROM nodes WHERE project = ?1 AND file_path != ''",
        [project],
        |row| row.get(0),
    )
}

fn warn_missing_source_extensions(
    repo: &Path,
    conn: &Connection,
    project: &str,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<(), EngineError> {
    let project_sources = ProjectSourceSet::discover(repo);
    let repo_exts = repo_source_extensions(repo, &project_sources)?;
    if repo_exts.is_empty() {
        return Ok(());
    }
    let indexed_exts = indexed_project_source_extensions(conn, project, repo, &project_sources)?;
    for (ext, language) in [
        ("java", "Java"),
        ("py", "Python"),
        ("go", "Go"),
        ("rs", "Rust"),
        ("ts", "TypeScript"),
        ("tsx", "TSX"),
        ("js", "JavaScript"),
        ("jsx", "JSX"),
    ] {
        if repo_exts.contains(ext) && !indexed_exts.contains(ext) {
            on_event(&EngineEvent::Warning {
                message: format!(
                    "AKA engine indexed 0 {language} source files even though the repository contains .{ext} files; graph/search may be incomplete. Try AKA_ENGINE_MODE=full or fix the AKA engine discovery rules."
                ),
            });
        }
    }
    Ok(())
}

fn indexed_project_source_extensions(
    conn: &Connection,
    project: &str,
    repo: &Path,
    project_sources: &ProjectSourceSet,
) -> Result<HashSet<String>, EngineError> {
    let mut exts = HashSet::new();
    if let Some(file_hash_col) = file_hashes_path_column(conn)? {
        let sql = format!("SELECT {file_hash_col} FROM file_hashes WHERE project = ?1");
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query([project])?;
        while let Some(row) = rows.next()? {
            let file_path = text_col(row, 0)?;
            if !project_sources.contains_project_file(repo, &file_path) {
                continue;
            }
            if let Some(ext) = source_extension(&file_path) {
                exts.insert(ext);
            }
        }
    }
    let mut stmt = conn
        .prepare("SELECT DISTINCT file_path FROM nodes WHERE project = ?1 AND file_path != ''")?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let file_path = text_col(row, 0)?;
        if !project_sources.contains_project_file(repo, &file_path) {
            continue;
        }
        if let Some(ext) = source_extension(&file_path) {
            exts.insert(ext);
        }
    }
    Ok(exts)
}

fn file_hashes_path_column(conn: &Connection) -> Result<Option<&'static str>, EngineError> {
    let mut stmt = conn.prepare("PRAGMA table_info(file_hashes)")?;
    let mut rows = stmt.query([])?;
    let mut has_rel_path = false;
    let mut has_file_path = false;
    while let Some(row) = rows.next()? {
        let name = text_col(row, 1)?;
        if name == "rel_path" {
            has_rel_path = true;
        } else if name == "file_path" {
            has_file_path = true;
        }
    }
    Ok(if has_rel_path {
        Some("rel_path")
    } else if has_file_path {
        Some("file_path")
    } else {
        None
    })
}

fn repo_source_extensions(
    repo: &Path,
    project_sources: &ProjectSourceSet,
) -> Result<HashSet<String>, EngineError> {
    if project_sources.has_git_listing() {
        return Ok(project_sources
            .project_files(repo)
            .filter_map(source_extension)
            .collect());
    }
    let mut exts = HashSet::new();
    collect_repo_source_extensions(repo, repo, &mut exts)?;
    Ok(exts)
}

fn collect_repo_source_extensions(
    repo: &Path,
    dir: &Path,
    exts: &mut HashSet<String>,
) -> Result<(), EngineError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if file_type.is_dir() {
            if is_source_discovery_skip_dir(name) {
                continue;
            }
            collect_repo_source_extensions(repo, &path, exts)?;
        } else if file_type.is_file() {
            let Ok(rel) = path.strip_prefix(repo) else {
                continue;
            };
            if let Some(ext) = source_extension(&rel.to_string_lossy()) {
                exts.insert(ext);
            }
        }
    }
    Ok(())
}

fn source_extension(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let ext = Path::new(&path)
        .extension()
        .and_then(|v| v.to_str())?
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "java" | "py" | "go" | "rs" | "ts" | "tsx" | "js" | "jsx"
    )
    .then_some(ext)
}

fn is_source_discovery_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | "vendor"
            | "vendors"
            | "target"
            | "build"
            | "dist"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".idea"
            | ".vscode"
            | ".next"
            | ".nuxt"
            | ".turbo"
            | "coverage"
    )
}

fn export_nodes(
    conn: &Connection,
    project: &str,
    path: &Path,
    synth: &SynthGraph,
    total: u64,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<u64, EngineError> {
    let file = File::create(path)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let nodes = collect_nodes(conn, project, synth, total, on_event)?;
    for node in &nodes {
        serde_json::to_writer(&mut out, node)?;
        out.write_all(b"\n")?;
    }
    Ok(nodes.len() as u64)
}

fn collect_nodes(
    conn: &Connection,
    project: &str,
    synth: &SynthGraph,
    total: u64,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<Vec<NodeRec>, EngineError> {
    emit_phase(on_event, "aka-engine:facts:nodes:query", 0, total);
    let mut stmt = conn.prepare(
        "SELECT id, label, name, qualified_name, file_path, start_line, end_line, properties \
         FROM nodes WHERE project = ?1 ORDER BY id",
    )?;
    let mut rows = stmt.query([project])?;
    let mut count = 0;
    let mut nodes = Vec::new();
    let mut progress = ExportProgress::new("nodes", total);
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let label = text_col(row, 1)?;
        let mut name = text_col(row, 2)?;
        let qn = text_col(row, 3)?;
        let file_path = text_col(row, 4)?;
        let start_line: i64 = row.get(5)?;
        let end_line: i64 = row.get(6)?;
        let props_text = text_col(row, 7)?;

        let mut properties = parse_props(&props_text);
        if label == "Route" {
            if let Some(route) = route_from_path(&file_path) {
                name = route;
                properties.insert("name".into(), Value::String(name.clone()));
            }
            sanitize_string_array_prop(&mut properties, "responseKeys");
            sanitize_string_array_prop(&mut properties, "errorKeys");
        }
        insert_if_missing(&mut properties, "name", Value::String(name));
        insert_if_missing(&mut properties, "qualifiedName", Value::String(qn.clone()));
        insert_if_missing(&mut properties, "filePath", Value::String(file_path));
        insert_if_missing(
            &mut properties,
            "startLine",
            Value::from(to_artifact_line(start_line)),
        );
        insert_if_missing(
            &mut properties,
            "endLine",
            Value::from(to_artifact_line(end_line)),
        );
        properties.insert("cbmId".into(), Value::from(cbm_id));

        let node = NodeRec {
            id: aka_node_id(cbm_id, &qn),
            label,
            properties,
        };
        nodes.push(node);
        count += 1;
        progress.emit(on_event, count);
    }
    emit_phase(on_event, "aka-engine:facts:nodes:synthetic", count, total);
    for community in &synth.communities {
        nodes.push(community.node_rec());
        count += 1;
    }
    for process in &synth.processes {
        nodes.push(process.node_rec());
        count += 1;
    }
    for property in &synth.properties {
        nodes.push(property.node_rec());
        count += 1;
    }
    for route in synth.routes.iter().filter(|r| r.emit_node) {
        nodes.push(route.node_rec());
        count += 1;
    }
    for tool in synth.tools.iter().filter(|t| t.emit_node) {
        nodes.push(tool.node_rec());
        count += 1;
    }
    for node in synth
        .commands
        .iter()
        .filter_map(SynthCommand::handler_node_rec)
    {
        nodes.push(node);
        count += 1;
    }
    for command in &synth.commands {
        nodes.push(command.node_rec());
        count += 1;
    }
    for config in &synth.configs {
        nodes.push(config.node_rec());
        count += 1;
    }
    for job in &synth.jobs {
        nodes.push(job.node_rec());
        count += 1;
    }
    for topic in &synth.topics {
        nodes.push(topic.node_rec());
        count += 1;
    }
    for cache in &synth.caches {
        nodes.push(cache.node_rec());
        count += 1;
    }
    for event in &synth.events {
        nodes.push(event.node_rec());
        count += 1;
    }
    for policy in &synth.policies {
        nodes.push(policy.node_rec());
        count += 1;
    }
    for resource in &synth.resources {
        nodes.push(resource.node_rec());
        count += 1;
    }
    for operation in &synth.graphql {
        nodes.push(operation.node_rec());
        count += 1;
    }
    for symbol in &synth.source_symbols {
        nodes.push(symbol.node_rec());
        count += 1;
    }
    for node in synth.persistence.node_recs() {
        nodes.push(node);
        count += 1;
    }
    for transaction in &synth.transactions {
        nodes.push(transaction.node_rec());
        count += 1;
    }
    progress.emit_force(on_event, count);
    Ok(nodes)
}

fn export_edges(
    conn: &Connection,
    project: &str,
    path: &Path,
    synth: &SynthGraph,
    total: u64,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<u64, EngineError> {
    let file = File::create(path)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let edges = collect_edges(conn, project, synth, total, on_event)?;
    for edge in &edges {
        serde_json::to_writer(&mut out, edge)?;
        out.write_all(b"\n")?;
    }
    Ok(edges.len() as u64)
}

fn collect_edges(
    conn: &Connection,
    project: &str,
    synth: &SynthGraph,
    total: u64,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<Vec<EdgeRec>, EngineError> {
    emit_phase(on_event, "aka-engine:facts:edges:query", 0, total);
    let mut stmt = conn.prepare(
        "SELECT e.id, e.source_id, e.target_id, e.type, e.properties, \
                s.qualified_name, t.qualified_name, s.label, t.label \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id \
         JOIN nodes t ON t.id = e.target_id \
         WHERE e.project = ?1 ORDER BY e.id",
    )?;
    let mut rows = stmt.query([project])?;
    let mut count = 0;
    let mut edges = Vec::new();
    let mut semantic = SemanticEdgeSynthesizer::load(conn, project)?;
    let mut progress = ExportProgress::new("edges", total);
    while let Some(row) = rows.next()? {
        let edge_id: i64 = row.get(0)?;
        let source_id: i64 = row.get(1)?;
        let target_id: i64 = row.get(2)?;
        let edge_type = text_col(row, 3)?;
        let props_text = text_col(row, 4)?;
        let source_qn = text_col(row, 5)?;
        let target_qn = text_col(row, 6)?;
        let props = props_value(&props_text);
        let source_label = text_col(row, 7)?;
        let target_label = text_col(row, 8)?;
        semantic.record(
            SemanticEndpoint::new(source_id, &source_qn, &source_label),
            SemanticEndpoint::new(target_id, &target_qn, &target_label),
            &edge_type,
        );
        let edge = EdgeRec {
            id: format!("cbm-edge:{edge_id}"),
            source_id: aka_node_id(source_id, &source_qn),
            target_id: aka_node_id(target_id, &target_qn),
            edge_type,
            confidence: props
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(1.0),
            reason: props
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("aka-engine")
                .to_string(),
            step: props.get("step").and_then(Value::as_u64).map(|v| v as u32),
            evidence: if props.is_null() { None } else { Some(props) },
        };
        edges.push(edge);
        count += 1;
        progress.emit(on_event, count);
    }
    emit_phase(on_event, "aka-engine:facts:edges:synthetic", count, total);
    for edge in semantic.edge_recs() {
        edges.push(edge);
        count += 1;
    }
    for edge in synth.properties.iter().map(SynthProperty::edge_rec) {
        edges.push(edge);
        count += 1;
    }
    for edge in synth
        .communities
        .iter()
        .flat_map(SynthCommunity::edge_recs)
        .chain(synth.processes.iter().flat_map(SynthProcess::edge_recs))
        .chain(synth.routes.iter().flat_map(SynthRoute::edge_recs))
        .chain(synth.tools.iter().flat_map(SynthTool::edge_recs))
        .chain(synth.commands.iter().flat_map(SynthCommand::edge_recs))
        .chain(synth.configs.iter().flat_map(SynthConfig::edge_recs))
        .chain(synth.jobs.iter().flat_map(SynthJob::edge_recs))
        .chain(synth.topics.iter().flat_map(SynthTopic::edge_recs))
        .chain(synth.caches.iter().flat_map(SynthCache::edge_recs))
        .chain(synth.events.iter().flat_map(SynthEvent::edge_recs))
        .chain(synth.policies.iter().flat_map(SynthPolicy::edge_recs))
        .chain(synth.resources.iter().flat_map(SynthResource::edge_recs))
        .chain(
            synth
                .graphql
                .iter()
                .flat_map(SynthGraphqlOperation::edge_recs),
        )
        .chain(synth.persistence.edge_recs())
        .chain(
            synth
                .transactions
                .iter()
                .flat_map(SynthTransaction::edge_recs),
        )
        .chain(synth.edges.iter().cloned())
    {
        edges.push(edge);
        count += 1;
    }
    progress.emit_force(on_event, count);
    Ok(edges)
}

struct ExportProgress {
    name: &'static str,
    total: u64,
    last_emit: Instant,
}

impl ExportProgress {
    fn new(name: &'static str, total: u64) -> Self {
        Self {
            name,
            total,
            last_emit: Instant::now(),
        }
    }

    fn emit(&mut self, on_event: &mut impl FnMut(&EngineEvent), count: u64) {
        let enough_rows = count == 1 || count.is_multiple_of(100);
        let enough_time = self.last_emit.elapsed() >= Duration::from_secs(1);
        let complete = self.total > 0 && count >= self.total;
        if enough_rows || enough_time || complete {
            self.emit_force(on_event, count);
        }
    }

    fn emit_force(&mut self, on_event: &mut impl FnMut(&EngineEvent), count: u64) {
        self.last_emit = Instant::now();
        emit_phase(
            on_event,
            format!("aka-engine:export-artifacts:{}", self.name),
            count,
            self.total,
        );
    }
}

#[derive(Debug, Clone, Default)]
struct SemanticEdgeSynthesizer {
    seen: HashSet<(String, String, String)>,
    out: Vec<EdgeRec>,
    nodes_by_qn: BTreeMap<String, SemanticNode>,
    methods_by_owner_name: BTreeMap<(String, String), Vec<SemanticNode>>,
    type_by_qn: BTreeMap<String, SemanticNode>,
    implements: Vec<(String, String)>,
}

impl SemanticEdgeSynthesizer {
    fn load(conn: &Connection, project: &str) -> Result<Self, rusqlite::Error> {
        let mut this = Self::default();
        let mut stmt = conn.prepare(
            "SELECT id, label, name, qualified_name \
             FROM nodes \
             WHERE project = ?1 AND label IN ('Class','Interface','Method','Field','Variable','Property') \
             ORDER BY id",
        )?;
        let mut rows = stmt.query([project])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let label = text_col(row, 1)?;
            let name = text_col(row, 2)?;
            let qn = text_col(row, 3)?;
            let node = SemanticNode {
                id: aka_node_id(id, &qn),
                label,
                name,
                qn: qn.clone(),
            };
            if matches!(node.label.as_str(), "Class" | "Interface") {
                this.type_by_qn.insert(qn.clone(), node.clone());
            }
            if node.label == "Method" {
                if let Some(owner) = semantic_owner_qn(&qn, &node.name) {
                    this.methods_by_owner_name
                        .entry((owner.to_string(), node.name.clone()))
                        .or_default()
                        .push(node.clone());
                }
            }
            this.nodes_by_qn.insert(qn, node);
        }
        this.add_member_properties();
        Ok(this)
    }

    fn record(
        &mut self,
        source: SemanticEndpoint<'_>,
        target: SemanticEndpoint<'_>,
        edge_type: &str,
    ) {
        match edge_type {
            "DEFINES_METHOD" => {
                self.add(
                    source.node_id(),
                    target.node_id(),
                    "HAS_METHOD",
                    "aka semantic edge from DEFINES_METHOD",
                );
            }
            "USAGE" if matches!(target.label, "Field" | "Variable" | "Property") => {
                self.add(
                    source.node_id(),
                    target.node_id(),
                    "ACCESSES",
                    "aka semantic edge from symbol usage",
                );
            }
            "INHERITS" | "IMPLEMENTS"
                if matches!(source.label, "Class") && matches!(target.label, "Interface") =>
            {
                self.add(
                    source.node_id(),
                    target.node_id(),
                    "IMPLEMENTS",
                    "aka semantic edge from inheritance reference",
                );
                self.implements
                    .push((source.qn.to_string(), target.qn.to_string()));
            }
            _ => {}
        }
    }

    fn edge_recs(mut self) -> Vec<EdgeRec> {
        for (class_qn, iface_qn) in self.implements.clone() {
            self.add_method_implements(&class_qn, &iface_qn);
        }
        self.out
    }

    fn add_member_properties(&mut self) {
        let nodes: Vec<_> = self.nodes_by_qn.values().cloned().collect();
        for member in nodes
            .iter()
            .filter(|n| matches!(n.label.as_str(), "Field" | "Property"))
        {
            let Some(owner_qn) = semantic_owner_qn(&member.qn, &member.name) else {
                continue;
            };
            let Some(owner) = self.type_by_qn.get(owner_qn) else {
                continue;
            };
            self.add(
                owner.id.clone(),
                member.id.clone(),
                "HAS_PROPERTY",
                "aka semantic edge from owned property",
            );
        }
    }

    fn add_method_implements(&mut self, class_qn: &str, iface_qn: &str) {
        let iface_methods: Vec<_> = self
            .methods_by_owner_name
            .iter()
            .filter(|((owner, _), _)| owner == iface_qn)
            .map(|((_, name), methods)| (name.clone(), methods.clone()))
            .collect();
        for (method_name, interface_methods) in iface_methods {
            let Some(class_methods) = self
                .methods_by_owner_name
                .get(&(class_qn.to_string(), method_name))
                .cloned()
            else {
                continue;
            };
            for class_method in &class_methods {
                for interface_method in &interface_methods {
                    self.add(
                        class_method.id.clone(),
                        interface_method.id.clone(),
                        "METHOD_IMPLEMENTS",
                        "aka semantic edge from class/interface method match",
                    );
                }
            }
        }
    }

    fn add(&mut self, source: String, target: String, edge_type: &str, reason: &str) {
        let key = (source.clone(), edge_type.to_string(), target.clone());
        if !self.seen.insert(key) {
            return;
        }
        let evidence = json!({
            "source": "aka-cbm-synth",
            "kind": "semantic-compat",
            "from": "aka-engine"
        });
        self.out.push(EdgeRec {
            id: format!(
                "semantic:{}:{:016x}",
                edge_type.to_ascii_lowercase(),
                stable_hash(&format!("{source}|{edge_type}|{target}"))
            ),
            source_id: source,
            target_id: target,
            edge_type: edge_type.into(),
            confidence: 0.86,
            reason: reason.into(),
            step: None,
            evidence: Some(evidence),
        });
    }
}

#[derive(Debug, Clone)]
struct SemanticNode {
    id: String,
    label: String,
    name: String,
    qn: String,
}

#[derive(Debug, Clone, Copy)]
struct SemanticEndpoint<'a> {
    id: i64,
    qn: &'a str,
    label: &'a str,
}

impl<'a> SemanticEndpoint<'a> {
    fn new(id: i64, qn: &'a str, label: &'a str) -> Self {
        Self { id, qn, label }
    }

    fn node_id(self) -> String {
        aka_node_id(self.id, self.qn)
    }
}

pub(super) fn semantic_owner_qn<'a>(member_qn: &'a str, name: &str) -> Option<&'a str> {
    let tail = member_qn.rsplit('.').next()?;
    if tail != name {
        return None;
    }
    member_qn.rsplit_once('.').map(|(owner, _)| owner)
}

#[derive(Debug, Clone)]
pub(super) struct SynthNode {
    aka_id: String,
    qn: String,
    label: String,
    name: String,
    file_path: String,
    start_line: i64,
    end_line: i64,
    language: String,
    route_path: Option<String>,
    route_method: Option<String>,
    decorators: Vec<String>,
    parent_class: Option<String>,
    is_exported: bool,
    ast_framework_multiplier: f64,
    ast_framework_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct SynthGraph {
    communities: Vec<SynthCommunity>,
    processes: Vec<SynthProcess>,
    routes: Vec<SynthRoute>,
    tools: Vec<SynthTool>,
    commands: Vec<SynthCommand>,
    configs: Vec<SynthConfig>,
    jobs: Vec<SynthJob>,
    topics: Vec<SynthTopic>,
    caches: Vec<SynthCache>,
    events: Vec<SynthEvent>,
    policies: Vec<SynthPolicy>,
    resources: Vec<SynthResource>,
    graphql: Vec<SynthGraphqlOperation>,
    persistence: SynthPersistenceGraph,
    transactions: Vec<SynthTransaction>,
    properties: Vec<SynthProperty>,
    source_symbols: Vec<SynthSourceSymbol>,
    edges: Vec<EdgeRec>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct CommunityRef {
    id: String,
    label: String,
}

#[derive(Debug, Clone)]
struct SynthCommunity {
    id: String,
    heuristic_label: String,
    cohesion: f64,
    members: Vec<SynthNode>,
}

impl SynthCommunity {
    fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.heuristic_label.clone()));
        properties.insert(
            "heuristicLabel".into(),
            Value::String(self.heuristic_label.clone()),
        );
        properties.insert("cohesion".into(), json!(self.cohesion));
        properties.insert("symbolCount".into(), Value::from(self.members.len() as u64));
        properties.insert(
            "keywords".into(),
            Value::Array(
                community_keywords(&self.members)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        properties.insert("enrichedBy".into(), Value::String("heuristic".into()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        NodeRec {
            id: self.id.clone(),
            label: "Community".into(),
            properties,
        }
    }

    fn edge_recs(&self) -> Vec<EdgeRec> {
        self.members
            .iter()
            .map(|member| EdgeRec {
                id: format!("{}:member:{:016x}", self.id, stable_hash(&member.aka_id)),
                source_id: member.aka_id.clone(),
                target_id: self.id.clone(),
                edge_type: "MEMBER_OF".into(),
                confidence: MIN_TRACE_CONFIDENCE,
                reason: "aka community synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "community-membership",
                    "heuristicLabel": self.heuristic_label.clone(),
                    "cohesion": self.cohesion,
                })),
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub(super) struct SynthProcess {
    id: String,
    name: String,
    process_type: String,
    communities: Vec<CommunityRef>,
    steps: Vec<SynthNode>,
    entry_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct SynthRoute {
    id: String,
    pub(super) route: String,
    pub(super) file_path: String,
    emit_node: bool,
    method: Option<String>,
    handler_id: Option<String>,
    handler_name: Option<String>,
    middleware: Vec<String>,
    response_keys: Vec<String>,
    error_keys: Vec<String>,
    pub(super) consumers: Vec<SynthRouteConsumer>,
    process_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct SynthRouteConsumer {
    pub(super) node_id: String,
    pub(super) keys: Vec<String>,
    pub(super) fetch_count: u32,
}

#[derive(Debug, Clone)]
pub(super) struct RouteCandidate {
    pub(super) route: String,
    pub(super) method: Option<String>,
    pub(super) handler_id: Option<String>,
    pub(super) handler_name: Option<String>,
}

impl SynthRoute {
    fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.route.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("routeSource".into(), Value::String("source-scan".into()));
        if let Some(method) = &self.method {
            properties.insert("method".into(), Value::String(method.clone()));
        }
        if let Some(handler_id) = &self.handler_id {
            properties.insert("handlerId".into(), Value::String(handler_id.clone()));
        }
        if let Some(handler_name) = &self.handler_name {
            properties.insert("handlerName".into(), Value::String(handler_name.clone()));
        }
        if !self.middleware.is_empty() {
            properties.insert(
                "middleware".into(),
                Value::Array(self.middleware.iter().cloned().map(Value::String).collect()),
            );
        }
        if !self.response_keys.is_empty() {
            properties.insert(
                "responseKeys".into(),
                Value::Array(
                    self.response_keys
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        if !self.error_keys.is_empty() {
            properties.insert(
                "errorKeys".into(),
                Value::Array(self.error_keys.iter().cloned().map(Value::String).collect()),
            );
        }
        NodeRec {
            id: self.id.clone(),
            label: "Route".into(),
            properties,
        }
    }

    fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::new();
        if let Some(handler_id) = &self.handler_id {
            out.push(EdgeRec {
                id: format!("{}:handles:{:016x}", self.id, stable_hash(handler_id)),
                source_id: handler_id.clone(),
                target_id: self.id.clone(),
                edge_type: "HANDLES_ROUTE".into(),
                confidence: 0.65,
                reason: "aka route synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "route-handler",
                    "route": self.route,
                })),
            });
        }
        for consumer in &self.consumers {
            out.push(EdgeRec {
                id: format!(
                    "{}:fetches:{:016x}",
                    self.id,
                    stable_hash(&consumer.node_id)
                ),
                source_id: consumer.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: "FETCHES".into(),
                confidence: 0.6,
                reason: fetch_reason(&consumer.keys, consumer.fetch_count),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "fetch-consumer",
                    "route": self.route,
                    "accessedKeys": consumer.keys,
                    "fetchCount": consumer.fetch_count,
                })),
            });
        }
        for process_id in &self.process_ids {
            out.push(EdgeRec {
                id: format!("{}:entry-process:{:016x}", self.id, stable_hash(process_id)),
                source_id: self.id.clone(),
                target_id: process_id.clone(),
                edge_type: "ENTRY_POINT_OF".into(),
                confidence: 0.55,
                reason: "aka route process linkage".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "route-entry-process",
                    "route": self.route,
                })),
            });
        }
        out
    }
}

impl SynthProcess {
    fn node_rec(&self) -> NodeRec {
        let entry = self.steps.first().expect("process has entry");
        let terminal = self.steps.last().expect("process has terminal");
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert(
            "processType".into(),
            Value::String(self.process_type.clone()),
        );
        properties.insert(
            "communities".into(),
            Value::Array(
                self.communities
                    .iter()
                    .map(|c| json!({"id": c.id.clone(), "label": c.label.clone()}))
                    .collect(),
            ),
        );
        properties.insert(
            "communityIds".into(),
            Value::Array(
                self.communities
                    .iter()
                    .map(|c| Value::String(c.id.clone()))
                    .collect(),
            ),
        );
        properties.insert(
            "communityLabels".into(),
            Value::Array(
                self.communities
                    .iter()
                    .map(|c| Value::String(c.label.clone()))
                    .collect(),
            ),
        );
        properties.insert("stepCount".into(), Value::from(self.steps.len() as u64));
        properties.insert("entryPointId".into(), Value::String(entry.aka_id.clone()));
        properties.insert("terminalId".into(), Value::String(terminal.aka_id.clone()));
        properties.insert(
            "trace".into(),
            Value::Array(
                self.steps
                    .iter()
                    .map(|step| Value::String(step.aka_id.clone()))
                    .collect(),
            ),
        );
        properties.insert("heuristicLabel".into(), Value::String(self.name.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        if let Some(reason) = &self.entry_reason {
            properties.insert("entryReason".into(), Value::String(reason.clone()));
        }
        NodeRec {
            id: self.id.clone(),
            label: "Process".into(),
            properties,
        }
    }

    fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::with_capacity(self.steps.len() + 1);
        let entry = self.steps.first().expect("process has entry");
        out.push(EdgeRec {
            id: format!("{}:entry", self.id),
            source_id: entry.aka_id.clone(),
            target_id: self.id.clone(),
            edge_type: "ENTRY_POINT_OF".into(),
            confidence: 0.7,
            reason: "aka process synthesis".into(),
            step: None,
            evidence: Some(json!({"source": "aka-cbm-synth", "kind": "entry"})),
        });
        for (idx, step) in self.steps.iter().enumerate() {
            let step_no = (idx + 1) as u32;
            out.push(EdgeRec {
                id: format!("{}:step:{step_no}", self.id),
                source_id: step.aka_id.clone(),
                target_id: self.id.clone(),
                edge_type: "STEP_IN_PROCESS".into(),
                confidence: 0.7,
                reason: "aka process synthesis".into(),
                step: Some(step_no),
                evidence: Some(json!({"source": "aka-cbm-synth", "kind": "call-chain"})),
            });
        }
        out
    }
}

fn synthesize_graph_with_progress(
    conn: &Connection,
    project: &str,
    repo: &Path,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<SynthGraph, EngineError> {
    emit_phase(on_event, "aka-core:enrichment:native-labels", 0, 0);
    let native_communities = has_native_label(conn, project, "Community")?;
    let native_processes = has_native_label(conn, project, "Process")?;
    let native_routes = load_native_app_nodes(conn, project, "Route")?;
    let native_tools = load_native_app_nodes(conn, project, "Tool")?;
    emit_phase(on_event, "aka-core:enrichment:nodes", 0, 0);
    let mut nodes = load_synth_nodes(conn, project)?;
    let source_symbols = synthesize_source_symbols_from_sources(repo, &nodes);
    for symbol in &source_symbols {
        nodes.insert(symbol.node().aka_id.clone(), symbol.node().clone());
    }
    let existing_node_ids = load_existing_node_ids(conn, project)?;
    let properties = synthesize_python_properties(conn, project, repo, &existing_node_ids)?;
    if nodes.is_empty() {
        let processes = Vec::new();
        let native_routes = Vec::new();
        let native_tools = Vec::new();
        let commands = synthesize_commands_from_sources(repo, &nodes, &processes);
        let routes = synthesize_routes_from_sources(repo, &nodes, &processes, &native_routes);
        let tools = synthesize_tools_from_sources(repo, &nodes, &processes, &native_tools);
        let persistence = synthesize_persistence_from_sources(repo, &nodes);
        let configs = synthesize_configs_from_sources(repo, &nodes);
        let jobs = synthesize_jobs_from_sources(repo, &nodes, &processes);
        let mut topics = synthesize_topics_from_sources(repo, &nodes);
        merge_native_channel_topics(
            &mut topics,
            load_native_channel_topic_detections(conn, project, repo)?,
        );
        let caches = synthesize_caches_from_sources(repo, &nodes);
        let events = synthesize_events_from_sources(repo, &nodes);
        let policies = synthesize_policies_from_sources(repo, &nodes);
        let resources = synthesize_resources_from_sources(repo, &nodes);
        let graphql = synthesize_graphql_from_sources(repo, &nodes, &processes);
        let transactions = synthesize_transactions_from_sources(repo, &nodes);
        return Ok(SynthGraph {
            processes,
            routes,
            tools,
            commands,
            properties,
            persistence,
            configs,
            jobs,
            topics,
            caches,
            events,
            policies,
            resources,
            graphql,
            transactions,
            source_symbols,
            ..SynthGraph::default()
        });
    }
    emit_phase(
        on_event,
        format!(
            "aka-core:enrichment:calls ({} process-step nodes)",
            nodes.len()
        ),
        0,
        0,
    );
    emit_phase(on_event, "aka-core:enrichment:existing-call-pairs", 0, 0);
    let existing_call_pairs = load_existing_call_pairs(conn, project)?;
    emit_phase(on_event, "aka-core:enrichment:dependency-edges", 0, 0);
    let synthetic_edges =
        synthesize_dependency_edges_from_sources(repo, &nodes, &existing_call_pairs, |progress| {
            let (phase, current, total) = dependency_progress_phase(progress);
            emit_phase(on_event, phase, current, total);
        });
    emit_phase(
        on_event,
        format!(
            "aka-core:enrichment:call-graph ({} synthetic edges)",
            synthetic_edges.len()
        ),
        0,
        0,
    );
    let calls = load_call_graph(conn, project, &nodes, &synthetic_edges)?;
    let project_sources = ProjectSourceSet::discover(repo);
    emit_phase(on_event, "aka-core:enrichment:project-subgraph", 0, 0);
    let process_nodes = project_process_nodes(repo, &nodes, &project_sources);
    let process_calls = calls.project_subgraph(&process_nodes);
    emit_phase(
        on_event,
        format!(
            "aka-core:enrichment:process-hints ({} process nodes / {} call edges)",
            process_nodes.len(),
            process_calls.edges.len()
        ),
        0,
        0,
    );
    let mut command_entry_hints = command_entry_hints_from_sources(repo, &nodes);
    for (handler_id, strategy) in job_entry_hints_from_sources(repo, &nodes) {
        command_entry_hints
            .entry(handler_id)
            .or_insert(CommandEntryHint { strategy });
    }

    emit_phase(on_event, "aka-core:enrichment:communities", 0, 0);
    let communities = if native_communities {
        Vec::new()
    } else {
        synthesize_communities(&process_nodes, &process_calls.edges)
    };
    emit_phase(on_event, "aka-core:enrichment:community-memberships", 0, 0);
    let community_memberships = if native_communities {
        load_native_community_memberships(conn, project, &nodes)?
    } else {
        community_memberships_from_synth(&communities)
    };
    emit_phase(on_event, "aka-core:enrichment:processes", 0, 0);
    let processes = if native_processes {
        Vec::new()
    } else {
        let symbol_count = count_process_symbol_basis(conn, project)?;
        synthesize_processes_from_calls(
            &process_nodes,
            &process_calls.adjacency,
            &process_calls.indegree,
            &community_memberships,
            &command_entry_hints,
            symbol_count,
        )
    };
    // Share the (read-only) inputs across stage worker threads cheaply, and run
    // every source-scanning stage under a hard timeout so a single pathological
    // file or repo can't wedge the enrichment stage. On timeout a stage is skipped
    // (logged with `skipped=true`) and indexing continues with partial results.
    let repo_arc = Arc::new(repo.to_path_buf());
    let nodes = Arc::new(nodes);
    let processes = Arc::new(processes);
    let stage_timeout = synth_stage_timeout();

    let routes = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        let processes = Arc::clone(&processes);
        let native_routes = native_routes.clone();
        synthesize_with_timeout_and_progress(
            on_event,
            "aka-core:enrichment:routes",
            stage_timeout,
            Vec::new(),
            move |progress| {
                synthesize_routes_from_sources_with_progress(
                    repo.as_path(),
                    nodes.as_ref(),
                    processes.as_slice(),
                    &native_routes,
                    progress,
                )
            },
        )
    };
    let tools = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        let processes = Arc::clone(&processes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:tools",
            stage_timeout,
            Vec::new(),
            move || {
                synthesize_tools_from_sources(
                    repo.as_path(),
                    nodes.as_ref(),
                    processes.as_slice(),
                    &native_tools,
                )
            },
        )
    };
    let commands = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        let processes = Arc::clone(&processes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:commands",
            stage_timeout,
            Vec::new(),
            move || {
                synthesize_commands_from_sources(
                    repo.as_path(),
                    nodes.as_ref(),
                    processes.as_slice(),
                )
            },
        )
    };
    let configs = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:configs",
            stage_timeout,
            Vec::new(),
            move || synthesize_configs_from_sources(repo.as_path(), nodes.as_ref()),
        )
    };
    let jobs = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        let processes = Arc::clone(&processes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:jobs",
            stage_timeout,
            Vec::new(),
            move || {
                synthesize_jobs_from_sources(repo.as_path(), nodes.as_ref(), processes.as_slice())
            },
        )
    };
    let mut topics = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:topics",
            stage_timeout,
            Vec::new(),
            move || synthesize_topics_from_sources(repo.as_path(), nodes.as_ref()),
        )
    };
    merge_native_channel_topics(
        &mut topics,
        load_native_channel_topic_detections(conn, project, repo)?,
    );
    let caches = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:caches",
            stage_timeout,
            Vec::new(),
            move || synthesize_caches_from_sources(repo.as_path(), nodes.as_ref()),
        )
    };
    let events = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:events",
            stage_timeout,
            Vec::new(),
            move || synthesize_events_from_sources(repo.as_path(), nodes.as_ref()),
        )
    };
    let policies = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:policies",
            stage_timeout,
            Vec::new(),
            move || synthesize_policies_from_sources(repo.as_path(), nodes.as_ref()),
        )
    };
    let resources = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:resources",
            stage_timeout,
            Vec::new(),
            move || synthesize_resources_from_sources(repo.as_path(), nodes.as_ref()),
        )
    };
    let graphql = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        let processes = Arc::clone(&processes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:graphql",
            stage_timeout,
            Vec::new(),
            move || {
                synthesize_graphql_from_sources(
                    repo.as_path(),
                    nodes.as_ref(),
                    processes.as_slice(),
                )
            },
        )
    };
    let persistence = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:persistence",
            stage_timeout,
            SynthPersistenceGraph::default(),
            move || synthesize_persistence_from_sources(repo.as_path(), nodes.as_ref()),
        )
    };
    let transactions = {
        let repo = Arc::clone(&repo_arc);
        let nodes = Arc::clone(&nodes);
        run_synth_stage(
            on_event,
            "aka-core:enrichment:transactions",
            stage_timeout,
            Vec::new(),
            move || synthesize_transactions_from_sources(repo.as_path(), nodes.as_ref()),
        )
    };

    let processes = Arc::try_unwrap(processes).unwrap_or_else(|shared| (*shared).clone());

    Ok(SynthGraph {
        communities,
        processes,
        routes,
        tools,
        commands,
        configs,
        jobs,
        topics,
        caches,
        events,
        policies,
        resources,
        graphql,
        persistence,
        transactions,
        properties,
        source_symbols,
        edges: synthetic_edges,
    })
}

fn dependency_progress_phase(progress: DependencyProgress) -> (String, u64, u64) {
    let base = "aka-core:enrichment:dependency-edges";
    let phase = match progress.phase {
        DependencyProgressPhase::Start => base.to_string(),
        DependencyProgressPhase::FileStart {
            file_path,
            node_count,
        } => format!("{base}:file-start path={file_path} nodes={node_count}"),
        DependencyProgressPhase::FileRead {
            file_path,
            byte_count,
        } => format!("{base}:file-read path={file_path} bytes={byte_count}"),
        DependencyProgressPhase::FileMissing { file_path } => {
            format!("{base}:file-missing path={file_path}")
        }
        DependencyProgressPhase::JavaStart { file_path } => {
            format!("{base}:java-start path={file_path}")
        }
        DependencyProgressPhase::JavaDone {
            file_path,
            edge_count,
            elapsed_ms,
        } => {
            format!("{base}:java-done path={file_path} edges={edge_count} elapsed_ms={elapsed_ms}")
        }
        DependencyProgressPhase::JavaTimeout {
            file_path,
            edge_count,
            elapsed_ms,
        } => format!(
            "{base}:java-timeout path={file_path} partial_edges={edge_count} elapsed_ms={elapsed_ms}"
        ),
        DependencyProgressPhase::PythonStart { file_path } => {
            format!("{base}:python-start path={file_path}")
        }
        DependencyProgressPhase::PythonDone {
            file_path,
            edge_count,
            elapsed_ms,
        } => format!(
            "{base}:python-done path={file_path} edges={edge_count} elapsed_ms={elapsed_ms}"
        ),
        DependencyProgressPhase::PythonTimeout {
            file_path,
            edge_count,
            elapsed_ms,
        } => format!(
            "{base}:python-timeout path={file_path} partial_edges={edge_count} elapsed_ms={elapsed_ms}"
        ),
        DependencyProgressPhase::FileDone {
            file_path,
            edge_count,
            elapsed_ms,
        } => format!(
            "{base}:file-done path={file_path} total_edges={edge_count} elapsed_ms={elapsed_ms}"
        ),
    };
    (phase, progress.current, progress.total)
}

#[cfg(test)]
fn synthesize_graph(
    conn: &Connection,
    project: &str,
    repo: &Path,
) -> Result<SynthGraph, EngineError> {
    fn sink(_: &EngineEvent) {}
    synthesize_graph_with_progress(conn, project, repo, &mut sink)
}

fn has_native_label(conn: &Connection, project: &str, label: &str) -> Result<bool, EngineError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND label = ?2",
        [project, label],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[derive(Debug, Clone)]
struct NativeAppNode {
    id: String,
    name: String,
    file_path: String,
}

fn load_native_app_nodes(
    conn: &Connection,
    project: &str,
    label: &str,
) -> Result<Vec<NativeAppNode>, EngineError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, qualified_name, file_path, properties \
         FROM nodes WHERE project = ?1 AND label = ?2 ORDER BY id",
    )?;
    let mut rows = stmt.query([project, label])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let name = text_col(row, 1)?;
        let qn = text_col(row, 2)?;
        let file_path = text_col(row, 3)?;
        let props = parse_props(&text_col(row, 4)?);
        let name = props
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string();
        out.push(NativeAppNode {
            id: aka_node_id(cbm_id, &qn),
            name,
            file_path,
        });
    }
    Ok(out)
}

fn load_native_channel_topic_detections(
    conn: &Connection,
    project: &str,
    repo: &Path,
) -> Result<Vec<NativeTopicDetection>, EngineError> {
    let project_sources = ProjectSourceSet::discover(repo);
    let mut stmt = conn.prepare(
        "SELECT e.type, s.id, s.qualified_name, s.file_path, t.name, t.properties, e.properties \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id \
         JOIN nodes t ON t.id = e.target_id \
         WHERE e.project = ?1 \
           AND t.label = 'Channel' \
           AND e.type IN ('EMITS', 'LISTENS_ON') \
         ORDER BY e.id",
    )?;
    let mut rows = stmt.query([project])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let edge_type = text_col(row, 0)?;
        let source_id: i64 = row.get(1)?;
        let source_qn = text_col(row, 2)?;
        let file_path = text_col(row, 3)?;
        let channel_name = text_col(row, 4)?;
        if !project_sources.contains_project_file(repo, &file_path) {
            continue;
        }
        let channel_props = parse_props(&text_col(row, 5)?);
        let edge_props = parse_props(&text_col(row, 6)?);
        let topic = channel_props
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or(&channel_name)
            .to_string();
        if topic.is_empty() {
            continue;
        }
        let broker = channel_props
            .get("transport")
            .or_else(|| edge_props.get("transport"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("unknown")
            .to_string();
        let kind = match edge_type.as_str() {
            "EMITS" => TopicEndpointKind::Producer,
            "LISTENS_ON" => TopicEndpointKind::Consumer,
            _ => continue,
        };
        out.push(NativeTopicDetection {
            topic,
            broker,
            kind,
            node_id: aka_node_id(source_id, &source_qn),
            file_path,
            native_edge_type: edge_type,
        });
    }
    Ok(out)
}

fn load_synth_nodes(
    conn: &Connection,
    project: &str,
) -> Result<BTreeMap<String, SynthNode>, EngineError> {
    let mut nodes = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT id, label, name, qualified_name, file_path, start_line, end_line, properties \
         FROM nodes WHERE project = ?1 ORDER BY id",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let label = text_col(row, 1)?;
        let name = text_col(row, 2)?;
        let qn = text_col(row, 3)?;
        let file_path = text_col(row, 4)?;
        let start_line: i64 = row.get(5)?;
        let end_line: i64 = row.get(6)?;
        let props = parse_props(&text_col(row, 7)?);
        if !is_semantic_symbol_label(&label) || is_noisy_source_path(&file_path) {
            continue;
        }
        let aka_id = aka_node_id(cbm_id, &qn);
        let language = props
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let route_path = props
            .get("route_path")
            .or_else(|| props.get("routePath"))
            .and_then(Value::as_str)
            .map(normalize_route_literal);
        let route_method = props
            .get("route_method")
            .or_else(|| props.get("routeMethod"))
            .and_then(Value::as_str)
            .map(|v| v.to_ascii_uppercase());
        let decorators = props
            .get("decorators")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let parent_class = props
            .get("parent_class")
            .or_else(|| props.get("parentClass"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let ast_framework_multiplier = props
            .get("astFrameworkMultiplier")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let ast_framework_reason = props
            .get("astFrameworkReason")
            .and_then(Value::as_str)
            .map(str::to_string);
        let is_exported = props
            .get("isExported")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                props
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.eq_ignore_ascii_case("public"))
            });
        nodes.insert(
            aka_id.clone(),
            SynthNode {
                aka_id,
                qn,
                label,
                name,
                file_path,
                start_line,
                end_line,
                language,
                route_path,
                route_method,
                decorators,
                parent_class,
                is_exported,
                ast_framework_multiplier,
                ast_framework_reason,
            },
        );
    }
    Ok(nodes)
}

fn load_existing_node_ids(
    conn: &Connection,
    project: &str,
) -> Result<HashSet<String>, EngineError> {
    let mut ids = HashSet::new();
    let mut stmt = conn.prepare("SELECT id, qualified_name FROM nodes WHERE project = ?1")?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let qn = text_col(row, 1)?;
        ids.insert(aka_node_id(cbm_id, &qn));
    }
    Ok(ids)
}

fn count_process_symbol_basis(conn: &Connection, project: &str) -> Result<usize, EngineError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND label != 'File'",
        [project],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

#[derive(Debug, Clone, Default)]
struct CallGraph {
    adjacency: BTreeMap<String, BTreeSet<String>>,
    indegree: BTreeMap<String, usize>,
    edges: Vec<(String, String)>,
}

impl CallGraph {
    fn project_subgraph(&self, nodes: &BTreeMap<String, SynthNode>) -> Self {
        let mut graph = CallGraph::default();
        for (source, target) in &self.edges {
            if !nodes.contains_key(source) || !nodes.contains_key(target) || source == target {
                continue;
            }
            let inserted = graph
                .adjacency
                .entry(source.clone())
                .or_default()
                .insert(target.clone());
            if inserted {
                *graph.indegree.entry(target.clone()).or_default() += 1;
                graph.edges.push((source.clone(), target.clone()));
            }
        }
        graph
    }
}

fn load_call_graph(
    conn: &Connection,
    project: &str,
    nodes: &BTreeMap<String, SynthNode>,
    synthetic_edges: &[EdgeRec],
) -> Result<CallGraph, EngineError> {
    let mut graph = CallGraph::default();
    let mut stmt = conn.prepare(
        "SELECT e.source_id, e.target_id, e.properties, s.qualified_name, t.qualified_name \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id AND s.project = e.project \
         JOIN nodes t ON t.id = e.target_id AND t.project = e.project \
         WHERE e.project = ?1 \
           AND e.type = 'CALLS' \
           AND s.label IN ('Function','Method','Class','Interface','Struct','Enum','Trait','Type') \
           AND t.label IN ('Function','Method','Class','Interface','Struct','Enum','Trait','Type') \
           AND s.file_path != '' \
           AND t.file_path != '' \
         ORDER BY e.id",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let props = props_value(&text_col(row, 2)?);
        if props
            .get("confidence")
            .and_then(Value::as_f64)
            .is_some_and(|confidence| confidence < MIN_TRACE_CONFIDENCE)
        {
            continue;
        }
        let source_qn = text_col(row, 3)?;
        let target_qn = text_col(row, 4)?;
        let source = aka_node_id(source_id, &source_qn);
        let target = aka_node_id(target_id, &target_qn);
        if !nodes.contains_key(&source) || !nodes.contains_key(&target) || source == target {
            continue;
        }
        let inserted = graph
            .adjacency
            .entry(source.clone())
            .or_default()
            .insert(target.clone());
        if inserted {
            *graph.indegree.entry(target.clone()).or_default() += 1;
            graph.edges.push((source, target));
        }
    }
    for edge in synthetic_edges
        .iter()
        .filter(|edge| edge.edge_type == "CALLS")
    {
        if !nodes.contains_key(&edge.source_id)
            || !nodes.contains_key(&edge.target_id)
            || edge.source_id == edge.target_id
        {
            continue;
        }
        let inserted = graph
            .adjacency
            .entry(edge.source_id.clone())
            .or_default()
            .insert(edge.target_id.clone());
        if inserted {
            *graph.indegree.entry(edge.target_id.clone()).or_default() += 1;
            graph
                .edges
                .push((edge.source_id.clone(), edge.target_id.clone()));
        }
    }
    Ok(graph)
}

fn load_existing_call_pairs(
    conn: &Connection,
    project: &str,
) -> Result<HashSet<(String, String)>, EngineError> {
    let mut pairs = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT e.source_id, e.target_id, s.qualified_name, t.qualified_name \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id AND s.project = e.project \
         JOIN nodes t ON t.id = e.target_id AND t.project = e.project \
         WHERE e.project = ?1 AND e.type = 'CALLS'",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let source_qn = text_col(row, 2)?;
        let target_qn = text_col(row, 3)?;
        pairs.insert((
            aka_node_id(source_id, &source_qn),
            aka_node_id(target_id, &target_qn),
        ));
    }
    Ok(pairs)
}

fn load_native_community_memberships(
    conn: &Connection,
    project: &str,
    nodes: &BTreeMap<String, SynthNode>,
) -> Result<BTreeMap<String, Vec<CommunityRef>>, EngineError> {
    let mut memberships: BTreeMap<String, BTreeSet<CommunityRef>> = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT e.source_id, s.qualified_name, c.id, c.qualified_name, c.name, c.properties \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id AND s.project = e.project \
         JOIN nodes c ON c.id = e.target_id AND c.project = e.project \
         WHERE e.project = ?1 AND c.label = 'Community' AND e.type = 'MEMBER_OF' \
         ORDER BY e.id",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let source_id: i64 = row.get(0)?;
        let source_qn = text_col(row, 1)?;
        let community_id: i64 = row.get(2)?;
        let community_qn = text_col(row, 3)?;
        let community_name = text_col(row, 4)?;
        let community_props = parse_props(&text_col(row, 5)?);
        let source = aka_node_id(source_id, &source_qn);
        if !nodes.contains_key(&source) {
            continue;
        }
        let id = aka_node_id(community_id, &community_qn);
        let label = community_props
            .get("heuristicLabel")
            .and_then(Value::as_str)
            .or_else(|| community_props.get("label").and_then(Value::as_str))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                if community_name.is_empty() {
                    &community_qn
                } else {
                    &community_name
                }
            })
            .to_string();
        memberships
            .entry(source)
            .or_default()
            .insert(CommunityRef { id, label });
    }
    Ok(memberships
        .into_iter()
        .map(|(id, refs)| (id, refs.into_iter().collect()))
        .collect())
}

fn project_process_nodes(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    project_sources: &ProjectSourceSet,
) -> BTreeMap<String, SynthNode> {
    nodes
        .iter()
        .filter(|(_, node)| {
            if !is_process_step_label(&node.label) {
                return false;
            }
            let project_code = is_business_language(&node.language)
                || is_project_code_source_path(&node.file_path);
            node.file_path.is_empty()
                || !project_code
                || project_sources.contains_project_file(repo, &node.file_path)
        })
        .map(|(id, node)| (id.clone(), node.clone()))
        .collect()
}

fn synthesize_communities(
    nodes: &BTreeMap<String, SynthNode>,
    call_edges: &[(String, String)],
) -> Vec<SynthCommunity> {
    let labels = propagated_community_labels(nodes, call_edges);
    let mut groups: BTreeMap<String, Vec<SynthNode>> = BTreeMap::new();
    for node in nodes.values() {
        groups
            .entry(
                labels
                    .get(&node.aka_id)
                    .cloned()
                    .unwrap_or_else(|| community_key(&node.file_path)),
            )
            .or_default()
            .push(node.clone());
    }
    if groups.is_empty() || nodes.len() < MIN_SYNTH_COMMUNITY_SIZE {
        return Vec::new();
    }

    let mut node_group = BTreeMap::new();
    for (key, members) in &groups {
        for member in members {
            node_group.insert(member.aka_id.clone(), key.clone());
        }
    }

    let mut internal_calls: BTreeMap<String, usize> = BTreeMap::new();
    let mut incident_calls: BTreeMap<String, usize> = BTreeMap::new();
    for (source, target) in call_edges {
        let Some(source_group) = node_group.get(source) else {
            continue;
        };
        let Some(target_group) = node_group.get(target) else {
            continue;
        };
        if source_group == target_group {
            *internal_calls.entry(source_group.clone()).or_default() += 1;
            *incident_calls.entry(source_group.clone()).or_default() += 1;
        } else {
            *incident_calls.entry(source_group.clone()).or_default() += 1;
            *incident_calls.entry(target_group.clone()).or_default() += 1;
        }
    }

    groups
        .into_iter()
        .filter(|(_, members)| members.len() >= MIN_SYNTH_COMMUNITY_SIZE)
        .map(|(key, mut members)| {
            members.sort_by(|a, b| {
                a.file_path
                    .cmp(&b.file_path)
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.aka_id.cmp(&b.aka_id))
            });
            let incident = *incident_calls.get(&key).unwrap_or(&0);
            let internal = *internal_calls.get(&key).unwrap_or(&0);
            let cohesion = if incident == 0 {
                1.0
            } else {
                internal as f64 / incident as f64
            };
            let heuristic_label = community_label(&key, &members);
            SynthCommunity {
                id: format!("community:heuristic:{:016x}", stable_hash(&key)),
                heuristic_label,
                cohesion: round3(cohesion),
                members,
            }
        })
        .collect()
}

fn propagated_community_labels(
    nodes: &BTreeMap<String, SynthNode>,
    call_edges: &[(String, String)],
) -> BTreeMap<String, String> {
    let initial: BTreeMap<String, String> = nodes
        .values()
        .map(|node| (node.aka_id.clone(), community_key(&node.file_path)))
        .collect();
    if call_edges.is_empty() {
        return initial;
    }

    let mut initial_sizes: BTreeMap<String, usize> = BTreeMap::new();
    for label in initial.values() {
        *initial_sizes.entry(label.clone()).or_default() += 1;
    }

    let mut neighbors: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (source, target) in call_edges {
        if !initial.contains_key(source) || !initial.contains_key(target) || source == target {
            continue;
        }
        neighbors
            .entry(source.clone())
            .or_default()
            .insert(target.clone());
        neighbors
            .entry(target.clone())
            .or_default()
            .insert(source.clone());
    }

    let mut labels = initial;
    for _ in 0..COMMUNITY_LABEL_PROPAGATION_PASSES {
        let mut next = labels.clone();
        let mut changed = false;
        for (node_id, current_label) in &labels {
            let Some(node_neighbors) = neighbors.get(node_id) else {
                continue;
            };
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for neighbor in node_neighbors {
                if let Some(label) = labels.get(neighbor) {
                    *counts.entry(label.clone()).or_default() += 1;
                }
            }
            let Some((best_label, best_count)) = counts
                .iter()
                .max_by(|(label_a, count_a), (label_b, count_b)| {
                    count_a.cmp(count_b).then_with(|| label_b.cmp(label_a))
                })
                .map(|(label, count)| (label.clone(), *count))
            else {
                continue;
            };
            let own_count = counts.get(current_label).copied().unwrap_or(0);
            let own_initial_size = initial_sizes.get(current_label).copied().unwrap_or(0);
            let should_adopt = best_label != *current_label
                && best_count > own_count
                && (best_count >= 2 || own_initial_size < MIN_SYNTH_COMMUNITY_SIZE);
            if should_adopt {
                next.insert(node_id.clone(), best_label);
                changed = true;
            }
        }
        labels = next;
        if !changed {
            break;
        }
    }

    labels
}

fn community_memberships_from_synth(
    communities: &[SynthCommunity],
) -> BTreeMap<String, Vec<CommunityRef>> {
    let mut out = BTreeMap::new();
    for community in communities {
        let community_ref = CommunityRef {
            id: community.id.clone(),
            label: community.heuristic_label.clone(),
        };
        for member in &community.members {
            out.entry(member.aka_id.clone())
                .or_insert_with(Vec::new)
                .push(community_ref.clone());
        }
    }
    out
}

fn synthesize_processes_from_calls(
    nodes: &BTreeMap<String, SynthNode>,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    indegree: &BTreeMap<String, usize>,
    community_memberships: &BTreeMap<String, Vec<CommunityRef>>,
    command_entry_hints: &BTreeMap<String, CommandEntryHint>,
    symbol_count: usize,
) -> Vec<SynthProcess> {
    if adjacency.is_empty() {
        return Vec::new();
    }

    let max_processes = dynamic_process_cap(symbol_count);
    let mut starts = find_entry_points(nodes, adjacency, indegree, command_entry_hints);
    if starts.is_empty() {
        starts = fallback_entry_points(nodes, adjacency);
        starts.sort_by(|a, b| {
            let na = &nodes[a];
            let nb = &nodes[b];
            na.file_path
                .cmp(&nb.file_path)
                .then_with(|| na.name.cmp(&nb.name))
                .then_with(|| a.cmp(b))
        });
    }
    starts.truncate(PROCESS_MAX_STARTS);

    let mut traces = Vec::new();
    for start in starts {
        traces.extend(trace_from_entry_point(&start, nodes, adjacency));
        if traces.len() >= max_processes * 2 {
            break;
        }
    }
    let mut traces = deduplicate_by_endpoints(deduplicate_traces(traces));
    traces.sort_by_key(|trace| Reverse(trace.len()));
    traces.truncate(max_processes);

    traces
        .into_iter()
        .filter_map(|trace| {
            process_from_trace(&trace, nodes, community_memberships, command_entry_hints)
        })
        .collect()
}

fn find_entry_points(
    nodes: &BTreeMap<String, SynthNode>,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    indegree: &BTreeMap<String, usize>,
    command_entry_hints: &BTreeMap<String, CommandEntryHint>,
) -> Vec<String> {
    let mut candidates = Vec::new();
    for (id, node) in nodes {
        if !matches!(node.label.as_str(), "Function" | "Method") {
            continue;
        }
        let Some(callees) = adjacency.get(id) else {
            continue;
        };
        if callees.is_empty() {
            continue;
        }
        let callers = *indegree.get(id).unwrap_or(&0);
        let score = entry_score(node, callers, callees.len(), command_entry_hints.get(id));
        if score > 0.0 {
            candidates.push((id.clone(), score));
        }
    }
    candidates.sort_by(|(a_id, a_score), (b_id, b_score)| {
        b_score.total_cmp(a_score).then_with(|| {
            let a = &nodes[a_id];
            let b = &nodes[b_id];
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a_id.cmp(b_id))
        })
    });
    candidates.into_iter().map(|(id, _)| id).collect()
}

fn fallback_entry_points(
    nodes: &BTreeMap<String, SynthNode>,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<String> {
    adjacency
        .keys()
        .filter(|id| {
            nodes
                .get(*id)
                .is_some_and(allows_name_only_process_fallback)
        })
        .cloned()
        .collect()
}

fn allows_name_only_process_fallback(node: &SynthNode) -> bool {
    !is_jvm_business_node(node)
}

fn trace_from_entry_point(
    entry_id: &str,
    nodes: &BTreeMap<String, SynthNode>,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<Vec<String>> {
    let mut traces = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back((entry_id.to_string(), vec![entry_id.to_string()]));

    while let Some((current, path)) = queue.pop_front() {
        if traces.len() >= PROCESS_BRANCH_LIMIT * 3 {
            break;
        }
        let callees = adjacency.get(&current);
        if path.len() >= PROCESS_MAX_STEPS || callees.is_none_or(BTreeSet::is_empty) {
            if path.len() >= PROCESS_MIN_STEPS {
                traces.push(path);
            }
            continue;
        }

        let mut ranked: Vec<&String> = callees.expect("checked").iter().collect();
        ranked.sort_by(|a, b| {
            let na = &nodes[*a];
            let nb = &nodes[*b];
            step_score(nb)
                .cmp(&step_score(na))
                .then_with(|| na.file_path.cmp(&nb.file_path))
                .then_with(|| na.name.cmp(&nb.name))
                .then_with(|| a.cmp(b))
        });
        let mut advanced = false;
        for next in ranked.into_iter().take(PROCESS_BRANCH_LIMIT) {
            if path.iter().any(|id| id == next) {
                continue;
            }
            let mut next_path = path.clone();
            next_path.push(next.clone());
            queue.push_back((next.clone(), next_path));
            advanced = true;
        }
        if !advanced && path.len() >= PROCESS_MIN_STEPS {
            traces.push(path);
        }
    }

    traces
}

fn deduplicate_traces(mut traces: Vec<Vec<String>>) -> Vec<Vec<String>> {
    traces.sort_by_key(|trace| Reverse(trace.len()));
    let mut unique: Vec<Vec<String>> = Vec::new();
    for trace in traces {
        if !unique
            .iter()
            .any(|existing| contains_trace(existing, &trace))
        {
            unique.push(trace);
        }
    }
    unique
}

fn deduplicate_by_endpoints(mut traces: Vec<Vec<String>>) -> Vec<Vec<String>> {
    traces.sort_by_key(|trace| Reverse(trace.len()));
    let mut seen_endpoints: BTreeSet<(String, String)> = BTreeSet::new();
    let mut out = Vec::new();
    for trace in traces {
        let (Some(first), Some(last)) = (trace.first(), trace.last()) else {
            continue;
        };
        if seen_endpoints.insert((first.clone(), last.clone())) {
            out.push(trace);
        }
    }
    out
}

fn contains_trace(existing: &[String], candidate: &[String]) -> bool {
    candidate.len() <= existing.len()
        && existing
            .windows(candidate.len())
            .any(|window| window == candidate)
}

fn process_from_trace(
    path: &[String],
    nodes: &BTreeMap<String, SynthNode>,
    community_memberships: &BTreeMap<String, Vec<CommunityRef>>,
    command_entry_hints: &BTreeMap<String, CommandEntryHint>,
) -> Option<SynthProcess> {
    if path.len() < PROCESS_MIN_STEPS {
        return None;
    }
    let key = path.join(">");
    let steps: Vec<SynthNode> = path
        .iter()
        .filter_map(|id| nodes.get(id).cloned())
        .collect();
    if steps.len() < PROCESS_MIN_STEPS {
        return None;
    }
    let entry = steps.first().expect("steps").display_name();
    let terminal = steps.last().expect("steps").display_name();
    let id = format!("process:call-chain:{:016x}", stable_hash(&key));
    let communities = process_communities(path, community_memberships);
    let process_type = if communities.len() > 1 {
        "cross_community"
    } else {
        "intra_community"
    }
    .to_string();
    Some(SynthProcess {
        id,
        name: format!("{entry} → {terminal}"),
        process_type,
        communities,
        steps,
        entry_reason: path
            .first()
            .and_then(|id| command_entry_hints.get(id))
            .map(|hint| hint.strategy.clone()),
    })
}

fn process_communities(
    path: &[String],
    community_memberships: &BTreeMap<String, Vec<CommunityRef>>,
) -> Vec<CommunityRef> {
    let mut communities = BTreeSet::new();
    for node_id in path {
        if let Some(node_communities) = community_memberships.get(node_id) {
            communities.extend(node_communities.iter().cloned());
        }
    }
    communities.into_iter().collect()
}

fn synthesize_routes_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    processes: &[SynthProcess],
    native_routes: &[NativeAppNode],
) -> Vec<SynthRoute> {
    let mut progress = |_: String, _: u64, _: u64| {};
    synthesize_routes_from_sources_with_progress(
        repo,
        nodes,
        processes,
        native_routes,
        &mut progress,
    )
}

fn synthesize_routes_from_sources_with_progress(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    processes: &[SynthProcess],
    native_routes: &[NativeAppNode],
    progress: &mut dyn FnMut(String, u64, u64),
) -> Vec<SynthRoute> {
    let mut route_progress = RouteSynthesisProgress::new(progress);
    route_progress.emit_force("group-files", 0, 0);
    let by_file = route_nodes_by_file(nodes);
    let total_source_files = by_file.len() as u64;
    route_progress.emit_force("discover-project-files", 0, 0);
    let project_sources = ProjectSourceSet::discover(repo);
    route_progress.emit_force("prefixes:python", 0, 0);
    let python_prefixes = python_router_prefixes_by_file(repo, by_file.keys().map(String::as_str));
    route_progress.emit_force("interfaces:java", 0, 0);
    let java_interface_routes = java_interface_routes_by_method(repo, nodes);
    route_progress.emit_force("urlconf:django", 0, 0);
    let django_routes_by_file = django_urlconf_routes_from_repo(repo, &project_sources, &by_file);
    route_progress.emit_force("functional:spring", 0, 0);
    let spring_functional_routes_by_file =
        spring_functional_routes_from_repo(repo, &project_sources, nodes);
    route_progress.emit_force("realtime", 0, 0);
    let realtime_routes_by_file = realtime_routes_by_file(repo, &by_file, &python_prefixes);
    let mut routes: BTreeMap<(String, String, Option<String>), SynthRoute> = BTreeMap::new();
    route_progress.emit_force("native-routes", 0, native_routes.len() as u64);
    for native in native_routes {
        let route = route_from_path(&native.file_path)
            .unwrap_or_else(|| normalize_route_literal(trim_route_suffix(&native.name)));
        routes.insert(
            (route.clone(), native.file_path.clone(), None),
            SynthRoute {
                id: native.id.clone(),
                route,
                file_path: native.file_path.clone(),
                emit_node: false,
                method: None,
                handler_id: None,
                handler_name: None,
                middleware: Vec::new(),
                response_keys: Vec::new(),
                error_keys: Vec::new(),
                consumers: Vec::new(),
                process_ids: process_ids_for_entry(processes, &native.file_path, None),
            },
        );
    }
    route_progress.emit_force(
        "native-routes",
        native_routes.len() as u64,
        native_routes.len() as u64,
    );
    route_progress.emit_force("source-files", 0, total_source_files);
    for (idx, (file_path, file_nodes)) in by_file.iter().enumerate() {
        let Some(text) = read_repo_text(repo, file_path) else {
            route_progress.emit_file(
                "source-files",
                (idx + 1) as u64,
                total_source_files,
                file_path,
            );
            continue;
        };
        let handler = pick_handler_node(file_nodes);
        let mut route_candidates = Vec::new();
        if let Some(route) = route_from_path(file_path) {
            route_candidates.push(RouteCandidate {
                route,
                method: None,
                handler_id: handler.map(|n| n.aka_id.clone()),
                handler_name: handler.map(|n| n.display_name().to_string()),
            });
        }
        if is_js_ts_route_source(file_path, file_nodes) {
            route_candidates.extend(extract_route_handler_literals(&text).into_iter().map(
                |route| RouteCandidate {
                    route,
                    method: None,
                    handler_id: handler.map(|n| n.aka_id.clone()),
                    handler_name: handler.map(|n| n.display_name().to_string()),
                },
            ));
        }
        route_candidates.extend(extract_annotated_routes(
            &text,
            file_nodes,
            python_prefixes.get(file_path),
            &java_interface_routes,
        ));
        dedup_route_candidates(&mut route_candidates);
        if route_candidates.is_empty() {
            route_progress.emit_file(
                "source-files",
                (idx + 1) as u64,
                total_source_files,
                file_path,
            );
            continue;
        }
        let response_keys = extract_response_keys_for_file(repo, file_path, &text);
        let error_keys = extract_error_keys(&response_keys, &text);
        let middleware = extract_middleware(&text);
        for candidate in route_candidates {
            merge_route_candidate(
                &mut routes,
                processes,
                file_path,
                candidate,
                &middleware,
                &response_keys,
                &error_keys,
            );
        }
        route_progress.emit_file(
            "source-files",
            (idx + 1) as u64,
            total_source_files,
            file_path,
        );
    }

    let django_total = django_routes_by_file.len() as u64;
    route_progress.emit_force("merge:django", 0, django_total);
    for (idx, (file_path, route_candidates)) in django_routes_by_file.into_iter().enumerate() {
        let text = read_repo_text(repo, &file_path).unwrap_or_default();
        let fallback_response_keys = extract_response_keys_for_file(repo, &file_path, &text);
        let fallback_error_keys = extract_error_keys(&fallback_response_keys, &text);
        let middleware = extract_middleware(&text);
        for candidate in route_candidates {
            let (response_keys, error_keys) = response_keys_for_route_candidate(
                repo,
                nodes,
                &candidate,
                &fallback_response_keys,
                &fallback_error_keys,
            );
            merge_route_candidate(
                &mut routes,
                processes,
                &file_path,
                candidate,
                &middleware,
                &response_keys,
                &error_keys,
            );
        }
        route_progress.emit_file("merge:django", (idx + 1) as u64, django_total, &file_path);
    }

    let spring_total = spring_functional_routes_by_file.len() as u64;
    route_progress.emit_force("merge:spring-functional", 0, spring_total);
    for (idx, (file_path, route_candidates)) in
        spring_functional_routes_by_file.into_iter().enumerate()
    {
        let text = read_repo_text(repo, &file_path).unwrap_or_default();
        let response_keys = extract_response_keys_for_file(repo, &file_path, &text);
        let error_keys = extract_error_keys(&response_keys, &text);
        let middleware = extract_middleware(&text);
        for candidate in route_candidates {
            merge_route_candidate(
                &mut routes,
                processes,
                &file_path,
                candidate,
                &middleware,
                &response_keys,
                &error_keys,
            );
        }
        route_progress.emit_file(
            "merge:spring-functional",
            (idx + 1) as u64,
            spring_total,
            &file_path,
        );
    }

    let realtime_total = realtime_routes_by_file.len() as u64;
    route_progress.emit_force("merge:realtime", 0, realtime_total);
    for (idx, (file_path, route_candidates)) in realtime_routes_by_file.into_iter().enumerate() {
        let text = read_repo_text(repo, &file_path).unwrap_or_default();
        let response_keys = extract_response_keys_for_file(repo, &file_path, &text);
        let error_keys = extract_error_keys(&response_keys, &text);
        let middleware = extract_middleware(&text);
        for candidate in route_candidates {
            merge_route_candidate(
                &mut routes,
                processes,
                &file_path,
                candidate,
                &middleware,
                &response_keys,
                &error_keys,
            );
        }
        route_progress.emit_file(
            "merge:realtime",
            (idx + 1) as u64,
            realtime_total,
            &file_path,
        );
    }

    route_progress.emit_force("consumers:scan-files", 0, total_source_files);
    {
        let mut consumer_progress = |current, total| {
            route_progress.emit("consumers:scan-files", current, total);
        };
        attach_route_consumers_with_progress(repo, nodes, &mut routes, &mut consumer_progress);
    }
    route_progress.emit_force("consumers:dedupe", routes.len() as u64, routes.len() as u64);

    let mut out: Vec<SynthRoute> = routes.into_values().collect();
    route_progress.emit_force("sort", out.len() as u64, out.len() as u64);
    out.sort_by(|a, b| {
        a.route
            .cmp(&b.route)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    route_progress.emit_force("done", out.len() as u64, out.len() as u64);
    out
}

struct RouteSynthesisProgress<'a> {
    progress: &'a mut dyn FnMut(String, u64, u64),
    last_emit: Instant,
}

impl<'a> RouteSynthesisProgress<'a> {
    fn new(progress: &'a mut dyn FnMut(String, u64, u64)) -> Self {
        Self {
            progress,
            last_emit: Instant::now(),
        }
    }

    fn emit(&mut self, suffix: &str, current: u64, total: u64) {
        let enough_items = current == 0 || current == 1 || current.is_multiple_of(25);
        let enough_time = self.last_emit.elapsed() >= Duration::from_secs(1);
        let complete = total > 0 && current >= total;
        if enough_items || enough_time || complete {
            self.emit_force(suffix, current, total);
        }
    }

    fn emit_file(&mut self, suffix: &str, current: u64, total: u64, file_path: &str) {
        let enough_items = current == 0 || current == 1 || current.is_multiple_of(25);
        let enough_time = self.last_emit.elapsed() >= Duration::from_secs(1);
        let complete = total > 0 && current >= total;
        if enough_items || enough_time || complete {
            self.emit_force(&format!("{suffix} path={file_path}"), current, total);
        }
    }

    fn emit_force(&mut self, suffix: &str, current: u64, total: u64) {
        self.last_emit = Instant::now();
        (self.progress)(
            format!("aka-core:enrichment:routes:{suffix}"),
            current,
            total,
        );
    }
}

fn response_keys_for_route_candidate(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    candidate: &RouteCandidate,
    fallback_response_keys: &[String],
    fallback_error_keys: &[String],
) -> (Vec<String>, Vec<String>) {
    let Some(handler_id) = candidate.handler_id.as_deref() else {
        return (
            fallback_response_keys.to_vec(),
            fallback_error_keys.to_vec(),
        );
    };
    let Some(handler) = nodes.get(handler_id) else {
        return (
            fallback_response_keys.to_vec(),
            fallback_error_keys.to_vec(),
        );
    };
    if handler.file_path.is_empty() {
        return (
            fallback_response_keys.to_vec(),
            fallback_error_keys.to_vec(),
        );
    }
    let Some(text) = read_repo_text(repo, &handler.file_path) else {
        return (
            fallback_response_keys.to_vec(),
            fallback_error_keys.to_vec(),
        );
    };
    let response_keys = extract_response_keys_for_file(repo, &handler.file_path, &text);
    if response_keys.is_empty() {
        return (
            fallback_response_keys.to_vec(),
            fallback_error_keys.to_vec(),
        );
    }
    let error_keys = extract_error_keys(&response_keys, &text);
    (response_keys, error_keys)
}

fn merge_route_candidate(
    routes: &mut BTreeMap<(String, String, Option<String>), SynthRoute>,
    processes: &[SynthProcess],
    file_path: &str,
    candidate: RouteCandidate,
    middleware: &[String],
    response_keys: &[String],
    error_keys: &[String],
) {
    let route = candidate.route;
    let key = (
        route.clone(),
        file_path.to_string(),
        candidate.method.clone(),
    );
    match routes.get_mut(&key) {
        Some(existing) => {
            if existing.method.is_none() {
                existing.method = candidate.method.clone();
            }
            if existing.handler_id.is_none() {
                existing.handler_id = candidate.handler_id.clone();
                existing.handler_name = candidate.handler_name.clone();
            }
            merge_strings(&mut existing.middleware, middleware);
            merge_strings(&mut existing.response_keys, response_keys);
            merge_strings(&mut existing.error_keys, error_keys);
            merge_strings(
                &mut existing.process_ids,
                &process_ids_for_entry(processes, file_path, candidate.handler_id.as_deref()),
            );
        }
        None => {
            routes.insert(
                key,
                SynthRoute {
                    id: format!(
                        "route:heuristic:{:016x}",
                        stable_hash(&format!("{route}|{file_path}"))
                    ),
                    route,
                    file_path: file_path.to_string(),
                    emit_node: true,
                    method: candidate.method,
                    handler_id: candidate.handler_id.clone(),
                    handler_name: candidate.handler_name,
                    middleware: middleware.to_vec(),
                    response_keys: response_keys.to_vec(),
                    error_keys: error_keys.to_vec(),
                    consumers: Vec::new(),
                    process_ids: process_ids_for_entry(
                        processes,
                        file_path,
                        candidate.handler_id.as_deref(),
                    ),
                },
            );
        }
    }
}

fn string_literals(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some(byte) = text.as_bytes().get(idx).copied() else {
            break;
        };
        if matches!(byte, b'\'' | b'"' | b'`') {
            if let Some((literal, end)) = read_string_literal(text, idx) {
                if is_topic_literal(&literal) {
                    values.push(literal);
                }
                idx = end;
                continue;
            }
        }
        idx += 1;
    }
    values.sort();
    values.dedup();
    values
}

fn is_topic_literal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.starts_with('/')
        && !value.contains("://")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/'))
}

pub(super) fn web_nodes_by_file(
    nodes: &BTreeMap<String, SynthNode>,
) -> BTreeMap<String, Vec<&SynthNode>> {
    nodes_by_file(nodes)
        .into_iter()
        .filter(|(file_path, file_nodes)| {
            is_js_ts_source_path(file_path)
                || file_nodes
                    .iter()
                    .any(|node| is_js_ts_language(&node.language))
        })
        .collect()
}

pub(super) fn route_nodes_by_file(
    nodes: &BTreeMap<String, SynthNode>,
) -> BTreeMap<String, Vec<&SynthNode>> {
    nodes_by_file(nodes)
        .into_iter()
        .filter(|(file_path, file_nodes)| {
            is_web_backend_source_path(file_path)
                || file_nodes.iter().any(|node| {
                    is_web_backend_language(&node.language)
                        || node.route_path.is_some()
                        || node.decorators.iter().any(|decorator| {
                            decorator.contains("Mapping")
                                || decorator.contains("RestController")
                                || decorator.contains(".route")
                                || decorator.contains(".get")
                                || decorator.contains(".post")
                        })
                })
        })
        .collect()
}

fn is_js_ts_route_source(file_path: &str, file_nodes: &[&SynthNode]) -> bool {
    is_js_ts_source_path(file_path)
        || file_nodes
            .iter()
            .any(|node| is_js_ts_language(&node.language))
}

fn is_js_ts_source_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    matches!(
        Path::new(&lower).extension().and_then(|ext| ext.to_str()),
        Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts")
    )
}

fn is_web_backend_source_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    is_js_ts_source_path(&lower)
        || matches!(
            Path::new(&lower).extension().and_then(|ext| ext.to_str()),
            Some("java" | "kt" | "kts" | "scala" | "groovy" | "py")
        )
}

fn is_js_ts_language(language: &str) -> bool {
    matches!(
        language.to_ascii_lowercase().as_str(),
        "javascript" | "typescript" | "tsx" | "jsx"
    )
}

fn is_web_backend_language(language: &str) -> bool {
    matches!(
        language.to_ascii_lowercase().as_str(),
        "java"
            | "kotlin"
            | "scala"
            | "groovy"
            | "python"
            | "javascript"
            | "typescript"
            | "tsx"
            | "jsx"
    )
}

fn route_from_path(file_path: &str) -> Option<String> {
    let normalized = file_path.replace('\\', "/");
    let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if let Some(idx) = find_segment_pair(&segments, "pages", "api") {
        return route_from_segments(&segments[idx + 2..], true);
    }
    if let Some(idx) = find_segment_pair(&segments, "app", "api") {
        return route_from_segments(&segments[idx + 2..], true);
    }
    for marker in ["routes", "controllers", "handlers"] {
        if let Some(idx) = segments.iter().position(|s| s.eq_ignore_ascii_case(marker)) {
            if let Some(route) = route_from_segments(&segments[idx + 1..], true) {
                return Some(route);
            }
        }
    }
    None
}

fn find_segment_pair(segments: &[&str], a: &str, b: &str) -> Option<usize> {
    segments
        .windows(2)
        .position(|w| w[0].eq_ignore_ascii_case(a) && w[1].eq_ignore_ascii_case(b))
}

fn route_from_segments(segments: &[&str], api_prefix: bool) -> Option<String> {
    let mut parts = Vec::new();
    for segment in segments {
        let stem = file_stem_label(segment);
        if matches!(
            stem.as_str(),
            "route" | "index" | "page" | "layout" | "handler" | "controller"
        ) {
            continue;
        }
        let part = stem
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_start_matches("...")
            .trim_start_matches('$');
        if part.is_empty() {
            continue;
        }
        let part = if stem.starts_with('[') || stem.starts_with('$') {
            format!(":{part}")
        } else {
            part.to_string()
        };
        parts.push(part);
    }
    if parts.is_empty() && !api_prefix {
        return None;
    }
    let body = parts.join("/");
    if api_prefix {
        if body.is_empty() {
            Some("/api".into())
        } else {
            Some(format!("/api/{body}"))
        }
    } else if body.is_empty() {
        None
    } else {
        Some(format!("/{body}"))
    }
}

pub(super) fn spring_mapping_path(decorators: &[String]) -> Option<String> {
    for decorator in decorators {
        let name_end = decorator.find('(').unwrap_or(decorator.len());
        let name = decorator[..name_end].trim().trim_start_matches('@');
        if !is_spring_mapping_annotation(name) {
            continue;
        }
        if name_end == decorator.len() {
            return Some("/".into());
        }
        let args_start = name_end + 1;
        let args_end = decorator.rfind(')').unwrap_or(decorator.len());
        if args_start >= args_end {
            return Some("/".into());
        }
        let args = &decorator[args_start..args_end];
        if let Some(path) = first_route_literal(args) {
            return Some(normalize_route_literal(&path));
        }
        return Some("/".into());
    }
    None
}

fn is_spring_mapping_annotation(name: &str) -> bool {
    matches!(
        name.rsplit('.').next().unwrap_or(name),
        "RequestMapping"
            | "GetMapping"
            | "PostMapping"
            | "PutMapping"
            | "DeleteMapping"
            | "PatchMapping"
    )
}

pub(super) fn declarative_http_client_path(decorators: &[String]) -> Option<String> {
    for decorator in decorators {
        let Some(name_end) = decorator.find('(') else {
            continue;
        };
        let name = decorator[..name_end].trim_start_matches('@');
        if !matches!(
            name.rsplit('.').next().unwrap_or(name),
            "FeignClient" | "HttpExchange"
        ) {
            continue;
        }
        let args_start = name_end + 1;
        let args_end = decorator.rfind(')').unwrap_or(decorator.len());
        if args_start >= args_end {
            return Some("/".into());
        }
        let args = &decorator[args_start..args_end];
        let path = keyword_string_arg(args, "path")
            .or_else(|| keyword_string_arg(args, "url"))
            .or_else(|| first_route_literal(args));
        if let Some(path) = path {
            return Some(normalize_route_literal(&path));
        }
        return Some("/".into());
    }
    None
}

pub(super) fn declarative_http_method_path(decorators: &[String]) -> Option<String> {
    for decorator in decorators {
        let Some(name_end) = decorator.find('(') else {
            continue;
        };
        let name = decorator[..name_end].trim_start_matches('@');
        if !matches!(
            name.rsplit('.').next().unwrap_or(name),
            "HttpExchange"
                | "GetExchange"
                | "PostExchange"
                | "PutExchange"
                | "DeleteExchange"
                | "PatchExchange"
        ) {
            continue;
        }
        let args_start = name_end + 1;
        let args_end = decorator.rfind(')').unwrap_or(decorator.len());
        if args_start >= args_end {
            return Some("/".into());
        }
        let args = &decorator[args_start..args_end];
        let path = keyword_string_arg(args, "url")
            .or_else(|| keyword_string_arg(args, "path"))
            .or_else(|| first_route_literal(args));
        if let Some(path) = path {
            return Some(normalize_route_literal(&path));
        }
        return Some("/".into());
    }
    None
}

pub(super) fn request_line_path(decorator: &str) -> Option<String> {
    let name_end = decorator.find('(')?;
    let name = decorator[..name_end].trim_start_matches('@');
    if name.rsplit('.').next().unwrap_or(name) != "RequestLine" {
        return None;
    }
    let args_start = name_end + 1;
    let args_end = decorator.rfind(')').unwrap_or(decorator.len());
    if args_start >= args_end {
        return None;
    }
    let literal = first_route_literal(&decorator[args_start..args_end])?;
    parse_request_line_path(&literal)
}

fn parse_request_line_path(literal: &str) -> Option<String> {
    let mut parts = literal.split_whitespace();
    let method = parts.next()?;
    if !matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
    ) {
        return None;
    }
    let path = parts.next()?;
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    path.starts_with('/').then(|| normalize_route_literal(path))
}

pub(super) fn first_route_literal(text: &str) -> Option<String> {
    let mut i = 0usize;
    while i < text.len() {
        if let Some((literal, end)) = read_string_literal(text, i) {
            if literal.starts_with('/') || literal.starts_with('{') || !literal.contains('=') {
                return Some(literal);
            }
            i = end;
        } else {
            i += text[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        }
    }
    None
}

fn keyword_string_arg(args: &str, keyword: &str) -> Option<String> {
    let needle = format!("{keyword}=");
    let compact = args.replace(' ', "");
    let pos = compact.find(&needle)?;
    let start = pos + needle.len();
    read_string_literal(&compact, start).map(|(literal, _)| literal)
}

pub(super) fn join_route_paths(prefix: &str, suffix: &str) -> String {
    let prefix = normalize_route_literal(prefix);
    let suffix = normalize_route_literal(suffix);
    if prefix.is_empty() || prefix == "/" {
        return if suffix.is_empty() {
            "/".into()
        } else if suffix.starts_with('/') {
            suffix
        } else {
            format!("/{suffix}")
        };
    }
    if suffix.is_empty() || suffix == "/" {
        return prefix;
    }
    format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    )
}

fn extract_route_handler_literals(text: &str) -> BTreeSet<String> {
    let mut routes = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if !is_ident_start(bytes[i] as char) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        let word = &text[start..i];
        let lower = word.to_ascii_lowercase();
        if !matches!(
            lower.as_str(),
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "all" | "route"
        ) {
            continue;
        }
        let mut j = skip_ws(text, i);
        if j < bytes.len() && bytes[j] == b'(' {
            j = skip_ws(text, j + 1);
            if let Some((literal, _end)) = read_string_literal(text, j) {
                if literal.starts_with('/') && !literal.starts_with("//") {
                    routes.insert(normalize_route_literal(&literal));
                }
            }
        }
    }
    routes
}

pub(super) fn clamp_char_boundary(text: &str, idx: usize) -> usize {
    let mut idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

pub(super) fn process_ids_for_entry(
    processes: &[SynthProcess],
    file_path: &str,
    handler_id: Option<&str>,
) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for process in processes {
        let touches_handler =
            handler_id.is_some_and(|id| process.steps.iter().any(|step| step.aka_id == id));
        let touches_file = process.steps.iter().any(|step| step.file_path == file_path);
        if touches_handler || touches_file {
            ids.insert(process.id.clone());
        }
    }
    ids.into_iter().collect()
}

fn fetch_reason(keys: &[String], fetch_count: u32) -> String {
    let mut out = String::from("fetch-url-match");
    if !keys.is_empty() {
        out.push_str("|keys:");
        out.push_str(&keys.join(","));
    }
    out.push_str("|fetches:");
    out.push_str(&fetch_count.max(1).to_string());
    out
}

pub(super) fn normalize_route_literal(route: &str) -> String {
    let mut out = normalize_flask_route_params(route.trim());
    if out.len() > 1 {
        while out.ends_with('/') {
            out.pop();
        }
    }
    out
}

fn normalize_flask_route_params(route: &str) -> String {
    let mut out = String::with_capacity(route.len());
    let mut rest = route;
    while let Some(open) = rest.find('<') {
        let (before, after_open) = rest.split_at(open);
        out.push_str(before);
        let after_open = &after_open[1..];
        let Some(close) = after_open.find('>') else {
            out.push('<');
            out.push_str(after_open);
            return out;
        };
        let raw_param = &after_open[..close];
        let param = raw_param
            .rsplit_once(':')
            .map(|(_, name)| name)
            .unwrap_or(raw_param)
            .trim();
        if param.is_empty() {
            out.push('<');
            out.push_str(raw_param);
            out.push('>');
        } else {
            out.push('{');
            out.push_str(param);
            out.push('}');
        }
        rest = &after_open[close + 1..];
    }
    out.push_str(rest);
    out
}

fn trim_route_suffix(route: &str) -> &str {
    route
        .strip_suffix("/route")
        .or_else(|| route.strip_suffix("/index"))
        .unwrap_or(route)
}

pub(super) fn merge_strings(target: &mut Vec<String>, source: &[String]) {
    target.extend(source.iter().cloned());
    target.sort();
    target.dedup();
}

pub(super) fn property_name_offsets(text: &str, name: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = text[search_from..].find(name) {
        let i = search_from + rel;
        let before = i
            .checked_sub(1)
            .and_then(|idx| text.as_bytes().get(idx))
            .copied()
            .map(char::from);
        let after = text.as_bytes().get(i + name.len()).copied().map(char::from);
        if before.is_none_or(|ch| !is_ident_continue(ch))
            && after.is_none_or(|ch| !is_ident_continue(ch))
        {
            out.push(i);
        }
        search_from = i + name.len();
    }
    out
}

pub(super) fn read_string_literal(text: &str, start: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let quote = *bytes.get(start)?;
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    if matches!(quote, b'\'' | b'"')
        && bytes.get(start + 1) == Some(&quote)
        && bytes.get(start + 2) == Some(&quote)
    {
        return read_triple_quoted_string_literal(text, start, quote);
    }
    let mut out = String::new();
    let mut escape = false;
    let mut i = start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            let ch = text[i..].chars().next()?;
            out.push(ch);
            escape = false;
            i += ch.len_utf8();
            continue;
        }
        if b == b'\\' {
            escape = true;
        } else if b == quote {
            return Some((out, i + 1));
        } else {
            let ch = text[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
    }
    None
}

fn read_triple_quoted_string_literal(
    text: &str,
    start: usize,
    quote: u8,
) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut escape = false;
    let mut i = start + 3;
    while i < bytes.len() {
        if !escape
            && bytes.get(i) == Some(&quote)
            && bytes.get(i + 1) == Some(&quote)
            && bytes.get(i + 2) == Some(&quote)
        {
            return Some((out, i + 3));
        }
        let ch = text[i..].chars().next()?;
        if escape {
            out.push(ch);
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else {
            out.push(ch);
        }
        i += ch.len_utf8();
    }
    None
}

pub(super) fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || matches!(ch, '_' | '$')
}

impl SynthNode {
    fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.aka_id
        } else {
            &self.name
        }
    }

    fn start_line_key(&self) -> i64 {
        self.start_line
    }

    fn end_line_key(&self) -> i64 {
        self.end_line
    }
}

fn dynamic_process_cap(symbol_count: usize) -> usize {
    (symbol_count / 10)
        .clamp(PROCESS_MIN_COUNT, PROCESS_MAX_COUNT)
        .max(PROCESS_MIN_COUNT)
}

fn entry_score(
    node: &SynthNode,
    caller_count: usize,
    callee_count: usize,
    command_entry_hint: Option<&CommandEntryHint>,
) -> f64 {
    if callee_count == 0 {
        return 0.0;
    }
    if is_jvm_business_node(node)
        && !has_jvm_source_entry_fact(node, command_entry_hint)
        && !node.name.eq_ignore_ascii_case("main")
    {
        return 0.0;
    }
    let base_score = callee_count as f64 / (caller_count as f64 + 1.0);
    let export_multiplier = if node.is_exported { 2.0 } else { 1.0 };
    let name_multiplier = if is_utility_name(&node.name) {
        0.3
    } else if is_name_entry_candidate(node) {
        1.5
    } else {
        1.0
    };
    let framework_multiplier = framework_multiplier_from_path(&node.file_path);
    let ast_multiplier = if node.ast_framework_reason.is_some() {
        node.ast_framework_multiplier.max(1.0)
    } else {
        node.ast_framework_multiplier
    };
    let source_entry_multiplier = if command_entry_hint.is_some() {
        3.0
    } else {
        1.0
    };
    base_score
        * export_multiplier
        * name_multiplier
        * framework_multiplier
        * ast_multiplier
        * source_entry_multiplier
}

fn has_jvm_source_entry_fact(
    node: &SynthNode,
    command_entry_hint: Option<&CommandEntryHint>,
) -> bool {
    command_entry_hint.is_some()
        || node.route_path.is_some()
        || node.route_method.is_some()
        || node.ast_framework_reason.is_some()
        || node
            .decorators
            .iter()
            .any(|decorator| jvm_entry_decorator_name(decorator).is_some())
}

fn jvm_entry_decorator_name(decorator: &str) -> Option<&str> {
    let trimmed = decorator.trim().trim_start_matches('@');
    let end = trimmed
        .find('(')
        .or_else(|| trimmed.find(char::is_whitespace))
        .unwrap_or(trimmed.len());
    let simple = trimmed[..end].rsplit('.').next().unwrap_or(trimmed);
    matches!(
        simple,
        "GetMapping"
            | "PostMapping"
            | "PutMapping"
            | "DeleteMapping"
            | "PatchMapping"
            | "RequestMapping"
            | "MessageMapping"
            | "SubscribeMapping"
            | "KafkaListener"
            | "KafkaHandler"
            | "RabbitListener"
            | "JmsListener"
            | "SqsListener"
            | "Scheduled"
            | "Async"
            | "EventListener"
            | "TransactionalEventListener"
            | "QueryMapping"
            | "MutationMapping"
            | "SchemaMapping"
            | "BatchMapping"
    )
    .then_some(simple)
}

fn is_hard_entry_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "main" | "start" | "init" | "bootstrap"
    )
}

fn is_entry_name(name: &str, language: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    is_hard_entry_name(&lower)
        || lower.starts_with("handle")
        || lower.starts_with("process")
        || lower.starts_with("execute")
        || lower.starts_with("perform")
        || lower.starts_with("dispatch")
        || lower.starts_with("trigger")
        || lower.starts_with("fire")
        || lower.starts_with("emit")
        || lower.starts_with("on")
        || lower.ends_with("handler")
        || lower.ends_with("controller")
        || language_entry_name(&lower, language)
}

fn language_entry_name(lower_name: &str, language: &str) -> bool {
    match language {
        "python" => lower_name == "__main__" || lower_name.starts_with("view_"),
        "go" => lower_name == "init" || lower_name == "servehttp",
        "java" | "kotlin" | "csharp" => lower_name == "main",
        "rust" => lower_name == "main" || lower_name.starts_with("run_"),
        _ => false,
    }
}

fn is_name_entry_candidate(node: &SynthNode) -> bool {
    let lower = node.name.to_ascii_lowercase();
    if is_jvm_business_node(node) {
        return lower == "main";
    }
    is_entry_name(&node.name, &node.language)
}

fn is_jvm_business_node(node: &SynthNode) -> bool {
    let language = node.language.to_ascii_lowercase();
    matches!(
        language.as_str(),
        "java" | "kotlin" | "scala" | "groovy" | "csharp"
    ) || matches!(
        std::path::Path::new(&node.file_path.to_ascii_lowercase())
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("java" | "kt" | "kts" | "scala" | "groovy" | "cs")
    )
}

fn is_utility_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with('_')
        || lower.starts_with("get")
        || lower.starts_with("set")
        || lower.starts_with("is")
        || lower.starts_with("has")
        || lower.starts_with("can")
        || lower.starts_with("should")
        || lower.starts_with("format")
        || lower.starts_with("parse")
        || lower.starts_with("validate")
        || lower.starts_with("convert")
        || lower.starts_with("transform")
        || lower.starts_with("to")
        || lower.starts_with("from")
        || lower.starts_with("encode")
        || lower.starts_with("decode")
        || lower.starts_with("serialize")
        || lower.starts_with("deserialize")
        || lower.starts_with("clone")
        || lower.starts_with("copy")
        || lower.starts_with("merge")
        || lower.starts_with("filter")
        || lower.starts_with("map")
        || lower.starts_with("reduce")
        || matches!(
            lower.as_str(),
            "log" | "debug" | "error" | "warn" | "info" | "utils" | "helpers"
        )
        || lower.ends_with("helper")
        || lower.ends_with("util")
        || lower.ends_with("utils")
}

fn framework_multiplier_from_path(path: &str) -> f64 {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    if p.contains("/pages/api/")
        || p.contains("/app/api/")
        || p.contains("/routes/")
        || p.contains("/controllers/")
        || p.contains("/handlers/")
        || p.contains("/views/")
        || p.ends_with("controller.ts")
        || p.ends_with("controller.js")
        || p.ends_with("controller.py")
        || p.ends_with("handler.ts")
        || p.ends_with("handler.js")
        || p.ends_with("handler.py")
    {
        2.0
    } else if is_utility_file(&p) {
        0.6
    } else {
        1.0
    }
}

fn is_utility_file(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    p.contains("/utils/")
        || p.contains("/util/")
        || p.contains("/helpers/")
        || p.contains("/helper/")
        || p.contains("/common/")
        || p.contains("/shared/")
        || p.ends_with("/utils.ts")
        || p.ends_with("/utils.js")
        || p.ends_with("/helpers.ts")
        || p.ends_with("/helpers.js")
        || p.ends_with("_utils.py")
        || p.ends_with("_helpers.py")
}

fn step_score(node: &SynthNode) -> i32 {
    match node.label.as_str() {
        "Function" | "Method" => 30,
        "Class" | "Interface" | "Struct" | "Trait" => 15,
        _ => 0,
    }
}

fn is_process_step_label(label: &str) -> bool {
    matches!(
        label,
        "Function" | "Method" | "Class" | "Interface" | "Struct" | "Enum" | "Trait" | "Type"
    )
}

fn is_semantic_symbol_label(label: &str) -> bool {
    is_process_step_label(label) || matches!(label, "Field" | "Variable" | "Property")
}

fn community_key(file_path: &str) -> String {
    let path = file_path.replace('\\', "/");
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    match segments.as_slice() {
        [] => "(unknown)".into(),
        [only] => file_stem_label(only),
        [root, file] if looks_like_file(file) => (*root).to_string(),
        [root, name, ..]
            if matches!(
                *root,
                "apps" | "crates" | "libs" | "packages" | "services" | "tools"
            ) =>
        {
            format!("{root}/{name}")
        }
        [root, name, ..]
            if matches!(
                *root,
                "app" | "cmd" | "internal" | "lib" | "pkg" | "src" | "test" | "tests"
            ) && !looks_like_file(name) =>
        {
            format!("{root}/{name}")
        }
        [root, ..] => (*root).to_string(),
    }
}

fn looks_like_file(segment: &str) -> bool {
    segment
        .rsplit_once('.')
        .is_some_and(|(_, ext)| !ext.is_empty())
}

fn community_label(key: &str, members: &[SynthNode]) -> String {
    let folder = key
        .split('/')
        .rfind(|part| !part.is_empty() && *part != "(unknown)");
    if let Some(folder) = folder {
        return capitalize(folder);
    }

    let names: Vec<&str> = members
        .iter()
        .map(|node| node.name.as_str())
        .filter(|name| !name.is_empty())
        .collect();
    if names.len() > 2 {
        let prefix = common_prefix(&names);
        if prefix.len() > 2 {
            return capitalize(&prefix);
        }
    }
    "Cluster".into()
}

fn community_keywords(members: &[SynthNode]) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for member in members {
        for token in keyword_tokens(&member.name) {
            *counts.entry(token).or_default() += 2;
        }
        for segment in member
            .file_path
            .replace('\\', "/")
            .split('/')
            .filter(|segment| !segment.is_empty())
        {
            if looks_like_file(segment) {
                continue;
            }
            for token in keyword_tokens(segment) {
                *counts.entry(token).or_default() += 1;
            }
        }
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|(a_token, a_count), (b_token, b_count)| {
        b_count
            .cmp(a_count)
            .then_with(|| a_token.len().cmp(&b_token.len()))
            .then_with(|| a_token.cmp(b_token))
    });
    ranked.into_iter().take(8).map(|(token, _)| token).collect()
}

fn keyword_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .flat_map(split_identifier)
        .filter(|token| {
            token.len() >= 3
                && !matches!(
                    token.as_str(),
                    "src" | "lib" | "core" | "utils" | "common" | "shared" | "test" | "tests"
                )
        })
        .collect()
}

fn split_identifier(raw: &str) -> Vec<String> {
    let raw = raw.trim_matches('_');
    if raw.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut start = 0usize;
    let chars: Vec<(usize, char)> = raw.char_indices().collect();
    for i in 1..chars.len() {
        let prev = chars[i - 1].1;
        let current = chars[i].1;
        let next = chars.get(i + 1).map(|(_, ch)| *ch);
        let boundary = (prev.is_ascii_lowercase() && current.is_ascii_uppercase())
            || (prev.is_ascii_uppercase()
                && current.is_ascii_uppercase()
                && next.is_some_and(|ch| ch.is_ascii_lowercase()))
            || (prev.is_ascii_alphabetic() && current.is_ascii_digit())
            || (prev.is_ascii_digit() && current.is_ascii_alphabetic());
        if boundary {
            let off = chars[i].0;
            parts.push(raw[start..off].to_ascii_lowercase());
            start = off;
        }
    }
    parts.push(raw[start..].to_ascii_lowercase());
    parts
}

fn file_stem_label(file_name: &str) -> String {
    file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .filter(|stem| !stem.is_empty())
        .unwrap_or(file_name)
        .to_string()
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn common_prefix(strings: &[&str]) -> String {
    let Some(first) = strings.iter().min() else {
        return String::new();
    };
    let Some(last) = strings.iter().max() else {
        return String::new();
    };
    first
        .chars()
        .zip(last.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a)
        .collect()
}

fn round3(value: f64) -> f64 {
    (value.clamp(0.0, 1.0) * 1000.0).round() / 1000.0
}

fn export_chunks(
    conn: &Connection,
    project: &str,
    repo: &Path,
    path: &Path,
    total: u64,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<u64, EngineError> {
    let file = File::create(path)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let chunks = collect_chunks(conn, project, repo, total, on_event)?;
    for chunk in &chunks {
        serde_json::to_writer(&mut out, chunk)?;
        out.write_all(b"\n")?;
    }
    Ok(chunks.len() as u64)
}

fn collect_chunks(
    conn: &Connection,
    project: &str,
    repo: &Path,
    total: u64,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<Vec<ChunkRec>, EngineError> {
    emit_phase(on_event, "aka-engine:facts:chunks:query", 0, total);
    let mut stmt = conn.prepare(
        "SELECT id, qualified_name, label, file_path, start_line, end_line \
         FROM nodes \
         WHERE project = ?1 AND file_path != '' AND label NOT IN ('File','Folder','Project','Package','Module') \
         ORDER BY id",
    )?;
    let mut rows = stmt.query([project])?;
    let mut count = 0;
    let mut chunks = Vec::new();
    let mut sources = SourceCache::new(repo);
    let mut progress = ExportProgress::new("chunks", total);
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let qn = text_col(row, 1)?;
        let label = text_col(row, 2)?;
        let file_path = text_col(row, 3)?;
        let start_line: i64 = row.get(4)?;
        let end_line: i64 = row.get(5)?;
        let text = sources
            .read_line_span(&file_path, start_line, end_line)
            .unwrap_or_default();
        chunks.push(ChunkRec {
            node_id: aka_node_id(cbm_id, &qn),
            kind: format!("ast-{}", label.to_ascii_lowercase()),
            file_path,
            start_line: to_artifact_line(start_line),
            end_line: to_artifact_line(end_line),
            text,
        });
        count += 1;
        progress.emit(on_event, count);
    }
    progress.emit_force(on_event, count);
    Ok(chunks)
}

struct SourceCache<'a> {
    repo: &'a Path,
    missing: BTreeSet<String>,
    files: BTreeMap<String, Vec<String>>,
}

impl<'a> SourceCache<'a> {
    fn new(repo: &'a Path) -> Self {
        Self {
            repo,
            missing: BTreeSet::new(),
            files: BTreeMap::new(),
        }
    }

    fn read_line_span(
        &mut self,
        file_path: &str,
        start_line: i64,
        end_line: i64,
    ) -> Option<String> {
        if self.missing.contains(file_path) {
            return None;
        }
        if !self.files.contains_key(file_path) {
            let path = self.repo.join(file_path);
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    self.files.insert(
                        file_path.to_string(),
                        text.lines().map(str::to_string).collect(),
                    );
                }
                Err(_) => {
                    self.missing.insert(file_path.to_string());
                    return None;
                }
            }
        }
        let lines = self.files.get(file_path)?;
        let start = start_line.max(1) as usize;
        let end = end_line.max(start_line).max(1) as usize;
        let from = start.saturating_sub(1).min(lines.len());
        let to = end.min(lines.len());
        if from >= to {
            return None;
        }
        Some(lines[from..to].join("\n"))
    }

    fn read_file(&mut self, file_path: &str) -> Option<String> {
        if self.missing.contains(file_path) {
            return None;
        }
        if !self.files.contains_key(file_path) {
            let path = self.repo.join(file_path);
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    self.files.insert(
                        file_path.to_string(),
                        text.lines().map(str::to_string).collect(),
                    );
                }
                Err(_) => {
                    self.missing.insert(file_path.to_string());
                    return None;
                }
            }
        }
        self.files.get(file_path).map(|lines| lines.join("\n"))
    }
}

fn parse_props(text: &str) -> Map<String, Value> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

fn text_col(row: &Row<'_>, idx: usize) -> Result<String, rusqlite::Error> {
    match row.get_ref(idx)? {
        ValueRef::Null => Ok(String::new()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            Ok(String::from_utf8_lossy(bytes).into_owned())
        }
        ValueRef::Integer(v) => Ok(v.to_string()),
        ValueRef::Real(v) => Ok(v.to_string()),
    }
}

fn props_value(text: &str) -> Value {
    serde_json::from_str::<Value>(text).unwrap_or(Value::Null)
}

fn insert_if_missing(props: &mut Map<String, Value>, key: &str, value: Value) {
    props.entry(key.to_string()).or_insert(value);
}

fn sanitize_string_array_prop(props: &mut Map<String, Value>, key: &str) {
    let Some(values) = props.get(key).and_then(Value::as_array) else {
        return;
    };
    let mut out: Vec<Value> = values
        .iter()
        .filter_map(Value::as_str)
        .map(|s| s.trim_matches(['"', '\'']).to_string())
        .filter(|s| !s.is_empty() && s != "null" && s != "undefined")
        .map(Value::String)
        .collect();
    out.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    out.dedup();
    props.insert(key.to_string(), Value::Array(out));
}

fn to_artifact_line(line_1based: i64) -> u32 {
    if line_1based <= 0 {
        0
    } else {
        (line_1based - 1) as u32
    }
}

fn aka_node_id(cbm_id: i64, qn: &str) -> String {
    let mut out = String::with_capacity(qn.len() + 24);
    out.push_str("cbm:");
    out.push_str(&cbm_id.to_string());
    out.push(':');
    for ch in qn.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn git_head(repo: &Path) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).arg("rev-parse").arg("HEAD");
    hide_child_console(&mut cmd);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

fn wait_for_done_exit(
    child: &mut std::process::Child,
    grace: Duration,
) -> Result<ExitStatus, std::io::Error> {
    let deadline = Instant::now() + grace;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return child.wait();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests;
