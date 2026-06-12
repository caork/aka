//! aka-core — 域模型、工件合同类型、仓库注册表、engine 运行器。

pub mod artifact;
pub mod engine;
pub mod incremental;
pub mod paths;
pub mod registry;
pub mod types;

pub use artifact::{ArtifactDir, ArtifactError, NdjsonIter};
pub use engine::{EngineError, EngineRunner};
pub use incremental::{
    build_parse_cache_manifest, load_index_state, load_parse_cache_manifest, save_index_state,
    save_parse_cache_manifest, FileArtifactStats, FileFingerprint, IndexDelta, IndexState,
    ParseCacheManifest,
};
pub use paths::{aka_home, RepoPaths};
pub use registry::{
    clamp_render_nodes, Registry, RegistryError, RepoEntry, DEFAULT_RENDER_MAX_NODES,
    MAX_RENDER_NODES, MIN_RENDER_NODES,
};
pub use types::*;
