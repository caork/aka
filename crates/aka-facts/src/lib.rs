//! aka-facts — stable code-intelligence facts shared by parser, fusion, and indexes.
//!
//! This crate is the new in-process contract. Legacy artifact files can still
//! adapt into these records, but graph/search indexing should depend on facts
//! rather than on a disk artifact transport.

use std::io::BufRead;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const FACTS_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactStats {
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub nodes: u64,
    #[serde(default)]
    pub edges: u64,
    #[serde(default)]
    pub chunks: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FactManifest {
    pub contract_version: u32,
    pub engine_version: String,
    pub repo_path: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub generated_at: String,
    #[serde(default)]
    pub stats: FactStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeFact {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub properties: serde_json::Map<String, serde_json::Value>,
}

impl NodeFact {
    fn prop_str(&self, key: &str) -> Option<&str> {
        self.properties.get(key).and_then(|v| v.as_str())
    }

    pub fn name(&self) -> Option<&str> {
        self.prop_str("name")
    }

    pub fn file_path(&self) -> Option<&str> {
        self.prop_str("filePath").or_else(|| self.prop_str("path"))
    }

    /// Fact contract raw value: parser/tree-sitter 0-based row.
    pub fn start_line(&self) -> Option<u32> {
        self.properties
            .get("startLine")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
    }

    /// Fact contract raw value: parser/tree-sitter 0-based row.
    pub fn end_line(&self) -> Option<u32> {
        self.properties
            .get("endLine")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
    }

    /// 1-based human line number used by downstream graph/search/editor APIs.
    pub fn start_line_1based(&self) -> Option<u32> {
        self.start_line().map(|v| v + 1)
    }

    /// 1-based human line number used by downstream graph/search/editor APIs.
    pub fn end_line_1based(&self) -> Option<u32> {
        self.end_line().map(|v| v + 1)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeFact {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkFact {
    pub node_id: String,
    pub kind: String,
    pub file_path: String,
    /// Fact contract raw value: parser/tree-sitter 0-based row.
    #[serde(default)]
    pub start_line: u32,
    #[serde(default)]
    pub end_line: u32,
    pub text: String,
}

impl ChunkFact {
    pub fn start_line_1based(&self) -> u32 {
        self.start_line + 1
    }

    pub fn end_line_1based(&self) -> u32 {
        self.end_line + 1
    }
}

#[derive(Debug, Clone)]
pub enum FactRecord {
    Manifest(FactManifest),
    Node(NodeFact),
    Edge(EdgeFact),
    Chunk(ChunkFact),
    Done { stats: FactStats },
}

impl Serialize for FactRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let value = match self {
            Self::Manifest(manifest) => {
                let mut value =
                    serde_json::to_value(manifest).map_err(serde::ser::Error::custom)?;
                insert_record_kind(&mut value, "manifest");
                value
            }
            Self::Node(node) => {
                let mut value = serde_json::to_value(node).map_err(serde::ser::Error::custom)?;
                insert_record_kind(&mut value, "node");
                value
            }
            Self::Edge(edge) => {
                let mut value = serde_json::to_value(edge).map_err(serde::ser::Error::custom)?;
                insert_record_kind(&mut value, "edge");
                value
            }
            Self::Chunk(chunk) => {
                let mut value = serde_json::to_value(chunk).map_err(serde::ser::Error::custom)?;
                if let Value::Object(obj) = &mut value {
                    if let Some(kind) = obj.remove("kind") {
                        obj.insert("chunkKind".into(), kind);
                    }
                }
                insert_record_kind(&mut value, "chunk");
                value
            }
            Self::Done { stats } => serde_json::json!({
                "kind": "done",
                "stats": stats,
            }),
        };
        value.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FactRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut obj = Map::<String, Value>::deserialize(deserializer)?;
        let kind = obj
            .remove("kind")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| serde::de::Error::missing_field("kind"))?;
        match kind.as_str() {
            "manifest" => serde_json::from_value(Value::Object(obj))
                .map(Self::Manifest)
                .map_err(serde::de::Error::custom),
            "node" => serde_json::from_value(Value::Object(obj))
                .map(Self::Node)
                .map_err(serde::de::Error::custom),
            "edge" => serde_json::from_value(Value::Object(obj))
                .map(Self::Edge)
                .map_err(serde::de::Error::custom),
            "chunk" => {
                if let Some(chunk_kind) = obj.remove("chunkKind") {
                    obj.insert("kind".into(), chunk_kind);
                }
                serde_json::from_value(Value::Object(obj))
                    .map(Self::Chunk)
                    .map_err(serde::de::Error::custom)
            }
            "done" => {
                #[derive(Deserialize)]
                struct DoneRecord {
                    stats: FactStats,
                }
                serde_json::from_value::<DoneRecord>(Value::Object(obj))
                    .map(|done| Self::Done { stats: done.stats })
                    .map_err(serde::de::Error::custom)
            }
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["manifest", "node", "edge", "chunk", "done"],
            )),
        }
    }
}

