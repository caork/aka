use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::thread;

use aka_facts::{FactBatchBuilder, FactManifest, FactSink, FactSourceError, FactStats};
use chrono::Utc;
use serde_json::{Map, Value};

use crate::types::{EdgeRec, EngineEvent, NodeRec};

use super::fact_producer::normalize_engine_facts;
use super::{emit_phase, engine_cache_root, engine_mode, EngineError};

#[repr(C)]
#[derive(Clone, Copy)]
enum AkaEngineMode {
    Full = 0,
    Moderate = 1,
    Fast = 2,
}

#[repr(C)]
struct AkaEngineIndexOptions {
    repo_path: *const c_char,
    cache_dir: *const c_char,
    mode: AkaEngineMode,
    direct_facts_only: bool,
}

type ManifestCallback = extern "C" fn(*mut c_void, *const c_char, c_int, c_int, c_int) -> c_int;
type NodeCallback = extern "C" fn(
    *mut c_void,
    i64,
    *const c_char,
    *const c_char,
    *const c_char,
    *const c_char,
    c_int,
    c_int,
    *const c_char,
) -> c_int;
type EdgeCallback = extern "C" fn(
    *mut c_void,
    i64,
    i64,
    *const c_char,
    i64,
    *const c_char,
    *const c_char,
    c_double,
    *const c_char,
    c_int,
    i64,
    *const c_char,
) -> c_int;
type DoneCallback = extern "C" fn(*mut c_void, c_int, c_int, c_int) -> c_int;

#[repr(C)]
struct AkaEngineFactSink {
    userdata: *mut c_void,
    manifest: Option<ManifestCallback>,
    node: Option<NodeCallback>,
    edge: Option<EdgeCallback>,
    done: Option<DoneCallback>,
}

extern "C" {
    fn aka_engine_index_with_sink(
        options: *const AkaEngineIndexOptions,
        sink: *const AkaEngineFactSink,
    ) -> c_int;
}

#[derive(Debug, Default)]
pub(super) struct EmbeddedEngineFactProducer;

impl EmbeddedEngineFactProducer {
    pub(super) fn produce(
        &self,
        repo: &Path,
        options: super::EngineFactOptions<'_>,
        sink: &mut dyn FactSink<Error = FactSourceError>,
        on_event: &mut dyn FnMut(&EngineEvent),
    ) -> Result<super::ProducedEngineFacts, EngineError> {
        let cache_root = engine_cache_root(repo, options.debug_artifact_dir, options.cache_dir);
        std::fs::create_dir_all(&cache_root)?;

        let engine_repo = crate::user_facing_path(repo);
        let engine_repo = engine_repo
            .canonicalize()
            .map(|path| crate::user_facing_path(&path))
            .unwrap_or(engine_repo);
        let mode_name = engine_mode();
        let mode = ffi_mode(&mode_name);

        on_event(&EngineEvent::Log {
            stream: "engine".into(),
            line: format!(
                "embedded repo_path={} cache_dir={} mode={}",
                engine_repo.display(),
                cache_root.display(),
                mode_name
            ),
        });
        emit_phase(on_event, "aka-engine:index:embedded", 0, 0);

        let mut batch = run_embedded_engine_on_large_stack(engine_repo.clone(), cache_root, mode)?;

        normalize_engine_facts(&mut batch, &engine_repo, options.no_chunks, on_event);
        batch.replay_into(sink)?;
        Ok(super::ProducedEngineFacts::DirectFacts)
    }
}

#[cfg(test)]
pub(super) fn run_embedded_engine_smoke(
    repo: &Path,
    cache_dir: &Path,
    sink: &mut dyn FactSink<Error = FactSourceError>,
    on_event: &mut dyn FnMut(&EngineEvent),
) -> Result<super::ProducedEngineFacts, EngineError> {
    EmbeddedEngineFactProducer.produce(
        repo,
        super::EngineFactOptions {
            cache_dir: Some(cache_dir),
            debug_artifact_dir: None,
            no_chunks: false,
        },
        sink,
        on_event,
    )
}

