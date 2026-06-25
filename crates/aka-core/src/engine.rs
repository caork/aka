//! Engine runner backed by the embedded AKA engine direct-facts API.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aka_facts::{FactBatch, FactBatchBuilder, FactStats};

use crate::settings::{clamp_index_max_secs, AkaSettings, DEFAULT_INDEX_MAX_SECS};
use crate::types::{ChunkRec, EdgeRec, EngineEvent, NodeRec};

#[cfg(feature = "embedded-engine")]
mod embedded;
mod fact_producer;
#[cfg(feature = "embedded-engine")]
use embedded::EmbeddedEngineFactProducer;
use fact_producer::{EngineFactOptions, ProducedEngineFacts};

#[cfg_attr(not(feature = "embedded-engine"), allow(dead_code))]
const DEFAULT_ENGINE_MODE: &str = "fast";

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("embedded AKA engine runtime not found: {0} (set --engine-dir, AKA_ENGINE_DIR, AKA_ENGINE_LIB_DIR, or AKA_ENGINE_DLL)")]
    EngineDirMissing(PathBuf),
    #[error("engine exited with {code:?}; stderr tail:\n{stderr_tail}")]
    Failed {
        code: Option<i32>,
        stderr_tail: String,
    },
    #[error("engine io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("engine json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("engine facts error: {0}")]
    Facts(#[from] aka_facts::FactSourceError),
    #[error("indexing timed out after {elapsed_secs}s during {stage}")]
    Timeout { stage: String, elapsed_secs: u64 },
}

/// Native AKA engine runner.
pub struct EngineRunner {
    engine_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AnalyzeFactsOptions<'a> {
    pub cache_dir: Option<&'a Path>,
    pub no_chunks: bool,
    pub deadline: Option<IndexingDeadline>,
}

#[derive(Debug, Clone, Copy)]
pub struct IndexingDeadline {
    started_at: Instant,
    max_duration: Duration,
}

impl IndexingDeadline {
    pub fn new(max_duration: Duration) -> Self {
        Self {
            started_at: Instant::now(),
            max_duration,
        }
    }

    pub fn from_env() -> Self {
        Self::new(index_max_duration())
    }

    pub fn is_expired(self) -> bool {
        self.started_at.elapsed() >= self.max_duration
    }

    pub fn remaining(self) -> Duration {
        self.max_duration
            .checked_sub(self.started_at.elapsed())
            .unwrap_or_default()
    }

    pub fn elapsed_secs(self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub fn max_secs(self) -> u64 {
        self.max_duration.as_secs()
    }

    pub fn check(self, stage: impl Into<String>) -> Result<(), EngineError> {
        if self.is_expired() {
            Err(EngineError::Timeout {
                stage: stage.into(),
                elapsed_secs: self.elapsed_secs(),
            })
        } else {
            Ok(())
        }
    }
}

/// Replayable facts emitted by the embedded engine.
pub enum EngineFacts {
    DirectBatch(FactBatch),
}

impl EngineFacts {
    pub fn stats(&self) -> &FactStats {
        match self {
            Self::DirectBatch(batch) => &batch.stats,
        }
    }

    pub fn transport_name(&self) -> &'static str {
        match self {
            Self::DirectBatch(_) => "engine-direct-facts",
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
        }
    }
}

impl EngineRunner {
    /// `engine_dir` may be a directory containing embedded engine resources
    /// such as `aka_engine.dll` / `libaka_engine.a` / `ENGINE_SHA`.
    pub fn new(engine_dir: impl Into<PathBuf>) -> Result<Self, EngineError> {
        let requested = engine_dir.into();
        if !embedded_runtime_available(&requested) {
            return Err(EngineError::EngineDirMissing(requested.clone()));
        }
        let engine_dir = if requested.is_file() {
            requested
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            requested
        };
        Ok(Self { engine_dir })
    }

    pub fn dir(&self) -> &Path {
        &self.engine_dir
    }

