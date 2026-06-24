//! Compatibility exports for the legacy artifact names.
//!
//! The stable indexing contract now lives in `aka-facts`. These aliases keep
//! the existing graph/search/service crates compiling while the hot path moves
//! from disk artifacts to direct fact sources.

use serde::{Deserialize, Serialize};

/// Legacy artifact directory contract. Direct facts use `aka_facts::FACTS_VERSION`.
pub const CONTRACT_VERSION: u32 = 0;

pub type NodeRec = aka_facts::NodeFact;
pub type EdgeRec = aka_facts::EdgeFact;
pub type ChunkRec = aka_facts::ChunkFact;
pub type ArtifactStats = aka_facts::FactStats;
pub type Manifest = aka_facts::FactManifest;

/// engine stdout 的进度事件（NDJSON，每行一个）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum EngineEvent {
    Phase {
        phase: String,
        #[serde(default)]
        current: u64,
        #[serde(default)]
        total: u64,
    },
    Warning {
        message: String,
    },
    Log {
        stream: String,
        line: String,
    },
    Done {
        #[serde(default)]
        stats: ArtifactStats,
    },
}