fn run_embedded_engine_on_large_stack(
    engine_repo: PathBuf,
    cache_root: PathBuf,
    mode: AkaEngineMode,
) -> Result<aka_facts::FactBatch, EngineError> {
    thread::Builder::new()
        .name("aka-embedded-engine".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_embedded_engine_inner(&engine_repo, &cache_root, mode))
        .map_err(|error| {
            EngineError::Facts(FactSourceError::Message(format!(
                "spawn embedded engine thread failed: {error}"
            )))
        })?
        .join()
        .map_err(|_| EngineError::Failed {
            code: None,
            stderr_tail: "panic in embedded engine thread".into(),
        })?
}

fn run_embedded_engine_inner(
    engine_repo: &Path,
    cache_root: &Path,
    mode: AkaEngineMode,
) -> Result<aka_facts::FactBatch, EngineError> {
    let repo_c = cstring_path(engine_repo)?;
    let cache_c = cstring_path(cache_root)?;
    let mut batch = FactBatchBuilder::new();
    let mut bridge = SinkBridge {
        sink: &mut batch,
        error: None,
    };
    let ffi_sink = AkaEngineFactSink {
        userdata: (&mut bridge as *mut SinkBridge<'_>).cast::<c_void>(),
        manifest: Some(manifest_callback),
        node: Some(node_callback),
        edge: Some(edge_callback),
        done: Some(done_callback),
    };
    let ffi_options = AkaEngineIndexOptions {
        repo_path: repo_c.as_ptr(),
        cache_dir: cache_c.as_ptr(),
        mode,
        direct_facts_only: true,
    };

    let rc = unsafe { aka_engine_index_with_sink(&ffi_options, &ffi_sink) };
    if let Some(error) = bridge.error {
        return Err(error);
    }
    if rc != 0 {
        return Err(EngineError::Failed {
            code: Some(rc),
            stderr_tail: "embedded engine returned nonzero status".into(),
        });
    }
    Ok(batch.finish())
}

struct SinkBridge<'a> {
    sink: &'a mut dyn FactSink<Error = FactSourceError>,
    error: Option<EngineError>,
}

extern "C" fn manifest_callback(
    userdata: *mut c_void,
    repo_path: *const c_char,
    files: c_int,
    nodes: c_int,
    edges: c_int,
) -> c_int {
    with_bridge(userdata, |bridge| {
        bridge.sink.push_manifest(FactManifest {
            contract_version: aka_facts::FACTS_VERSION,
            engine_version: "aka-engine".into(),
            repo_path: cstr(repo_path).unwrap_or_default(),
            commit: None,
            generated_at: Utc::now().to_rfc3339(),
            stats: stats(files, nodes, edges, 0),
        })
    })
}

extern "C" fn node_callback(
    userdata: *mut c_void,
    cbm_id: i64,
    label: *const c_char,
    name: *const c_char,
    qualified_name: *const c_char,
    file_path: *const c_char,
    start_line_0based: c_int,
    end_line_0based: c_int,
    properties_json: *const c_char,
) -> c_int {
    with_bridge(userdata, |bridge| {
        let qualified_name = cstr(qualified_name).unwrap_or_default();
        let name = cstr(name).unwrap_or_default();
        let file_path = cstr(file_path).unwrap_or_default();
        let mut properties = parse_json_object(properties_json)?;
        insert_if_missing(&mut properties, "cbmId", Value::from(cbm_id));
        insert_if_missing(
            &mut properties,
            "qualifiedName",
            Value::String(qualified_name.clone()),
        );
        insert_if_missing(&mut properties, "name", Value::String(name));
        insert_if_missing(&mut properties, "filePath", Value::String(file_path));
        insert_if_missing(
            &mut properties,
            "startLine",
            Value::from(clamp_line(start_line_0based)),
        );
        insert_if_missing(
            &mut properties,
            "endLine",
            Value::from(clamp_line(end_line_0based)),
        );
        bridge.sink.push_node(NodeRec {
            id: node_id(cbm_id, &qualified_name),
            label: cstr(label).unwrap_or_default(),
            properties,
        })
    })
}