fn insert_record_kind(value: &mut Value, kind: &'static str) {
    if let Value::Object(obj) = value {
        obj.insert("kind".into(), Value::String(kind.into()));
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FactSourceError {
    #[error("fact io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("fact json error at line {line}: {source}")]
    Json {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("{0}")]
    Message(String),
}

pub type FactItem<T> = Result<T, FactSourceError>;

pub type FactId = String;
pub type SymbolId = String;
pub type JsonMap = Map<String, Value>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextRange {
    pub start_line_0based: u32,
    pub end_line_0based: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_col_0based: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_col_0based: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SymbolKind {
    File,
    Module,
    Package,
    Class,
    Interface,
    Enum,
    Trait,
    Type,
    Function,
    Method,
    Field,
    Variable,
    Route,
    GraphQl,
    Tool,
    Command,
    Config,
    Topic,
    Table,
    Repository,
    Migration,
    Resource,
    Transaction,
    Process,
    Community,
    Unknown(String),
}

impl SymbolKind {
    pub fn label(&self) -> &str {
        match self {
            Self::File => "File",
            Self::Module => "Module",
            Self::Package => "Package",
            Self::Class => "Class",
            Self::Interface => "Interface",
            Self::Enum => "Enum",
            Self::Trait => "Trait",
            Self::Type => "Type",
            Self::Function => "Function",
            Self::Method => "Method",
            Self::Field => "Field",
            Self::Variable => "Variable",
            Self::Route => "Route",
            Self::GraphQl => "GraphQL",
            Self::Tool => "Tool",
            Self::Command => "Command",
            Self::Config => "Config",
            Self::Topic => "Topic",
            Self::Table => "Table",
            Self::Repository => "Repository",
            Self::Migration => "Migration",
            Self::Resource => "Resource",
            Self::Transaction => "Transaction",
            Self::Process => "Process",
            Self::Community => "Community",
            Self::Unknown(label) => label,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OccurrenceRole {
    Definition,
    Declaration,
    Reference,
    Read,
    Write,
    Import,
    Export,
    Call,
    Implementation,
    Override,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RelationKind {
    Contains,
    Defines,
    Calls,
    Imports,
    Inherits,
    Implements,
    DependsOn,
    Reads,
    Writes,
    HandlesRoute,
    HandlesGraphQl,
    HandlesTool,
    HandlesCommand,
    HandlesGraphQlOperation,
    ConsumesTopic,
    PublishesTopic,
    StepInProcess,
    EntryPointOf,
    MemberOf,
    Unknown(String),
}

impl RelationKind {
    pub fn edge_type(&self) -> &str {
        match self {
            Self::Contains => "CONTAINS",
            Self::Defines => "DEFINES",
            Self::Calls => "CALLS",
            Self::Imports => "IMPORTS",
            Self::Inherits => "INHERITS",
            Self::Implements => "IMPLEMENTS",
            Self::DependsOn => "DEPENDS_ON",
            Self::Reads => "READS",
            Self::Writes => "WRITES",
            Self::HandlesRoute => "HANDLES_ROUTE",
            Self::HandlesGraphQl => "HANDLES_GRAPHQL",
            Self::HandlesTool => "HANDLES_TOOL",
            Self::HandlesCommand => "HANDLES_COMMAND",
            Self::HandlesGraphQlOperation => "HANDLES_GRAPHQL",
            Self::ConsumesTopic => "CONSUMES_TOPIC",
            Self::PublishesTopic => "PUBLISHES_TOPIC",
            Self::StepInProcess => "STEP_IN_PROCESS",
            Self::EntryPointOf => "ENTRY_POINT_OF",
            Self::MemberOf => "MEMBER_OF",
            Self::Unknown(edge_type) => edge_type,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFact {
    pub id: FactId,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default)]
    pub generated: bool,
    #[serde(default)]
    pub properties: JsonMap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolFact {
    pub id: FactId,
    pub symbol: SymbolId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualified_name: Option<String>,
    pub kind: SymbolKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<TextRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    #[serde(default)]
    pub properties: JsonMap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OccurrenceFact {
    pub id: FactId,
    pub symbol_id: FactId,
    pub file_id: FactId,
    pub range: TextRange,
    pub role: OccurrenceRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub syntax_kind: Option<String>,
    #[serde(default)]
    pub properties: JsonMap,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationFact {
    pub id: FactId,
    pub source: FactId,
    pub target: FactId,
    pub kind: RelationKind,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SemanticFactBundle {
    #[serde(default)]
    pub files: Vec<FileFact>,
    #[serde(default)]
    pub symbols: Vec<SymbolFact>,
    #[serde(default)]
    pub occurrences: Vec<OccurrenceFact>,
    #[serde(default)]
    pub relations: Vec<RelationFact>,
    #[serde(default)]
    pub chunks: Vec<ChunkFact>,
}

impl SemanticFactBundle {
    pub fn lower(self) -> FactBatch {
        let mut nodes = Vec::with_capacity(self.files.len() + self.symbols.len());
        let mut symbol_ranges = std::collections::BTreeMap::<FactId, TextRange>::new();

        for occurrence in &self.occurrences {
            if matches!(
                occurrence.role,
                OccurrenceRole::Definition | OccurrenceRole::Declaration
            ) {
                symbol_ranges
                    .entry(occurrence.symbol_id.clone())
                    .or_insert_with(|| occurrence.range.clone());
            }
        }

        for file in self.files {
            nodes.push(file.lower());
        }
        for symbol in self.symbols {
            let fallback_range = symbol_ranges.get(&symbol.id);
            nodes.push(symbol.lower(fallback_range));
        }
        let edges: Vec<_> = self
            .relations
            .into_iter()
            .map(RelationFact::lower)
            .collect();
        let stats = FactStats {
            files: nodes
                .iter()
                .filter(|node| node.label == SymbolKind::File.label())
                .count() as u64,
            nodes: nodes.len() as u64,
            edges: edges.len() as u64,
            chunks: self.chunks.len() as u64,
        };
        FactBatch::new(stats, nodes, edges, self.chunks)
    }
}

impl FileFact {
    pub fn lower(self) -> NodeFact {
        let mut properties = self.properties;
        properties.insert("name".into(), Value::String(self.path.clone()));
        properties.insert("path".into(), Value::String(self.path.clone()));
        properties.insert("filePath".into(), Value::String(self.path));
        if let Some(language) = self.language {
            properties.insert("language".into(), Value::String(language));
        }
        if let Some(digest) = self.digest {
            properties.insert("digest".into(), Value::String(digest));
        }
        properties.insert("generated".into(), Value::Bool(self.generated));
        NodeFact {
            id: self.id,
            label: SymbolKind::File.label().into(),
            properties,
        }
    }
}

impl SymbolFact {
    pub fn lower(self, fallback_range: Option<&TextRange>) -> NodeFact {
        let mut properties = self.properties;
        properties.insert("name".into(), Value::String(self.name));
        properties.insert("symbol".into(), Value::String(self.symbol.clone()));
        properties.insert(
            "qualifiedName".into(),
            Value::String(self.qualified_name.unwrap_or(self.symbol)),
        );
        if let Some(file_path) = self.file_path {
            properties.insert("filePath".into(), Value::String(file_path));
        }
        let range = self.range.as_ref().or(fallback_range);
        if let Some(range) = range {
            properties.insert("startLine".into(), Value::from(range.start_line_0based));
            properties.insert("endLine".into(), Value::from(range.end_line_0based));
            if let Some(col) = range.start_col_0based {
                properties.insert("startCol".into(), Value::from(col));
            }
            if let Some(col) = range.end_col_0based {
                properties.insert("endCol".into(), Value::from(col));
            }
        }
        if let Some(documentation) = self.documentation {
            properties.insert("documentation".into(), Value::String(documentation));
        }
        NodeFact {
            id: self.id,
            label: self.kind.label().into(),
            properties,
        }
    }
}

impl RelationFact {
    pub fn lower(self) -> EdgeFact {
        EdgeFact {
            id: self.id,
            source_id: self.source,
            target_id: self.target,
            edge_type: self.kind.edge_type().into(),
            confidence: self.confidence,
            reason: self.reason.unwrap_or_default(),
            step: self.step,
            evidence: self.evidence,
        }
    }
}

/// Reopenable source of normalized facts.
///
/// The current graph/search indexer reads nodes more than once. Direct engine
/// producers can satisfy this contract with an in-memory batch, a replayable
/// channel spool, or a debug file export while the final one-pass writer lands.
pub trait FactSource {
    fn stats(&self) -> &FactStats;

    fn nodes(&self) -> Result<Box<dyn Iterator<Item = FactItem<NodeFact>> + '_>, FactSourceError>;

    fn edges(&self) -> Result<Box<dyn Iterator<Item = FactItem<EdgeFact>> + '_>, FactSourceError>;

    fn chunks(
        &self,
    ) -> Result<Option<Box<dyn Iterator<Item = FactItem<ChunkFact>> + '_>>, FactSourceError>;
}

/// Streaming target for in-process fact producers.
///
/// Embedded parsers, SCIP importers, and stack-graphs adapters should write to
/// this trait instead of serializing to a transport first. `FactSource` is the
/// replayable read side used by the current indexer; `FactSink` is the write
/// side that lets producers feed the same contract through callbacks.
pub trait FactSink {
    type Error;

    fn push_record(&mut self, record: FactRecord) -> Result<(), Self::Error>;

    fn push_manifest(&mut self, manifest: FactManifest) -> Result<(), Self::Error> {
        self.push_record(FactRecord::Manifest(manifest))
    }

    fn push_node(&mut self, node: NodeFact) -> Result<(), Self::Error> {
        self.push_record(FactRecord::Node(node))
    }

    fn push_edge(&mut self, edge: EdgeFact) -> Result<(), Self::Error> {
        self.push_record(FactRecord::Edge(edge))
    }

    fn push_chunk(&mut self, chunk: ChunkFact) -> Result<(), Self::Error> {
        self.push_record(FactRecord::Chunk(chunk))
    }

    fn push_done(&mut self, stats: FactStats) -> Result<(), Self::Error> {
        self.push_record(FactRecord::Done { stats })
    }
}

#[derive(Debug, Clone, Default)]
pub struct FactBatch {
    pub stats: FactStats,
    pub nodes: Vec<NodeFact>,
    pub edges: Vec<EdgeFact>,
    pub chunks: Vec<ChunkFact>,
}

#[derive(Debug, Default)]
pub struct FactBatchBuilder {
    manifest: Option<FactManifest>,
    done_stats: Option<FactStats>,
    nodes: Vec<NodeFact>,
    edges: Vec<EdgeFact>,
    chunks: Vec<ChunkFact>,
    saw_done: bool,
}

impl FactBatchBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_record(&mut self, record: FactRecord) {
        match record {
            FactRecord::Manifest(manifest) => {
                self.manifest = Some(manifest);
            }
            FactRecord::Node(node) => self.nodes.push(node),
            FactRecord::Edge(edge) => self.edges.push(edge),
            FactRecord::Chunk(chunk) => self.chunks.push(chunk),
            FactRecord::Done { stats } => {
                self.done_stats = Some(stats);
                self.saw_done = true;
            }
        }
    }

    pub fn saw_done(&self) -> bool {
        self.saw_done
    }

    pub fn finish(self) -> FactBatch {
        let computed = FactStats {
            files: self
                .nodes
                .iter()
                .filter(|node| node.label == SymbolKind::File.label())
                .count() as u64,
            nodes: self.nodes.len() as u64,
            edges: self.edges.len() as u64,
            chunks: self.chunks.len() as u64,
        };
        let stats = self
            .done_stats
            .or_else(|| self.manifest.map(|manifest| manifest.stats))
            .unwrap_or(computed);
        FactBatch::new(stats, self.nodes, self.edges, self.chunks)
    }
}

impl FactSink for FactBatchBuilder {
    type Error = FactSourceError;

    fn push_record(&mut self, record: FactRecord) -> Result<(), Self::Error> {
        FactBatchBuilder::push_record(self, record);
        Ok(())
    }
}

impl FactBatch {
    pub fn new(
        stats: FactStats,
        nodes: Vec<NodeFact>,
        edges: Vec<EdgeFact>,
        chunks: Vec<ChunkFact>,
    ) -> Self {
        Self {
            stats,
            nodes,
            edges,
            chunks,
        }
    }

    pub fn replay_into<S: FactSink + ?Sized>(&self, sink: &mut S) -> Result<(), S::Error> {
        for node in &self.nodes {
            sink.push_node(node.clone())?;
        }
        for edge in &self.edges {
            sink.push_edge(edge.clone())?;
        }
        for chunk in &self.chunks {
            sink.push_chunk(chunk.clone())?;
        }
        sink.push_done(self.stats.clone())
    }
}

pub fn read_fact_records_ndjson(reader: impl BufRead) -> Result<FactBatch, FactSourceError> {
    let mut builder = FactBatchBuilder::new();
    for (idx, line) in reader.lines().enumerate() {
        let line_no = idx + 1;
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: FactRecord =
            serde_json::from_str(trimmed).map_err(|source| FactSourceError::Json {
                line: line_no,
                source,
            })?;
        builder.push_record(record);
    }
    Ok(builder.finish())
}

pub fn read_complete_fact_records_ndjson(
    reader: impl BufRead,
) -> Result<FactBatch, FactSourceError> {
    let mut builder = FactBatchBuilder::new();
    for (idx, line) in reader.lines().enumerate() {
        let line_no = idx + 1;
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: FactRecord =
            serde_json::from_str(trimmed).map_err(|source| FactSourceError::Json {
                line: line_no,
                source,
            })?;
        builder.push_record(record);
    }
    if !builder.saw_done() {
        return Err(FactSourceError::Message(
            "fact stream missing terminal done record".into(),
        ));
    }
    Ok(builder.finish())
}

impl FactSource for FactBatch {
    fn stats(&self) -> &FactStats {
        &self.stats
    }

    fn nodes(&self) -> Result<Box<dyn Iterator<Item = FactItem<NodeFact>> + '_>, FactSourceError> {
        Ok(Box::new(self.nodes.iter().cloned().map(Ok)))
    }

    fn edges(&self) -> Result<Box<dyn Iterator<Item = FactItem<EdgeFact>> + '_>, FactSourceError> {
        Ok(Box::new(self.edges.iter().cloned().map(Ok)))
    }

    fn chunks(
        &self,
    ) -> Result<Option<Box<dyn Iterator<Item = FactItem<ChunkFact>> + '_>>, FactSourceError> {
        Ok(Some(Box::new(self.chunks.iter().cloned().map(Ok))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_fact_preserves_line_semantics() {
        let node = NodeFact {
            id: "n1".into(),
            label: "Function".into(),
            properties: serde_json::json!({
                "name": "loadManifest",
                "filePath": "src/lib.rs",
                "startLine": 0,
                "endLine": 2
            })
            .as_object()
            .unwrap()
            .clone(),
        };

        assert_eq!(node.name(), Some("loadManifest"));
        assert_eq!(node.file_path(), Some("src/lib.rs"));
        assert_eq!(node.start_line(), Some(0));
        assert_eq!(node.start_line_1based(), Some(1));
        assert_eq!(node.end_line_1based(), Some(3));
    }

    #[test]
    fn semantic_bundle_lowers_to_replayable_facts() {
        let bundle = SemanticFactBundle {
            files: vec![FileFact {
                id: "file:src/lib.rs".into(),
                path: "src/lib.rs".into(),
                language: Some("rust".into()),
                digest: Some("sha256:abc".into()),
                generated: false,
                properties: JsonMap::new(),
            }],
            symbols: vec![SymbolFact {
                id: "sym:loadManifest".into(),
                symbol: "rust src/lib.rs/loadManifest().".into(),
                name: "loadManifest".into(),
                qualified_name: None,
                kind: SymbolKind::Function,
                file_path: Some("src/lib.rs".into()),
                range: None,
                documentation: None,
                properties: JsonMap::new(),
            }],
            occurrences: vec![OccurrenceFact {
                id: "occ:1".into(),
                symbol_id: "sym:loadManifest".into(),
                file_id: "file:src/lib.rs".into(),
                range: TextRange {
                    start_line_0based: 4,
                    end_line_0based: 8,
                    start_col_0based: Some(0),
                    end_col_0based: Some(1),
                },
                role: OccurrenceRole::Definition,
                syntax_kind: None,
                properties: JsonMap::new(),
            }],
            relations: vec![RelationFact {
                id: "rel:1".into(),
                source: "sym:loadManifest".into(),
                target: "file:src/lib.rs".into(),
                kind: RelationKind::Defines,
                confidence: 1.0,
                reason: Some("definition".into()),
                step: None,
                evidence: Some(Value::String("test".into())),
            }],
            chunks: vec![ChunkFact {
                node_id: "sym:loadManifest".into(),
                kind: "ast-function".into(),
                file_path: "src/lib.rs".into(),
                start_line: 4,
                end_line: 8,
                text: "fn load_manifest() {}".into(),
            }],
        };

        let facts = bundle.lower();
        let nodes: Vec<_> = facts.nodes().unwrap().map(Result::unwrap).collect();
        let edges: Vec<_> = facts.edges().unwrap().map(Result::unwrap).collect();
        let chunks: Vec<_> = facts
            .chunks()
            .unwrap()
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert_eq!(facts.stats.files, 1);
        assert_eq!(facts.stats.nodes, 2);
        assert_eq!(facts.stats.edges, 1);
        assert_eq!(facts.stats.chunks, 1);
        assert_eq!(nodes[1].label, "Function");
        assert_eq!(nodes[1].start_line(), Some(4));
        assert_eq!(edges[0].edge_type, "DEFINES");
        assert_eq!(chunks[0].start_line_1based(), 5);
    }

    #[test]
    fn fact_batch_is_replayable() {
        let batch = FactBatch::new(
            FactStats {
                files: 1,
                nodes: 1,
                edges: 0,
                chunks: 0,
            },
            vec![NodeFact {
                id: "n1".into(),
                label: "File".into(),
                properties: serde_json::Map::new(),
            }],
            Vec::new(),
            Vec::new(),
        );

        assert_eq!(batch.nodes().unwrap().count(), 1);
        assert_eq!(batch.nodes().unwrap().count(), 1);
        assert_eq!(batch.stats().nodes, 1);
    }

    #[test]
    fn fact_batch_replays_into_streaming_sink() {
        let batch = FactBatch::new(
            FactStats {
                files: 1,
                nodes: 1,
                edges: 1,
                chunks: 1,
            },
            vec![NodeFact {
                id: "file:src/lib.rs".into(),
                label: "File".into(),
                properties: serde_json::Map::new(),
            }],
            vec![EdgeFact {
                id: "edge:1".into(),
                source_id: "file:src/lib.rs".into(),
                target_id: "file:src/lib.rs".into(),
                edge_type: "SELF".into(),
                confidence: 1.0,
                reason: "test".into(),
                step: None,
                evidence: None,
            }],
            vec![ChunkFact {
                node_id: "file:src/lib.rs".into(),
                kind: "char".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "fn main() {}".into(),
            }],
        );
        let mut builder = FactBatchBuilder::new();

        batch.replay_into(&mut builder).unwrap();
        let replayed = builder.finish();

        assert_eq!(replayed.stats, batch.stats);
        assert_eq!(replayed.nodes.len(), 1);
        assert_eq!(replayed.edges.len(), 1);
        assert_eq!(replayed.chunks.len(), 1);
    }

    #[test]
    fn fact_batch_builder_accepts_streaming_sink_events() {
        let mut builder = FactBatchBuilder::new();
        {
            let sink: &mut dyn FactSink<Error = FactSourceError> = &mut builder;
            sink.push_node(NodeFact {
                id: "file:src/lib.rs".into(),
                label: "File".into(),
                properties: serde_json::json!({ "filePath": "src/lib.rs" })
                    .as_object()
                    .unwrap()
                    .clone(),
            })
            .unwrap();
            sink.push_node(NodeFact {
                id: "sym:main".into(),
                label: "Function".into(),
                properties: serde_json::json!({
                    "name": "main",
                    "filePath": "src/lib.rs"
                })
                .as_object()
                .unwrap()
                .clone(),
            })
            .unwrap();
            sink.push_edge(EdgeFact {
                id: "edge:1".into(),
                source_id: "sym:main".into(),
                target_id: "file:src/lib.rs".into(),
                edge_type: "DEFINES".into(),
                confidence: 1.0,
                reason: "streaming callback".into(),
                step: None,
                evidence: None,
            })
            .unwrap();
            sink.push_chunk(ChunkFact {
                node_id: "sym:main".into(),
                kind: "ast-function".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "fn main() {}".into(),
            })
            .unwrap();
            sink.push_done(FactStats {
                files: 1,
                nodes: 2,
                edges: 1,
                chunks: 1,
            })
            .unwrap();
        }

        assert!(builder.saw_done());
        let batch = builder.finish();

        assert_eq!(batch.stats.files, 1);
        assert_eq!(batch.stats.nodes, 2);
        assert_eq!(batch.stats.edges, 1);
        assert_eq!(batch.stats.chunks, 1);
        assert_eq!(batch.nodes().unwrap().count(), 2);
        assert_eq!(batch.edges().unwrap().count(), 1);
        assert_eq!(batch.chunks().unwrap().unwrap().count(), 1);
    }

    #[test]
    fn reads_fact_records_ndjson_into_replayable_batch() {
        let input = r#"
{"kind":"manifest","contractVersion":1,"engineVersion":"test","repoPath":"/repo","generatedAt":"now","stats":{"files":9,"nodes":9,"edges":9,"chunks":9}}
{"kind":"node","id":"file:src/lib.rs","label":"File","properties":{"filePath":"src/lib.rs"}}
{"kind":"node","id":"sym:main","label":"Function","properties":{"name":"main","filePath":"src/lib.rs","startLine":0,"endLine":2}}
{"kind":"edge","id":"edge:1","sourceId":"sym:main","targetId":"file:src/lib.rs","type":"DEFINES","confidence":1.0,"reason":"test"}
{"kind":"chunk","nodeId":"sym:main","chunkKind":"ast-function","filePath":"src/lib.rs","startLine":0,"endLine":2,"text":"fn main() {}"}
{"kind":"done","stats":{"files":1,"nodes":2,"edges":1,"chunks":1}}
"#;

        let batch = read_fact_records_ndjson(std::io::Cursor::new(input)).unwrap();

        assert_eq!(batch.stats.files, 1);
        assert_eq!(batch.stats.nodes, 2);
        assert_eq!(batch.stats.edges, 1);
        assert_eq!(batch.stats.chunks, 1);
        assert_eq!(batch.nodes().unwrap().count(), 2);
        assert_eq!(batch.edges().unwrap().count(), 1);
        assert_eq!(batch.chunks().unwrap().unwrap().count(), 1);
    }

    #[test]
    fn reads_fact_records_ndjson_uses_computed_stats_without_done_or_manifest() {
        let input = r#"
{"kind":"node","id":"file:src/lib.rs","label":"File","properties":{"filePath":"src/lib.rs"}}
{"kind":"node","id":"sym:main","label":"Function","properties":{"name":"main","filePath":"src/lib.rs"}}
"#;

        let batch = read_fact_records_ndjson(std::io::Cursor::new(input)).unwrap();

        assert_eq!(batch.stats.files, 1);
        assert_eq!(batch.stats.nodes, 2);
        assert_eq!(batch.stats.edges, 0);
        assert_eq!(batch.stats.chunks, 0);
    }

    #[test]
    fn reads_fact_records_ndjson_reports_json_line_number() {
        let input =
            "\n{\"kind\":\"node\",\"id\":\"n1\",\"label\":\"File\",\"properties\":{}}\nnot-json\n";

        let err = read_fact_records_ndjson(std::io::Cursor::new(input)).unwrap_err();

        assert!(err.to_string().contains("line 3"));
    }

    #[test]
    fn complete_fact_records_ndjson_requires_done_record() {
        let input = "{\"kind\":\"node\",\"id\":\"n1\",\"label\":\"File\",\"properties\":{}}\n";

        let err = read_complete_fact_records_ndjson(std::io::Cursor::new(input)).unwrap_err();

        assert!(err.to_string().contains("missing terminal done"));
    }
}
