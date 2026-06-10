//! 工件合同 v0 的数据类型 — 与 docs/contracts/artifacts.md 一一对应。
//! 字段只增不改不删；破坏性变更须 bump `CONTRACT_VERSION`。

use serde::{Deserialize, Serialize};

pub const CONTRACT_VERSION: u32 = 0;

/// nodes.ndjson 的一行 — gitnexus-shared `GraphNode` 原样透传。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRec {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
}

impl NodeRec {
    fn prop_str(&self, key: &str) -> Option<&str> {
        self.properties.get(key).and_then(|v| v.as_str())
    }

    pub fn name(&self) -> Option<&str> {
        self.prop_str("name")
    }

    pub fn file_path(&self) -> Option<&str> {
        self.prop_str("filePath").or_else(|| self.prop_str("path"))
    }

    pub fn start_line(&self) -> Option<u32> {
        self.properties
            .get("startLine")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
    }

    pub fn end_line(&self) -> Option<u32> {
        self.properties
            .get("endLine")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
    }
}

/// edges.ndjson 的一行 — gitnexus-shared `GraphRelationship` 原样透传。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeRec {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<serde_json::Value>,
}

/// chunks.ndjson 的一行 — embedding 切块（向量由 aka 侧按需计算）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkRec {
    pub node_id: String,
    pub kind: String,
    pub file_path: String,
    #[serde(default)]
    pub start_line: u32,
    #[serde(default)]
    pub end_line: u32,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtifactStats {
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub nodes: u64,
    #[serde(default)]
    pub edges: u64,
    #[serde(default)]
    pub chunks: u64,
}

/// manifest.json — 最后写入，作为工件完整性标记。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub contract_version: u32,
    pub engine_version: String,
    pub repo_path: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub generated_at: String,
    #[serde(default)]
    pub stats: ArtifactStats,
}

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
    Done {
        #[serde(default)]
        stats: ArtifactStats,
    },
}