extern "C" fn edge_callback(
    userdata: *mut c_void,
    edge_id: i64,
    source_id: i64,
    source_qn: *const c_char,
    target_id: i64,
    target_qn: *const c_char,
    edge_type: *const c_char,
    confidence: c_double,
    reason: *const c_char,
    has_step: c_int,
    step: i64,
    evidence_json: *const c_char,
) -> c_int {
    with_bridge(userdata, |bridge| {
        let evidence = parse_json_value(evidence_json)?;
        bridge.sink.push_edge(EdgeRec {
            id: format!("cbm-edge:{edge_id}"),
            source_id: node_id(source_id, &cstr(source_qn).unwrap_or_default()),
            target_id: node_id(target_id, &cstr(target_qn).unwrap_or_default()),
            edge_type: cstr(edge_type).unwrap_or_default(),
            confidence,
            reason: cstr(reason).unwrap_or_default(),
            step: (has_step != 0).then_some(step.max(0) as u32),
            evidence: Some(evidence),
        })
    })
}

extern "C" fn done_callback(
    userdata: *mut c_void,
    files: c_int,
    nodes: c_int,
    edges: c_int,
) -> c_int {
    with_bridge(userdata, |bridge| {
        bridge.sink.push_done(stats(files, nodes, edges, 0))
    })
}

