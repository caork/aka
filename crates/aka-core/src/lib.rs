//! aka-core — 域模型、facts 兼容层、仓库注册表、engine 运行器。

pub mod artifact;
pub mod engine;
pub mod incremental;
pub mod paths;
pub mod registry;
pub mod types;

pub use aka_facts::{
    ChunkFact, EdgeFact, FactBatch, FactItem, FactManifest, FactRecord, FactSource,
    FactSourceError, FactStats, NodeFact, FACTS_VERSION,
};
pub use artifact::{ArtifactDir, ArtifactError, NdjsonIter};
pub use engine::{AnalyzeFactsOptions, EngineError, EngineRunner};
pub use incremental::{
    build_parse_cache_manifest, build_parse_cache_manifest_from_facts, load_index_state,
    load_parse_cache_manifest, save_index_state, save_parse_cache_manifest, FileArtifactStats,
    FileFingerprint, IndexDelta, IndexState, ParseCacheManifest,
};
pub use paths::{aka_home, repo_dir_name, user_facing_path, RepoPaths};
pub use registry::{
    clamp_render_nodes, Registry, RegistryError, RepoEntry, DEFAULT_RENDER_MAX_NODES,
    MAX_RENDER_NODES, MIN_RENDER_NODES,
};
pub use types::*;
