use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, RecvTimeoutError, Sender},
    Arc,
};
use std::thread;
use std::time::Duration;

use aka_facts::{FactBatchBuilder, FactManifest, FactSink, FactSourceError, FactStats};
use chrono::Utc;
use serde_json::{Map, Value};

use crate::types::{EdgeRec, EngineEvent, NodeRec, PipelineProgress, PipelineStage};

use super::fact_producer::normalize_engine_facts;
use super::{emit_phase, engine_cache_root, engine_mode, enrich_direct_fact_batch, EngineError};

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
    deadline_ms_monotonic: u64,
    max_indexing_time_ms: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
#[allow(dead_code)]
enum AkaEngineLogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
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
type ProgressCallback =
    extern "C" fn(*mut c_void, *const c_char, *const c_char, c_int, c_int, c_int, c_int) -> c_int;
type LogCallback = extern "C" fn(*mut c_void, AkaEngineLogLevel, *const c_char);
type ShouldCancelCallback = extern "C" fn(*mut c_void) -> c_int;

#[repr(C)]
struct AkaEngineCallbacks {
    userdata: *mut c_void,
    progress: Option<ProgressCallback>,
    log: Option<LogCallback>,
    should_cancel: Option<ShouldCancelCallback>,
}

#[repr(C)]
struct AkaEngineFactSink {
    userdata: *mut c_void,
    manifest: Option<ManifestCallback>,
    node: Option<NodeCallback>,
    edge: Option<EdgeCallback>,
    done: Option<DoneCallback>,
    callbacks: *const AkaEngineCallbacks,
}

#[cfg(not(windows))]
extern "C" {
    fn aka_engine_index_with_sink(
        options: *const AkaEngineIndexOptions,
        sink: *const AkaEngineFactSink,
    ) -> c_int;
}

#[cfg(windows)]
type AkaEngineIndexWithSink =
    unsafe extern "C" fn(*const AkaEngineIndexOptions, *const AkaEngineFactSink) -> c_int;

#[cfg(not(windows))]
struct EmbeddedEngineApi;

#[cfg(not(windows))]
impl EmbeddedEngineApi {
    fn load() -> Result<Self, EngineError> {
        Ok(Self)
    }

    unsafe fn index_with_sink(
        &self,
        options: *const AkaEngineIndexOptions,
        sink: *const AkaEngineFactSink,
    ) -> c_int {
        aka_engine_index_with_sink(options, sink)
    }
}

#[cfg(windows)]
struct EmbeddedEngineApi {
    _library: libloading::Library,
    index_with_sink: AkaEngineIndexWithSink,
}

#[cfg(windows)]
impl EmbeddedEngineApi {
    fn load() -> Result<Self, EngineError> {
        let dll = resolve_windows_engine_dll()?;
        let library = unsafe { libloading::Library::new(&dll) }.map_err(|error| {
            EngineError::Facts(FactSourceError::Message(format!(
                "load embedded engine dll {} failed: {error}",
                dll.display()
            )))
        })?;
        let index_with_sink = unsafe {
            *library
                .get::<AkaEngineIndexWithSink>(b"aka_engine_index_with_sink\0")
                .map_err(|error| {
                    EngineError::Facts(FactSourceError::Message(format!(
                        "load aka_engine_index_with_sink from {} failed: {error}",
                        dll.display()
                    )))
                })?
        };
        Ok(Self {
            _library: library,
            index_with_sink,
        })
    }

    unsafe fn index_with_sink(
        &self,
        options: *const AkaEngineIndexOptions,
        sink: *const AkaEngineFactSink,
    ) -> c_int {
        (self.index_with_sink)(options, sink)
    }
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
        let cache_root = engine_cache_root(repo, options.cache_dir);
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
                "embedded repo_path={} cache_dir={} mode={} max_secs={}",
                engine_repo.display(),
                cache_root.display(),
                mode_name,
                options
                    .deadline
                    .map(|deadline| deadline.max_secs())
                    .unwrap_or(0)
            ),
        });
        emit_phase(on_event, "aka-engine:index:embedded", 0, 0);

        let mut batch = run_embedded_engine_on_large_stack(
            engine_repo.clone(),
            cache_root,
            mode,
            options.deadline,
            on_event,
        )?;
        if let Some(deadline) = options.deadline {
            deadline.check("facts:normalize")?;
        }

        normalize_engine_facts(
            &mut batch,
            &engine_repo,
            options.no_chunks,
            options.deadline,
            on_event,
        )?;
        if let Some(deadline) = options.deadline {
            deadline.check("enrichment:direct-facts")?;
        }
        enrich_direct_fact_batch(&engine_repo, &mut batch, options.deadline, on_event)?;
        if let Some(deadline) = options.deadline {
            deadline.check("engine:emit")?;
        }
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
            no_chunks: false,
            deadline: None,
        },
        sink,
        on_event,
    )
}

