//! aka-core — 域模型、direct facts、仓库注册表、embedded engine 运行器。

pub mod engine;
pub mod enrichment;
pub mod incremental;
pub mod paths;
pub mod registry;
pub mod settings;
pub mod types;

#[cfg(feature = "scip-import")]
pub use aka_facts::{
    import_scip_path, import_scip_path_with_metadata, read_scip_index_metadata, ScipImportError,
    ScipIndexMetadata,
};
pub use aka_facts::{
    produce_semantic_batch, produce_semantic_into, replay_semantic_bundle_into, ChunkFact,
    EdgeFact, FactBatch, FactItem, FactManifest, FactRecord, FactSink, FactSource, FactSourceError,
    FactStats, FileFact, JsonMap, NodeFact, OccurrenceFact, OccurrenceRole, ProducerCapability,
    ProducerContext, RelationFact, RelationKind, SemanticFactBundle, SemanticFactBundleBuilder,
    SemanticFactProducer, SemanticFactSink, SymbolFact, SymbolId, SymbolKind, TextRange,
    FACTS_VERSION,
};
pub use engine::{
    index_max_duration, AnalyzeFactsOptions, EngineError, EngineRunner, IndexingDeadline,
};
pub use enrichment::{
    allowed_lsp_analyzers, allowed_oss_analyzers, find_oss_analyzer, run_optional_lsp_enrichment,
    stamp_enrichment_batch, stamp_enrichment_edge, stamp_enrichment_node, AnalyzerRunMetadata,
    EnrichmentError, LspEnrichmentOutcome, LspEnrichmentPolicy, OssAnalyzer, OssAnalyzerKind,
};
pub use incremental::{
    build_parse_cache_manifest_from_facts, load_index_state, load_parse_cache_manifest,
    save_index_state, save_parse_cache_manifest, FileFactStats, FileFingerprint, IndexDelta,
    IndexState, ParseCacheManifest,
};
pub use paths::{aka_home, repo_dir_name, user_facing_path, RepoPaths};
pub use registry::{
    clamp_render_nodes, Registry, RegistryError, RepoEntry, DEFAULT_RENDER_MAX_NODES,
    MAX_RENDER_NODES, MIN_RENDER_NODES,
};
pub use settings::{
    clamp_index_max_secs, clamp_lsp_enrichment_max_secs, clamp_oss_analyzer_enrichment_max_secs,
    settings_path, AkaSettings, SettingsError, DEFAULT_INDEX_MAX_SECS,
    DEFAULT_LSP_ENRICHMENT_ENABLED, DEFAULT_LSP_ENRICHMENT_MAX_SECS,
    DEFAULT_OSS_ANALYZER_ENRICHMENT_ENABLED, DEFAULT_OSS_ANALYZER_ENRICHMENT_MAX_SECS,
    MAX_INDEX_MAX_SECS, MAX_LSP_ENRICHMENT_MAX_SECS, MAX_OSS_ANALYZER_ENRICHMENT_MAX_SECS,
    MIN_INDEX_MAX_SECS, MIN_LSP_ENRICHMENT_MAX_SECS, MIN_OSS_ANALYZER_ENRICHMENT_MAX_SECS,
};
pub use types::*;