    /// Discover embedded AKA engine resources from explicit path, env, local
    /// engine checkout, or fall back to the linked embedded build.
    pub fn discover(explicit: Option<&Path>) -> Result<Self, EngineError> {
        if let Some(dir) = explicit {
            return Self::new(dir);
        }
        if let Ok(env_dir) = std::env::var("AKA_ENGINE_DIR") {
            return Self::new(PathBuf::from(env_dir));
        }
        if let Ok(env_dir) = std::env::var("AKA_ENGINE_LIB_DIR") {
            return Self::new(PathBuf::from(env_dir));
        }
        if let Ok(dll) = std::env::var("AKA_ENGINE_DLL") {
            return Self::new(PathBuf::from(dll));
        }

        let mut candidates: Vec<PathBuf> = vec![
            PathBuf::from("engine"),
            PathBuf::from("engine/aka-engine-src/build/c"),
            PathBuf::from("/tmp/aka-engine-src"),
            PathBuf::from("/tmp/aka-engine-src/build/c"),
        ];
        if let Ok(cwd) = std::env::current_dir() {
            for ancestor in cwd.ancestors() {
                candidates.extend([
                    ancestor.join("engine"),
                    ancestor
                        .join("engine")
                        .join("aka-engine-src")
                        .join("build")
                        .join("c"),
                ]);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            candidates.extend(exe.ancestors().skip(1).map(|p| p.join("engine")));
        }
        for c in &candidates {
            if embedded_runtime_available(c) {
                return Self::new(c.clone());
            }
        }

        #[cfg(all(feature = "embedded-engine", not(windows)))]
        {
            Ok(Self {
                engine_dir: std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
            })
        }

        #[cfg(any(not(feature = "embedded-engine"), windows))]
        {
            Err(EngineError::EngineDirMissing(
                candidates.last().cloned().unwrap_or_default(),
            ))
        }
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
        let fact_options = EngineFactOptions {
            cache_dir: options.cache_dir,
            no_chunks: options.no_chunks,
            deadline: options.deadline,
        };
        let mut sink = FactBatchBuilder::new();
        let ProducedEngineFacts::DirectFacts =
            self.produce_engine_facts(repo, fact_options, &mut sink, &mut on_event)?;
        let batch = sink.finish();
        let done = EngineEvent::Done {
            stats: batch.stats.clone(),
        };
        on_event(&done);
        Ok(EngineFacts::DirectBatch(batch))
    }

    fn produce_engine_facts(
        &self,
        repo: &Path,
        options: EngineFactOptions<'_>,
        sink: &mut FactBatchBuilder,
        on_event: &mut dyn FnMut(&EngineEvent),
    ) -> Result<ProducedEngineFacts, EngineError> {
        let _ = self;
        #[cfg(feature = "embedded-engine")]
        {
            EmbeddedEngineFactProducer.produce(repo, options, sink, on_event)
        }
        #[cfg(not(feature = "embedded-engine"))]
        {
            let _ = (repo, options, sink, on_event);
            Err(EngineError::Facts(aka_facts::FactSourceError::Message(
                "embedded engine is required, but this build was compiled without embedded-engine"
                    .into(),
            )))
        }
    }
}

pub fn index_max_duration() -> Duration {
    let seconds = std::env::var("AKA_INDEX_MAX_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .or_else(|| {
            AkaSettings::load()
                .ok()
                .map(|settings| settings.index_max_secs)
        })
        .unwrap_or(DEFAULT_INDEX_MAX_SECS);
    Duration::from_secs(clamp_index_max_secs(seconds))
}

#[cfg_attr(not(feature = "embedded-engine"), allow(dead_code))]
fn engine_cache_root(repo: &Path, cache_dir: Option<&Path>) -> PathBuf {
    cache_dir
        .map(|p| p.join("aka-engine"))
        .unwrap_or_else(|| repo.join(".aka-engine-cache"))
}

fn embedded_runtime_available(base: &Path) -> bool {
    #[cfg(feature = "embedded-engine")]
    {
        if cfg!(windows) {
            embedded_windows_dll_candidates(base).any(|path| path.is_file())
        } else {
            true
        }
    }
    #[cfg(not(feature = "embedded-engine"))]
    {
        let _ = base;
        false
    }
}

#[cfg(feature = "embedded-engine")]
fn embedded_windows_dll_candidates(base: &Path) -> impl Iterator<Item = PathBuf> {
    let mut candidates = Vec::new();
    if base.is_file() {
        candidates.push(base.to_path_buf());
    } else {
        candidates.extend([
            base.join("aka_engine.dll"),
            base.join("engine").join("aka_engine.dll"),
            base.join("resources").join("engine").join("aka_engine.dll"),
            base.join("build").join("c").join("aka_engine.dll"),
        ]);
    }
    candidates.into_iter()
}

#[cfg_attr(not(feature = "embedded-engine"), allow(dead_code))]
fn engine_mode() -> String {
    match std::env::var("AKA_ENGINE_MODE") {
        Ok(mode) if matches!(mode.as_str(), "fast" | "moderate" | "full") => mode,
        _ => DEFAULT_ENGINE_MODE.to_string(),
    }
}

#[cfg_attr(not(feature = "embedded-engine"), allow(dead_code))]
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
