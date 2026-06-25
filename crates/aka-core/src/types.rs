//! Shared core types for direct facts and engine progress.

use serde::{Deserialize, Serialize};

pub const CONTRACT_VERSION: u32 = aka_facts::FACTS_VERSION;

pub type NodeRec = aka_facts::NodeFact;
pub type EdgeRec = aka_facts::EdgeFact;
pub type ChunkRec = aka_facts::ChunkFact;
pub type FactStats = aka_facts::FactStats;

/// Engine/runtime progress event.
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
        stats: FactStats,
    },
}