fn run_embedded_engine_on_large_stack(
    engine_repo: PathBuf,
    cache_root: PathBuf,
    mode: AkaEngineMode,
    deadline: Option<super::IndexingDeadline>,
    on_event: &mut dyn FnMut(&EngineEvent),
) -> Result<aka_facts::FactBatch, EngineError> {
    let (tx, rx) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = thread::Builder::new()
        .name("aka-embedded-engine".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let result = run_embedded_engine_inner(
                &engine_repo,
                &cache_root,
                mode,
                deadline,
                tx,
                worker_cancelled,
            );
            EngineWorkerMessage::Done(result)
        })
        .map_err(|error| {
            EngineError::Facts(FactSourceError::Message(format!(
                "spawn embedded engine thread failed: {error}"
            )))
        })?;

    let mut timeout_reported = false;
    loop {
        if let Some(deadline) = deadline {
            if deadline.is_expired() {
                cancelled.store(true, Ordering::Relaxed);
                if !timeout_reported {
                    timeout_reported = true;
                    on_event(&EngineEvent::Progress {
                        progress: PipelineProgress::new(
                            PipelineStage::Timeout,
                            "Indexing deadline reached; waiting for embedded engine to stop",
                        ),
                    });
                }
            }
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(EngineWorkerMessage::Event(event)) => on_event(&event),
            Ok(EngineWorkerMessage::Done(result)) => {
                let join_result = handle.join().map_err(|_| EngineError::Failed {
                    code: None,
                    stderr_tail: "panic in embedded engine thread".into(),
                })?;
                drop(join_result);
                return result;
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(deadline) = deadline {
                    if deadline.is_expired() {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                let done = handle.join().map_err(|_| EngineError::Failed {
                    code: None,
                    stderr_tail: "panic in embedded engine thread".into(),
                })?;
                return match done {
                    EngineWorkerMessage::Done(result) => result,
                    EngineWorkerMessage::Event(_) => Err(EngineError::Failed {
                        code: None,
                        stderr_tail: "embedded engine thread exited without result".into(),
                    }),
                };
            }
        }
    }
}

fn run_embedded_engine_inner(
    engine_repo: &Path,
    cache_root: &Path,
    mode: AkaEngineMode,
    deadline: Option<super::IndexingDeadline>,
    events: Sender<EngineWorkerMessage>,
    cancelled: Arc<AtomicBool>,
) -> Result<aka_facts::FactBatch, EngineError> {
    if let Some(deadline) = deadline {
        deadline.check("engine:parse")?;
    }
    let api = EmbeddedEngineApi::load()?;
    let max_indexing_time_ms = deadline
        .map(|deadline| deadline.remaining().as_millis() as u64)
        .unwrap_or(0);
    let repo_c = cstring_path(engine_repo)?;
    let cache_c = cstring_path(cache_root)?;
    let mut batch = FactBatchBuilder::new();
    let mut event_bridge = EventBridge {
        events: events.clone(),
        deadline,
        cancelled,
    };
    let ffi_callbacks = AkaEngineCallbacks {
        userdata: (&mut event_bridge as *mut EventBridge).cast::<c_void>(),
        progress: Some(progress_callback),
        log: Some(log_callback),
        should_cancel: Some(should_cancel_callback),
    };
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
        callbacks: &ffi_callbacks,
    };
    let ffi_options = AkaEngineIndexOptions {
        repo_path: repo_c.as_ptr(),
        cache_dir: cache_c.as_ptr(),
        mode,
        direct_facts_only: true,
        deadline_ms_monotonic: 0,
        max_indexing_time_ms,
    };

    let rc = unsafe { api.index_with_sink(&ffi_options, &ffi_sink) };
    if let Some(error) = bridge.error {
        return Err(error);
    }
    if rc == -2 {
        return Err(timeout_error(deadline, "engine:parse"));
    }
    if rc != 0 {
        return Err(EngineError::Failed {
            code: Some(rc),
            stderr_tail: "embedded engine returned nonzero status".into(),
        });
    }
    Ok(batch.finish())
}

enum EngineWorkerMessage {
    Event(EngineEvent),
    Done(Result<aka_facts::FactBatch, EngineError>),
}

struct EventBridge {
    events: Sender<EngineWorkerMessage>,
    deadline: Option<super::IndexingDeadline>,
    cancelled: Arc<AtomicBool>,
}

