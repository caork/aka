//! Engine fact normalization for the embedded direct-facts pipeline.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use aka_facts::FactBatch;
use serde_json::Value;

use crate::types::{ChunkRec, EdgeRec, EngineEvent, NodeRec};

use super::emit_phase;

pub(super) enum ProducedEngineFacts {
    DirectFacts,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct EngineFactOptions<'a> {
    pub(super) cache_dir: Option<&'a Path>,
    pub(super) no_chunks: bool,
}

pub(super) fn normalize_engine_facts(
    batch: &mut FactBatch,
    repo: &Path,
    no_chunks: bool,
    on_event: &mut dyn FnMut(&EngineEvent),
) {
    for node in &mut batch.nodes {
        normalize_engine_node_fact(node);
    }
    for edge in &mut batch.edges {
        normalize_engine_edge_fact(edge);
    }
    if no_chunks {
        batch.chunks.clear();
    } else if batch.chunks.is_empty() {
        batch.chunks = synthesize_chunks_from_node_facts(repo, &batch.nodes, on_event);
    }
    batch.stats.files = batch
        .nodes
        .iter()
        .filter(|node| node.label == "File")
        .count() as u64;
    batch.stats.nodes = batch.nodes.len() as u64;
    batch.stats.edges = batch.edges.len() as u64;
    batch.stats.chunks = batch.chunks.len() as u64;
}

fn synthesize_chunks_from_node_facts(
    repo: &Path,
    nodes: &[NodeRec],
    on_event: &mut dyn FnMut(&EngineEvent),
) -> Vec<ChunkRec> {
    let candidates: Vec<_> = nodes
        .iter()
        .filter(|node| {
            node.file_path().is_some()
                && !matches!(
                    node.label.as_str(),
                    "File" | "Folder" | "Project" | "Package" | "Module"
                )
        })
        .collect();
    let total = candidates.len() as u64;
    emit_phase(on_event, "aka-core:enrichment:chunks-from-facts", 0, total);
    let mut sources = SourceCache::new(repo);
    let mut chunks = Vec::with_capacity(candidates.len());
    let mut count = 0u64;
    for node in candidates {
        let Some(file_path) = node.file_path() else {
            continue;
        };
        let start_line = node.start_line().map(|line| line as i64 + 1).unwrap_or(1);
        let end_line = node
            .end_line()
            .map(|line| line as i64 + 1)
            .unwrap_or(start_line);
        let text = sources
            .read_line_span(file_path, start_line, end_line)
            .unwrap_or_default();
        chunks.push(ChunkRec {
            node_id: node.id.clone(),
            kind: format!("ast-{}", node.label.to_ascii_lowercase()),
            file_path: file_path.to_string(),
            start_line: to_fact_line(start_line),
            end_line: to_fact_line(end_line),
            text,
        });
        count += 1;
        if count == total || count.is_multiple_of(1000) {
            emit_phase(
                on_event,
                "aka-core:enrichment:chunks-from-facts",
                count,
                total,
            );
        }
    }
    chunks
}

fn normalize_engine_edge_fact(edge: &mut EdgeRec) {
    let Some(Value::Object(evidence)) = edge.evidence.as_ref() else {
        return;
    };
    if edge.reason.is_empty() {
        if let Some(reason) = evidence.get("reason").and_then(Value::as_str) {
            edge.reason = reason.to_string();
        }
    }
    if let Some(confidence) = evidence.get("confidence").and_then(Value::as_f64) {
        edge.confidence = confidence;
    }
    if edge.step.is_none() {
        edge.step = evidence
            .get("step")
            .and_then(Value::as_u64)
            .map(|value| value as u32);
    }
}

fn normalize_engine_node_fact(node: &mut NodeRec) {
    let mut cbm_id = None;
    let mut qualified_name = None;
    if let Some(rest) = node.id.strip_prefix("cbm:") {
        if let Some((id, qn)) = rest.split_once(':') {
            cbm_id = id.parse::<i64>().ok();
            qualified_name = Some(qn.to_string());
        }
    }
    if let Some(id) = cbm_id {
        insert_if_missing(&mut node.properties, "cbmId", Value::from(id));
    }
    if let Some(qn) = qualified_name {
        insert_if_missing(
            &mut node.properties,
            "qualifiedName",
            Value::String(qn.clone()),
        );
        insert_if_missing(&mut node.properties, "name", Value::String(short_name(&qn)));
    }
}