fn with_bridge(
    userdata: *mut c_void,
    f: impl FnOnce(&mut SinkBridge<'_>) -> Result<(), FactSourceError>,
) -> c_int {
    if userdata.is_null() {
        return 1;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let bridge = unsafe { &mut *(userdata.cast::<SinkBridge<'_>>()) };
        f(bridge)
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => {
            let bridge = unsafe { &mut *(userdata.cast::<SinkBridge<'_>>()) };
            bridge.error = Some(EngineError::Facts(error));
            1
        }
        Err(_) => {
            let bridge = unsafe { &mut *(userdata.cast::<SinkBridge<'_>>()) };
            bridge.error = Some(EngineError::Failed {
                code: None,
                stderr_tail: "panic in embedded engine fact callback".into(),
            });
            1
        }
    }
}

fn ffi_mode(mode: &str) -> AkaEngineMode {
    match mode {
        "full" => AkaEngineMode::Full,
        "moderate" => AkaEngineMode::Moderate,
        _ => AkaEngineMode::Fast,
    }
}

fn cstring_path(path: &Path) -> Result<CString, EngineError> {
    CString::new(path.display().to_string()).map_err(|error| {
        EngineError::Facts(FactSourceError::Message(format!(
            "path contains interior NUL: {error}"
        )))
    })
}

fn cstr(ptr: *const c_char) -> Result<String, FactSourceError> {
    if ptr.is_null() {
        return Ok(String::new());
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map(str::to_string)
        .map_err(|error| {
            FactSourceError::Message(format!("invalid embedded engine utf-8: {error}"))
        })
}

fn parse_json_object(ptr: *const c_char) -> Result<Map<String, Value>, FactSourceError> {
    match parse_json_value(ptr)? {
        Value::Object(map) => Ok(map),
        _ => Ok(Map::new()),
    }
}

fn parse_json_value(ptr: *const c_char) -> Result<Value, FactSourceError> {
    let text = cstr(ptr)?;
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    serde_json::from_str(&text).map_err(|source| FactSourceError::Json { line: 0, source })
}

fn insert_if_missing(props: &mut Map<String, Value>, key: &str, value: Value) {
    props.entry(key.to_string()).or_insert(value);
}

fn node_id(cbm_id: i64, qualified_name: &str) -> String {
    format!("cbm:{cbm_id}:{qualified_name}")
}

fn stats(files: c_int, nodes: c_int, edges: c_int, chunks: c_int) -> FactStats {
    FactStats {
        files: files.max(0) as u64,
        nodes: nodes.max(0) as u64,
        edges: edges.max(0) as u64,
        chunks: chunks.max(0) as u64,
    }
}

fn clamp_line(line: c_int) -> u32 {
    line.max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callbacks_copy_engine_records_into_fact_batch() {
        let mut batch = FactBatchBuilder::new();
        let mut bridge = SinkBridge {
            sink: &mut batch,
            error: None,
        };
        let userdata = (&mut bridge as *mut SinkBridge<'_>).cast::<c_void>();
        let repo = CString::new("/tmp/repo").unwrap();
        let label = CString::new("Function").unwrap();
        let name = CString::new("main").unwrap();
        let qn = CString::new("pkg.main").unwrap();
        let file_path = CString::new("src/lib.rs").unwrap();
        let props = CString::new(r#"{"custom":true}"#).unwrap();
        let edge_type = CString::new("CALLS").unwrap();
        let reason = CString::new("direct").unwrap();
        let evidence = CString::new(r#"{"reason":"direct","confidence":0.9}"#).unwrap();

        assert_eq!(manifest_callback(userdata, repo.as_ptr(), 1, 2, 1), 0);
        assert_eq!(
            node_callback(
                userdata,
                7,
                label.as_ptr(),
                name.as_ptr(),
                qn.as_ptr(),
                file_path.as_ptr(),
                3,
                5,
                props.as_ptr(),
            ),
            0
        );
        assert_eq!(
            edge_callback(
                userdata,
                11,
                7,
                qn.as_ptr(),
                8,
                CString::new("pkg.other").unwrap().as_ptr(),
                edge_type.as_ptr(),
                0.9,
                reason.as_ptr(),
                1,
                4,
                evidence.as_ptr(),
            ),
            0
        );
        assert_eq!(done_callback(userdata, 1, 1, 1), 0);

        drop(bridge);
        let batch = batch.finish();
        assert_eq!(batch.nodes[0].id, "cbm:7:pkg.main");
        assert_eq!(batch.nodes[0].label, "Function");
        assert_eq!(batch.nodes[0].properties["name"], "main");
        assert_eq!(batch.nodes[0].properties["filePath"], "src/lib.rs");
        assert_eq!(batch.nodes[0].properties["startLine"], 3);
        assert_eq!(batch.edges[0].id, "cbm-edge:11");
        assert_eq!(batch.edges[0].source_id, "cbm:7:pkg.main");
        assert_eq!(batch.edges[0].target_id, "cbm:8:pkg.other");
        assert_eq!(batch.edges[0].edge_type, "CALLS");
        assert_eq!(batch.edges[0].step, Some(4));
        assert_eq!(batch.stats.nodes, 1);
    }

    #[test]
    fn callback_parse_errors_are_stored_without_unwinding() {
        let mut batch = FactBatchBuilder::new();
        let mut bridge = SinkBridge {
            sink: &mut batch,
            error: None,
        };
        let userdata = (&mut bridge as *mut SinkBridge<'_>).cast::<c_void>();
        let invalid = CString::new("{").unwrap();

        assert_eq!(
            node_callback(
                userdata,
                1,
                CString::new("Function").unwrap().as_ptr(),
                CString::new("f").unwrap().as_ptr(),
                CString::new("pkg.f").unwrap().as_ptr(),
                CString::new("src/lib.rs").unwrap().as_ptr(),
                0,
                0,
                invalid.as_ptr(),
            ),
            1
        );

        assert!(matches!(bridge.error, Some(EngineError::Facts(_))));
    }

    #[test]
    #[ignore = "links and runs the native embedded engine; use for local smoke"]
    fn embedded_engine_indexes_tiny_repo_without_sqlite_dump() {
        let root = std::env::temp_dir().join(format!(
            "aka-core-embedded-engine-smoke-{}",
            std::process::id()
        ));
        let repo = root.join("repo");
        let cache = root.join("cache");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "pub fn main() {}\n").unwrap();

        let mut sink = FactBatchBuilder::new();
        run_embedded_engine_smoke(&repo, &cache, &mut sink, &mut |_| {}).unwrap();
        let batch = sink.finish();

        assert!(batch.stats.nodes > 0);
        assert!(batch.nodes.iter().any(|node| node.label == "File"));
        assert!(!contains_sqlite_db(&cache));
    }

    fn contains_sqlite_db(path: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(path) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if contains_sqlite_db(&path) {
                    return true;
                }
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("db") {
                return true;
            }
        }
        false
    }
}
