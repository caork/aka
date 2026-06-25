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
    Progress {
        progress: PipelineProgress,
    },
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStage {
    Prepare,
    EngineDiscover,
    EngineParse,
    EngineEmit,
    FactsNormalize,
    GraphNodes,
    GraphEdges,
    GraphLayout,
    SearchNodes,
    SearchChunks,
    SearchCommit,
    ParseCache,
    Register,
    LspEnrichment,
    Done,
    Timeout,
}

impl PipelineStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::EngineDiscover => "engine:discover",
            Self::EngineParse => "engine:parse",
            Self::EngineEmit => "engine:emit",
            Self::FactsNormalize => "facts:normalize",
            Self::GraphNodes => "graph:nodes",
            Self::GraphEdges => "graph:edges",
            Self::GraphLayout => "graph:layout",
            Self::SearchNodes => "search:nodes",
            Self::SearchChunks => "search:chunks",
            Self::SearchCommit => "search:commit",
            Self::ParseCache => "parse-cache",
            Self::Register => "register",
            Self::LspEnrichment => "lsp-enrichment",
            Self::Done => "done",
            Self::Timeout => "timeout",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipelineProgress {
    pub stage: PipelineStage,
    pub message: String,
    #[serde(default)]
    pub current: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub nodes: u64,
    #[serde(default)]
    pub edges: u64,
    #[serde(default)]
    pub chunks: u64,
}

impl PipelineProgress {
    pub fn new(stage: PipelineStage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
            current: 0,
            total: 0,
            files: 0,
            nodes: 0,
            edges: 0,
            chunks: 0,
        }
    }

    pub fn counts(mut self, current: u64, total: u64) -> Self {
        self.current = current;
        self.total = total;
        self
    }

    pub fn stats(mut self, stats: &FactStats) -> Self {
        self.files = stats.files;
        self.nodes = stats.nodes;
        self.edges = stats.edges;
        self.chunks = stats.chunks;
        self
    }
}