fn short_name(qualified_name: &str) -> String {
    qualified_name
        .rsplit([':', '.', '/', '#'])
        .find(|part| !part.is_empty())
        .unwrap_or(qualified_name)
        .to_string()
}

fn insert_if_missing(props: &mut serde_json::Map<String, Value>, key: &str, value: Value) {
    props.entry(key.to_string()).or_insert(value);
}

fn to_fact_line(line_1based: i64) -> u32 {
    if line_1based <= 0 {
        0
    } else {
        (line_1based - 1) as u32
    }
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
            match read_source_lines(&path) {
                Ok(lines) => {
                    self.files.insert(file_path.to_string(), lines);
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
}

fn read_source_lines(path: &Path) -> std::io::Result<Vec<String>> {
    let mut text = String::new();
    File::open(path)?.read_to_string(&mut text)?;
    Ok(text.lines().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aka-core-engine-fact-producer-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn normalize_engine_facts_synthesizes_chunks_from_nodes() {
        let repo = temp_dir("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "fn main() {}\n").unwrap();
        let mut events = Vec::new();
        let mut batch = FactBatch::new(
            Default::default(),
            vec![
                NodeRec {
                    id: "file:src/lib.rs".into(),
                    label: "File".into(),
                    properties: serde_json::json!({"filePath": "src/lib.rs"})
                        .as_object()
                        .unwrap()
                        .clone(),
                },
                NodeRec {
                    id: "cbm:2:pkg.main".into(),
                    label: "Function".into(),
                    properties: serde_json::json!({
                        "filePath": "src/lib.rs",
                        "startLine": 0,
                        "endLine": 1
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                },
            ],
            vec![EdgeRec {
                id: "edge:1".into(),
                source_id: "cbm:2:pkg.main".into(),
                target_id: "file:src/lib.rs".into(),
                edge_type: "STEP_IN_PROCESS".into(),
                confidence: 1.0,
                reason: String::new(),
                step: None,
                evidence: Some(serde_json::json!({
                    "confidence": 0.7,
                    "reason": "flow",
                    "step": 3
                })),
            }],
            Vec::new(),
        );

        normalize_engine_facts(&mut batch, &repo, false, &mut |event| {
            if let EngineEvent::Phase { phase, .. } = event {
                events.push(phase.clone());
            }
        });

        assert_eq!(batch.stats.files, 1);
        assert_eq!(batch.stats.nodes, 2);
        assert_eq!(batch.stats.edges, 1);
        assert_eq!(batch.stats.chunks, 1);
        assert_eq!(batch.nodes[1].properties["cbmId"], 2);
        assert_eq!(batch.nodes[1].properties["qualifiedName"], "pkg.main");
        assert_eq!(batch.nodes[1].properties["name"], "main");
        assert_eq!(batch.edges[0].confidence, 0.7);
        assert_eq!(batch.edges[0].reason, "flow");
        assert_eq!(batch.edges[0].step, Some(3));
        assert_eq!(batch.chunks[0].text, "fn main() {}");
        assert!(events
            .iter()
            .any(|phase| phase == "aka-core:enrichment:chunks-from-facts"));
    }

    #[test]
    fn normalize_engine_facts_honors_no_chunks() {
        let dir = temp_dir("no-chunks");
        let mut batch = FactBatch::new(
            Default::default(),
            vec![NodeRec {
                id: "file:src/lib.rs".into(),
                label: "File".into(),
                properties: serde_json::json!({"filePath": "src/lib.rs"})
                    .as_object()
                    .unwrap()
                    .clone(),
            }],
            Vec::new(),
            vec![ChunkRec {
                node_id: "file:src/lib.rs".into(),
                kind: "char".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "fn main() {}".into(),
            }],
        );

        normalize_engine_facts(&mut batch, &dir, true, &mut |_| {});
        assert_eq!(batch.stats.files, 1);
        assert_eq!(batch.stats.nodes, 1);
        assert_eq!(batch.stats.edges, 0);
        assert_eq!(batch.stats.chunks, 0);
        assert!(batch.chunks.is_empty());
    }
}
