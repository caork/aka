//! Engine fact producers for the fused indexing pipeline.
//!
//! The default implementation still drives the native engine binary, but the
//! boundary is now a producer contract instead of a baked-in sidecar path. The
//! embedded C callback producer and SCIP/stack-graphs importers should plug into
//! this same shape instead of teaching callers about transports.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use aka_facts::{read_complete_fact_records_ndjson, FactBatch, FactSink, FactSourceError};
use serde_json::Value;

use crate::types::{ChunkRec, EdgeRec, EngineEvent, NodeRec};

use super::{emit_phase, find_single_project_db, EngineError};

pub(super) struct EngineFactRequest {
    pub(super) facts_output_path: Option<PathBuf>,
}

pub(super) enum ProducedEngineFacts {
    DirectFacts,
    EngineDbFallback { project: String, db_path: PathBuf },
}

pub(super) trait EngineFactProducer {
    fn prepare(&self, cache_root: &Path) -> Result<EngineFactRequest, EngineError>;

    fn finish(
        &self,
        cache_root: &Path,
        engine_repo: &Path,
        no_chunks: bool,
        sink: &mut dyn FactSink<Error = FactSourceError>,
        on_event: &mut dyn FnMut(&EngineEvent),
    ) -> Result<ProducedEngineFacts, EngineError>;
}

#[derive(Debug, Default)]
pub(super) struct SidecarEngineFactProducer;

impl SidecarEngineFactProducer {
    const SIDECAR_FILE: &'static str = "facts.jsonl";

    fn sidecar_path(cache_root: &Path) -> PathBuf {
        cache_root.join(Self::SIDECAR_FILE)
    }
}

impl EngineFactProducer for SidecarEngineFactProducer {
    fn prepare(&self, cache_root: &Path) -> Result<EngineFactRequest, EngineError> {
        let sidecar_path = Self::sidecar_path(cache_root);
        let _ = std::fs::remove_file(&sidecar_path);
        Ok(EngineFactRequest {
            facts_output_path: Some(sidecar_path),
        })
    }

    fn finish(
        &self,
        cache_root: &Path,
        engine_repo: &Path,
        no_chunks: bool,
        sink: &mut dyn FactSink<Error = FactSourceError>,
        on_event: &mut dyn FnMut(&EngineEvent),
    ) -> Result<ProducedEngineFacts, EngineError> {
        let sidecar_path = Self::sidecar_path(cache_root);
        if sidecar_path.is_file() {
            on_event(&EngineEvent::Log {
                stream: "facts".into(),
                line: format!("using engine direct facts {}", sidecar_path.display()),
            });
            emit_phase(on_event, "aka-engine:facts:read-sidecar", 0, 0);
            let batch = read_engine_facts_sidecar(&sidecar_path, engine_repo, no_chunks, on_event)?;
            batch.replay_into(sink)?;
            return Ok(ProducedEngineFacts::DirectFacts);
        }

        let (project, db_path) = find_single_project_db(cache_root, engine_repo)?;
        on_event(&EngineEvent::Log {
            stream: "facts".into(),
            line: format!(
                "using engine db project={project} path={}",
                db_path.display()
            ),
        });
        Ok(ProducedEngineFacts::EngineDbFallback { project, db_path })
    }
}

fn read_engine_facts_sidecar(
    path: &Path,
    repo: &Path,
    no_chunks: bool,
    on_event: &mut dyn FnMut(&EngineEvent),
) -> Result<FactBatch, EngineError> {
    let file = File::open(path)?;
    let mut batch = read_complete_fact_records_ndjson(BufReader::new(file))?;
    normalize_engine_sidecar_facts(&mut batch, repo, no_chunks, on_event);
    Ok(batch)
}