extern "C" fn progress_callback(
    userdata: *mut c_void,
    phase: *const c_char,
    file_path: *const c_char,
    current: c_int,
    total: c_int,
    nodes: c_int,
    edges: c_int,
) -> c_int {
    let Some(bridge) = event_bridge(userdata) else {
        return 1;
    };
    if bridge.should_cancel() {
        return 1;
    }
    let phase = cstr_lossy(phase);
    let file_path = cstr_lossy(file_path);
    let mut progress = PipelineProgress::new(
        engine_progress_stage(&phase),
        engine_progress_message(&phase, &file_path),
    )
    .counts(current.max(0) as u64, total.max(0) as u64);
    progress.nodes = nodes.max(0) as u64;
    progress.edges = edges.max(0) as u64;
    let _ = bridge
        .events
        .send(EngineWorkerMessage::Event(EngineEvent::Progress {
            progress,
        }));
    if bridge.should_cancel() {
        return 1;
    }
    0
}

extern "C" fn log_callback(userdata: *mut c_void, level: AkaEngineLogLevel, line: *const c_char) {
    let Some(bridge) = event_bridge(userdata) else {
        return;
    };
    let line = cstr_lossy(line);
    let level = match level {
        AkaEngineLogLevel::Debug => "debug",
        AkaEngineLogLevel::Info => "info",
        AkaEngineLogLevel::Warn => "warn",
        AkaEngineLogLevel::Error => "error",
    };
    let _ = bridge
        .events
        .send(EngineWorkerMessage::Event(EngineEvent::Log {
            stream: "engine:c".into(),
            line: format!("{level}: {line}"),
        }));
}

extern "C" fn should_cancel_callback(userdata: *mut c_void) -> c_int {
    match event_bridge(userdata) {
        Some(bridge) if !bridge.should_cancel() => 0,
        _ => 1,
    }
}

impl EventBridge {
    fn should_cancel(&self) -> bool {
        if self.cancelled.load(Ordering::Relaxed) {
            return true;
        }
        if let Some(deadline) = self.deadline {
            if deadline.is_expired() {
                self.cancelled.store(true, Ordering::Relaxed);
                return true;
            }
        }
        false
    }
}

fn event_bridge<'a>(userdata: *mut c_void) -> Option<&'a EventBridge> {
    if userdata.is_null() {
        return None;
    }
    Some(unsafe { &*(userdata.cast::<EventBridge>()) })
}

fn engine_progress_stage(phase: &str) -> PipelineStage {
    match phase {
        "discover" => PipelineStage::EngineDiscover,
        "facts_manifest" | "facts_nodes" | "facts_edges" | "facts_done" => {
            PipelineStage::EngineEmit
        }
        "structure" | "definitions" | "parallel_extract" | "registry_build"
        | "lsp_cross_prepare" | "parallel_resolve" | "calls" | "usages" | "semantic" | "k8s"
        | "tests" | "githistory" | "decorator_tags" | "configlink" | "route_match"
        | "similarity" | "semantic_edges" | "complexity" => PipelineStage::EngineParse,
        _ => PipelineStage::EngineParse,
    }
}

fn engine_progress_message(phase: &str, file_path: &str) -> String {
    if file_path.is_empty() {
        format!("C engine phase {phase}")
    } else {
        format!("C engine phase {phase}: {file_path}")
    }
}

fn cstr_lossy(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

fn timeout_error(
    deadline: Option<super::IndexingDeadline>,
    stage: impl Into<String>,
) -> EngineError {
    if let Some(deadline) = deadline {
        EngineError::Timeout {
            stage: stage.into(),
            elapsed_secs: deadline.elapsed_secs(),
        }
    } else {
        EngineError::Failed {
            code: Some(-2),
            stderr_tail: "embedded engine cancelled".into(),
        }
    }
}

#[cfg(windows)]
fn resolve_windows_engine_dll() -> Result<PathBuf, EngineError> {
    if let Some(path) = std::env::var_os("AKA_ENGINE_DLL").map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
        return Err(EngineError::Facts(FactSourceError::Message(format!(
            "AKA_ENGINE_DLL does not exist: {}",
            path.display()
        ))));
    }

    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("AKA_ENGINE_LIB_DIR").map(PathBuf::from) {
        candidates.push(dir.join("aka_engine.dll"));
    }
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1) {
            candidates.extend([
                ancestor.join("aka_engine.dll"),
                ancestor.join("engine").join("aka_engine.dll"),
                ancestor
                    .join("resources")
                    .join("engine")
                    .join("aka_engine.dll"),
            ]);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors() {
            candidates.extend([
                ancestor.join("engine").join("aka_engine.dll"),
                ancestor
                    .join("engine")
                    .join("aka-engine-src")
                    .join("build")
                    .join("c")
                    .join("aka_engine.dll"),
            ]);
        }
    }

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            EngineError::Facts(FactSourceError::Message(
                "embedded engine dll not found; set AKA_ENGINE_DLL or AKA_ENGINE_LIB_DIR".into(),
            ))
        })
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