fn normalize_engine_sidecar_facts(
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
    fn sidecar_producer_finishes_with_direct_batch_without_artifact_dir() {
        let repo = temp_dir("repo");
        let cache = temp_dir("cache");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "fn main() {}\n").unwrap();
        let sidecar = SidecarEngineFactProducer::sidecar_path(&cache);
        std::fs::write(
            &sidecar,
            r#"
{"kind":"node","id":"file:src/lib.rs","label":"File","properties":{"filePath":"src/lib.rs"}}
{"kind":"node","id":"cbm:2:pkg.main","label":"Function","properties":{"filePath":"src/lib.rs","startLine":0,"endLine":1}}
{"kind":"done","stats":{"files":1,"nodes":2,"edges":0,"chunks":0}}
"#,
        )
        .unwrap();
        let mut events = Vec::new();
        let mut sink = aka_facts::FactBatchBuilder::new();

        let produced = SidecarEngineFactProducer
            .finish(&cache, &repo, false, &mut sink, &mut |event| match event {
                EngineEvent::Phase { phase, .. } => events.push(phase.clone()),
                EngineEvent::Log { line, .. } => events.push(line.clone()),
                _ => {}
            })
            .unwrap();

        let ProducedEngineFacts::DirectFacts = produced else {
            panic!("sidecar producer should write direct facts");
        };
        let batch = sink.finish();
        assert_eq!(batch.stats.files, 1);
        assert_eq!(batch.stats.nodes, 2);
        assert_eq!(batch.stats.chunks, 1);
        assert!(!cache.join("artifact").exists());
        assert!(events.iter().any(|line| line.contains("direct facts")));
        assert!(events
            .iter()
            .any(|phase| phase == "aka-engine:facts:read-sidecar"));
    }

    #[test]
    fn reads_engine_sidecar_as_direct_facts() {
        let dir = temp_dir("facts-sidecar");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "fn main() {}\n").unwrap();
        let sidecar = dir.join("facts.jsonl");
        std::fs::write(
            &sidecar,
            r#"
{"kind":"manifest","contractVersion":1,"engineVersion":"test","repoPath":"/repo","generatedAt":"now","stats":{"files":99,"nodes":99,"edges":99,"chunks":99}}
{"kind":"node","id":"file:src/lib.rs","label":"File","properties":{"filePath":"src/lib.rs"}}
{"kind":"node","id":"cbm:2:pkg.main","label":"Function","properties":{"filePath":"src/lib.rs","startLine":0,"endLine":1}}
{"kind":"edge","id":"edge:1","sourceId":"cbm:2:pkg.main","targetId":"file:src/lib.rs","type":"STEP_IN_PROCESS","confidence":1.0,"reason":"","evidence":{"confidence":0.7,"reason":"flow","step":3}}
{"kind":"done","stats":{"files":99,"nodes":99,"edges":99,"chunks":99}}
"#,
        )
        .unwrap();

        let batch = read_engine_facts_sidecar(&sidecar, &dir, false, &mut |_| {}).unwrap();

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
    }

    #[test]
    fn reads_engine_sidecar_honors_no_chunks() {
        let dir = temp_dir("facts-sidecar-no-chunks");
        let sidecar = dir.join("facts.jsonl");
        std::fs::write(
            &sidecar,
            r#"
{"kind":"node","id":"file:src/lib.rs","label":"File","properties":{"filePath":"src/lib.rs"}}
{"kind":"chunk","nodeId":"file:src/lib.rs","chunkKind":"char","filePath":"src/lib.rs","startLine":0,"endLine":0,"text":"fn main() {}"}
{"kind":"done","stats":{"files":1,"nodes":1,"edges":0,"chunks":1}}
"#,
        )
        .unwrap();

        let batch = read_engine_facts_sidecar(&sidecar, &dir, true, &mut |_| {}).unwrap();

        assert_eq!(batch.stats.files, 1);
        assert_eq!(batch.stats.nodes, 1);
        assert_eq!(batch.stats.edges, 0);
        assert_eq!(batch.stats.chunks, 0);
        assert!(batch.chunks.is_empty());
    }

    #[test]
    fn sidecar_producer_prepares_fresh_sidecar_request() {
        let cache = temp_dir("prepare-cache");
        let stale = SidecarEngineFactProducer::sidecar_path(&cache);
        std::fs::write(&stale, "stale").unwrap();

        let request = SidecarEngineFactProducer.prepare(&cache).unwrap();

        assert_eq!(request.facts_output_path, Some(stale.clone()));
        assert!(!stale.exists());
    }
}
