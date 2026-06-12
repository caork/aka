//! Engine runner backed by codebase-memory-mcp.
//!
//! `aka` still consumes the artifact contract in `docs/contracts/artifacts.md`,
//! but the producer is now the native C codebase-memory indexer instead of the
//! previous parser sidecar.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::Utc;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};
use serde_json::{json, Map, Value};

use crate::types::{ArtifactStats, EdgeRec, EngineEvent, Manifest, NodeRec, CONTRACT_VERSION};

const DEFAULT_CBM_MODE: &str = "fast";
const PROCESS_MAX_STARTS: usize = 200;
const PROCESS_MIN_COUNT: usize = 20;
const PROCESS_MAX_COUNT: usize = 300;
const PROCESS_MAX_STEPS: usize = 10;
const PROCESS_BRANCH_LIMIT: usize = 4;
const PROCESS_MIN_STEPS: usize = 3;
const MIN_SYNTH_COMMUNITY_SIZE: usize = 2;
const MIN_TRACE_CONFIDENCE: f64 = 0.5;
const COMMUNITY_LABEL_PROPAGATION_PASSES: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(
        "codebase-memory-mcp engine not found: {0} (set --engine-dir, AKA_ENGINE_DIR, or AKA_CBM_BIN)"
    )]
    EngineDirMissing(PathBuf),
    #[error("failed to spawn engine ({cmd}): {source}")]
    Spawn {
        cmd: String,
        #[source]
        source: std::io::Error,
    },
    #[error("engine exited with {code:?}; stderr tail:\n{stderr_tail}")]
    Failed {
        code: Option<i32>,
        stderr_tail: String,
    },
    #[error("engine io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("engine sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("engine json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("engine produced no project row in {0}")]
    MissingProject(PathBuf),
}

/// Native codebase-memory-mcp runner.
pub struct EngineRunner {
    engine_dir: PathBuf,
    cbm_bin: PathBuf,
}

impl EngineRunner {
    const DONE_EXIT_GRACE: Duration = Duration::from_secs(5);

    /// `engine_dir` may be a directory containing `codebase-memory-mcp`, a CBM
    /// source checkout with `build/c/codebase-memory-mcp`, or the binary path.
    pub fn new(engine_dir: impl Into<PathBuf>) -> Result<Self, EngineError> {
        let requested = engine_dir.into();
        let cbm_bin = resolve_cbm_binary(&requested)
            .ok_or_else(|| EngineError::EngineDirMissing(requested.clone()))?;
        let engine_dir = if requested.is_file() {
            requested
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            requested
        };
        Ok(Self {
            engine_dir,
            cbm_bin,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.engine_dir
    }

    /// Discover the native CBM engine from explicit path, env, local engine/,
    /// source checkout, or PATH.
    pub fn discover(explicit: Option<&Path>) -> Result<Self, EngineError> {
        if let Some(dir) = explicit {
            return Self::new(dir);
        }
        if let Ok(bin) = std::env::var("AKA_CBM_BIN") {
            return Self::new(PathBuf::from(bin));
        }
        if let Ok(env_dir) = std::env::var("AKA_ENGINE_DIR") {
            return Self::new(PathBuf::from(env_dir));
        }

        let mut candidates: Vec<PathBuf> = vec![
            PathBuf::from("engine"),
            PathBuf::from("/tmp/codebase-memory-mcp-src"),
        ];
        if let Ok(cwd) = std::env::current_dir() {
            candidates.extend(cwd.ancestors().map(|p| p.join("engine")));
        }
        if let Ok(exe) = std::env::current_exe() {
            candidates.extend(exe.ancestors().skip(1).map(|p| p.join("engine")));
        }
        for c in &candidates {
            if resolve_cbm_binary(c).is_some() {
                return Self::new(c.clone());
            }
        }

        if let Some(path_bin) = find_in_path(cbm_exe_name()) {
            return Self::new(path_bin);
        }

        Err(EngineError::EngineDirMissing(
            candidates.last().cloned().unwrap_or_default(),
        ))
    }

    /// Run codebase-memory, convert its SQLite graph into aka artifacts, and
    /// stream progress events to callers.
    pub fn analyze(
        &self,
        repo: &Path,
        out_dir: &Path,
        cache_dir: Option<&Path>,
        no_chunks: bool,
        mut on_event: impl FnMut(&EngineEvent),
    ) -> Result<ArtifactStats, EngineError> {
        std::fs::create_dir_all(out_dir)?;
        let cache_root = cache_dir
            .map(|p| p.join("codebase-memory"))
            .unwrap_or_else(|| out_dir.join(".codebase-memory-cache"));
        std::fs::create_dir_all(&cache_root)?;

        emit_phase(&mut on_event, "codebase-memory:index", 0, 0);
        let mode = cbm_mode();
        let args = json!({
            "repo_path": repo.display().to_string(),
            "mode": mode,
            "persistence": false,
        })
        .to_string();

        let mut cmd = Command::new(&self.cbm_bin);
        cmd.arg("cli")
            .arg("--progress")
            .arg("--json")
            .arg("index_repository")
            .arg(&args)
            .env("CBM_CACHE_DIR", &cache_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let cmd_display = format!(
            "{} cli --progress --json index_repository <args>",
            self.cbm_bin.display()
        );
        let mut child = cmd.spawn().map_err(|source| EngineError::Spawn {
            cmd: cmd_display,
            source,
        })?;

        let stderr = child.stderr.take().expect("piped stderr");
        let (phase_tx, phase_rx) = mpsc::channel();
        let stderr_handle = std::thread::spawn(move || {
            let mut tail: Vec<String> = Vec::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(phase) = parse_cbm_progress_phase(&line) {
                    let _ = phase_tx.send(phase);
                }
                tail.push(line);
                if tail.len() > 40 {
                    tail.remove(0);
                }
            }
            tail.join("\n")
        });

        let stdout = child.stdout.take().expect("piped stdout");
        for line in BufReader::new(stdout).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if line.contains("\"status\":\"error\"") || line.contains("Pipeline failed") {
                let _ = child.kill();
                let status = child.wait()?;
                let stderr_tail = stderr_handle.join().unwrap_or_default();
                return Err(EngineError::Failed {
                    code: status.code(),
                    stderr_tail: join_tail(stderr_tail, line),
                });
            }
            drain_cbm_progress(&phase_rx, &mut on_event);
        }
        drain_cbm_progress(&phase_rx, &mut on_event);

        let status = wait_for_done_exit(&mut child, Self::DONE_EXIT_GRACE)?;
        drain_cbm_progress(&phase_rx, &mut on_event);
        let stderr_tail = stderr_handle.join().unwrap_or_default();
        drain_cbm_progress(&phase_rx, &mut on_event);
        if !status.success() {
            return Err(EngineError::Failed {
                code: status.code(),
                stderr_tail,
            });
        }

        emit_phase(&mut on_event, "codebase-memory:export-artifacts", 0, 0);
        let (project, db_path) = find_single_project_db(&cache_root)?;
        let stats = export_artifacts(repo, out_dir, &db_path, &project, no_chunks, &mut on_event)?;
        let done = EngineEvent::Done {
            stats: stats.clone(),
        };
        on_event(&done);
        Ok(stats)
    }
}

fn cbm_exe_name() -> &'static str {
    if cfg!(windows) {
        "codebase-memory-mcp.exe"
    } else {
        "codebase-memory-mcp"
    }
}

fn resolve_cbm_binary(base: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if base.is_file() {
        candidates.push(base.to_path_buf());
    } else {
        candidates.extend([
            base.join(cbm_exe_name()),
            base.join("bin").join(cbm_exe_name()),
            base.join("build/c").join(cbm_exe_name()),
        ]);
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

fn cbm_mode() -> String {
    match std::env::var("AKA_CBM_MODE") {
        Ok(mode) if matches!(mode.as_str(), "fast" | "moderate" | "full") => mode,
        _ => DEFAULT_CBM_MODE.to_string(),
    }
}

fn emit_phase(
    on_event: &mut impl FnMut(&EngineEvent),
    phase: impl Into<String>,
    current: u64,
    total: u64,
) {
    on_event(&EngineEvent::Phase {
        phase: phase.into(),
        current,
        total,
    });
}

fn drain_cbm_progress(rx: &mpsc::Receiver<String>, on_event: &mut impl FnMut(&EngineEvent)) {
    while let Ok(phase) = rx.try_recv() {
        emit_phase(on_event, phase, 0, 0);
    }
}

fn parse_cbm_progress_phase(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed == "Starting incremental index" {
        return Some("codebase-memory:incremental-index".into());
    }
    if trimmed == "Starting full index" {
        return Some("codebase-memory:full-index".into());
    }
    if trimmed.starts_with('[') {
        return Some(format!("codebase-memory:{trimmed}"));
    }
    if trimmed.starts_with("Discovering files") || trimmed.starts_with("Extracting:") {
        return Some(format!("codebase-memory:{trimmed}"));
    }
    None
}

fn join_tail(stderr_tail: String, stdout_line: String) -> String {
    if stderr_tail.trim().is_empty() {
        stdout_line
    } else {
        format!("{stderr_tail}\n{stdout_line}")
    }
}

fn find_single_project_db(cache_root: &Path) -> Result<(String, PathBuf), EngineError> {
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(cache_root)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("db") {
            candidates.push(path);
        }
    }
    candidates.sort();

    for db_path in candidates {
        let conn = open_cbm_db(&db_path)?;
        let project: Result<String, rusqlite::Error> =
            conn.query_row("SELECT name FROM projects LIMIT 1", [], |row| row.get(0));
        match project {
            Ok(project) => return Ok((project, db_path)),
            Err(rusqlite::Error::QueryReturnedNoRows) => continue,
            Err(err) => return Err(err.into()),
        }
    }

    Err(EngineError::MissingProject(cache_root.to_path_buf()))
}

fn open_cbm_db(path: &Path) -> Result<Connection, rusqlite::Error> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )
}

fn export_artifacts(
    repo: &Path,
    out_dir: &Path,
    db_path: &Path,
    project: &str,
    no_chunks: bool,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<ArtifactStats, EngineError> {
    let conn = open_cbm_db(db_path)?;
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:inspect-db",
        0,
        0,
    );
    let db_counts = ArtifactStats {
        files: count_files(&conn, project)?,
        nodes: count_nodes(&conn, project)?,
        edges: count_edges(&conn, project)?,
        chunks: count_chunkable_nodes(&conn, project)?,
    };

    emit_phase(
        on_event,
        format!(
            "codebase-memory:export-artifacts:synthesize-graph ({} nodes / {} edges)",
            db_counts.nodes, db_counts.edges
        ),
        0,
        0,
    );
    let synth = synthesize_graph_with_progress(&conn, project, repo, on_event)?;

    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:nodes",
        0,
        db_counts.nodes,
    );
    let nodes = export_nodes(
        &conn,
        project,
        &out_dir.join("nodes.ndjson"),
        &synth,
        db_counts.nodes,
        on_event,
    )?;
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:edges",
        0,
        db_counts.edges,
    );
    let edges = export_edges(
        &conn,
        project,
        &out_dir.join("edges.ndjson"),
        &synth,
        db_counts.edges,
        on_event,
    )?;
    let mut stats = ArtifactStats {
        files: db_counts.files,
        nodes,
        edges,
        chunks: 0,
    };
    if no_chunks {
        let _ = std::fs::remove_file(out_dir.join("chunks.ndjson"));
    } else {
        emit_phase(
            on_event,
            "codebase-memory:export-artifacts:chunks",
            0,
            db_counts.chunks,
        );
        stats.chunks = export_chunks(
            &conn,
            project,
            repo,
            &out_dir.join("chunks.ndjson"),
            db_counts.chunks,
            on_event,
        )?;
    }

    emit_phase(on_event, "codebase-memory:export-artifacts:manifest", 0, 0);
    let manifest = Manifest {
        contract_version: CONTRACT_VERSION,
        engine_version: format!("codebase-memory-mcp+aka ({})", db_path.display()),
        repo_path: repo.display().to_string(),
        commit: git_head(repo),
        generated_at: Utc::now().to_rfc3339(),
        stats: stats.clone(),
    };
    let manifest_path = out_dir.join("manifest.json");
    let file = File::create(manifest_path)?;
    serde_json::to_writer_pretty(BufWriter::new(file), &manifest)?;
    Ok(stats)
}

fn count_nodes(conn: &Connection, project: &str) -> Result<u64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) FROM nodes WHERE project = ?1",
        [project],
        |row| row.get(0),
    )
}

fn count_edges(conn: &Connection, project: &str) -> Result<u64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) FROM edges WHERE project = ?1",
        [project],
        |row| row.get(0),
    )
}

fn count_chunkable_nodes(conn: &Connection, project: &str) -> Result<u64, rusqlite::Error> {
    conn.query_row(
        "SELECT COUNT(*) \
         FROM nodes \
         WHERE project = ?1 AND file_path != '' AND label NOT IN ('File','Folder','Project','Package','Module')",
        [project],
        |row| row.get(0),
    )
}

fn count_files(conn: &Connection, project: &str) -> Result<u64, rusqlite::Error> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM file_hashes WHERE project = ?1",
        [project],
        |row| row.get(0),
    )?;
    if count > 0 {
        return Ok(count);
    }
    conn.query_row(
        "SELECT COUNT(DISTINCT file_path) FROM nodes WHERE project = ?1 AND file_path != ''",
        [project],
        |row| row.get(0),
    )
}

fn export_nodes(
    conn: &Connection,
    project: &str,
    path: &Path,
    synth: &SynthGraph,
    total: u64,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<u64, EngineError> {
    let file = File::create(path)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let mut stmt = conn.prepare(
        "SELECT id, label, name, qualified_name, file_path, start_line, end_line, properties \
         FROM nodes WHERE project = ?1 ORDER BY id",
    )?;
    let mut rows = stmt.query([project])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let label = text_col(row, 1)?;
        let mut name = text_col(row, 2)?;
        let qn = text_col(row, 3)?;
        let file_path = text_col(row, 4)?;
        let start_line: i64 = row.get(5)?;
        let end_line: i64 = row.get(6)?;
        let props_text = text_col(row, 7)?;

        let mut properties = parse_props(&props_text);
        if label == "Route" {
            if let Some(route) = route_from_path(&file_path) {
                name = route;
                properties.insert("name".into(), Value::String(name.clone()));
            }
            sanitize_string_array_prop(&mut properties, "responseKeys");
            sanitize_string_array_prop(&mut properties, "errorKeys");
        }
        insert_if_missing(&mut properties, "name", Value::String(name));
        insert_if_missing(&mut properties, "qualifiedName", Value::String(qn.clone()));
        insert_if_missing(&mut properties, "filePath", Value::String(file_path));
        insert_if_missing(
            &mut properties,
            "startLine",
            Value::from(to_artifact_line(start_line)),
        );
        insert_if_missing(
            &mut properties,
            "endLine",
            Value::from(to_artifact_line(end_line)),
        );
        properties.insert("cbmId".into(), Value::from(cbm_id));

        let node = NodeRec {
            id: aka_node_id(cbm_id, &qn),
            label,
            properties,
        };
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
        emit_export_progress(on_event, "nodes", count, total);
    }
    for community in &synth.communities {
        let node = community.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for process in &synth.processes {
        let node = process.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for route in synth.routes.iter().filter(|r| r.emit_node) {
        let node = route.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for tool in synth.tools.iter().filter(|t| t.emit_node) {
        let node = tool.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    emit_export_progress(on_event, "nodes", count, total);
    Ok(count)
}

fn export_edges(
    conn: &Connection,
    project: &str,
    path: &Path,
    synth: &SynthGraph,
    total: u64,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<u64, EngineError> {
    let file = File::create(path)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let mut stmt = conn.prepare(
        "SELECT e.id, e.source_id, e.target_id, e.type, e.properties, \
                s.qualified_name, t.qualified_name \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id \
         JOIN nodes t ON t.id = e.target_id \
         WHERE e.project = ?1 ORDER BY e.id",
    )?;
    let mut rows = stmt.query([project])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let edge_id: i64 = row.get(0)?;
        let source_id: i64 = row.get(1)?;
        let target_id: i64 = row.get(2)?;
        let edge_type = text_col(row, 3)?;
        let props_text = text_col(row, 4)?;
        let source_qn = text_col(row, 5)?;
        let target_qn = text_col(row, 6)?;
        let props = props_value(&props_text);
        let edge = EdgeRec {
            id: format!("cbm-edge:{edge_id}"),
            source_id: aka_node_id(source_id, &source_qn),
            target_id: aka_node_id(target_id, &target_qn),
            edge_type,
            confidence: props
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(1.0),
            reason: props
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("codebase-memory")
                .to_string(),
            step: props.get("step").and_then(Value::as_u64).map(|v| v as u32),
            evidence: if props.is_null() { None } else { Some(props) },
        };
        serde_json::to_writer(&mut out, &edge)?;
        out.write_all(b"\n")?;
        count += 1;
        emit_export_progress(on_event, "edges", count, total);
    }
    for edge in synth
        .communities
        .iter()
        .flat_map(SynthCommunity::edge_recs)
        .chain(synth.processes.iter().flat_map(SynthProcess::edge_recs))
        .chain(synth.routes.iter().flat_map(SynthRoute::edge_recs))
        .chain(synth.tools.iter().flat_map(SynthTool::edge_recs))
    {
        serde_json::to_writer(&mut out, &edge)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    emit_export_progress(on_event, "edges", count, total);
    Ok(count)
}

fn emit_export_progress(
    on_event: &mut impl FnMut(&EngineEvent),
    name: &'static str,
    count: u64,
    total: u64,
) {
    if count == 0 || count.is_multiple_of(1_000) || (total > 0 && count >= total) {
        emit_phase(
            on_event,
            format!("codebase-memory:export-artifacts:{name}"),
            count,
            total,
        );
    }
}

#[derive(Debug, Clone)]
struct SynthNode {
    aka_id: String,
    label: String,
    name: String,
    file_path: String,
    language: String,
    is_exported: bool,
    ast_framework_multiplier: f64,
    ast_framework_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct SynthGraph {
    communities: Vec<SynthCommunity>,
    processes: Vec<SynthProcess>,
    routes: Vec<SynthRoute>,
    tools: Vec<SynthTool>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct CommunityRef {
    id: String,
    label: String,
}

#[derive(Debug, Clone)]
struct SynthCommunity {
    id: String,
    heuristic_label: String,
    cohesion: f64,
    members: Vec<SynthNode>,
}

impl SynthCommunity {
    fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.heuristic_label.clone()));
        properties.insert(
            "heuristicLabel".into(),
            Value::String(self.heuristic_label.clone()),
        );
        properties.insert("cohesion".into(), json!(self.cohesion));
        properties.insert("symbolCount".into(), Value::from(self.members.len() as u64));
        properties.insert(
            "keywords".into(),
            Value::Array(
                community_keywords(&self.members)
                    .into_iter()
                    .map(Value::String)
                    .collect(),
            ),
        );
        properties.insert("enrichedBy".into(), Value::String("heuristic".into()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        NodeRec {
            id: self.id.clone(),
            label: "Community".into(),
            properties,
        }
    }

    fn edge_recs(&self) -> Vec<EdgeRec> {
        self.members
            .iter()
            .map(|member| EdgeRec {
                id: format!("{}:member:{:016x}", self.id, stable_hash(&member.aka_id)),
                source_id: member.aka_id.clone(),
                target_id: self.id.clone(),
                edge_type: "MEMBER_OF".into(),
                confidence: MIN_TRACE_CONFIDENCE,
                reason: "aka community synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "community-membership",
                    "heuristicLabel": self.heuristic_label.clone(),
                    "cohesion": self.cohesion,
                })),
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
struct SynthProcess {
    id: String,
    name: String,
    process_type: String,
    communities: Vec<CommunityRef>,
    steps: Vec<SynthNode>,
}

#[derive(Debug, Clone)]
struct SynthRoute {
    id: String,
    route: String,
    file_path: String,
    emit_node: bool,
    handler_id: Option<String>,
    handler_name: Option<String>,
    middleware: Vec<String>,
    response_keys: Vec<String>,
    error_keys: Vec<String>,
    consumers: Vec<SynthRouteConsumer>,
    process_ids: Vec<String>,
}

#[derive(Debug, Clone)]
struct SynthRouteConsumer {
    node_id: String,
    keys: Vec<String>,
    fetch_count: u32,
}

impl SynthRoute {
    fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.route.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("routeSource".into(), Value::String("source-scan".into()));
        if let Some(handler_id) = &self.handler_id {
            properties.insert("handlerId".into(), Value::String(handler_id.clone()));
        }
        if let Some(handler_name) = &self.handler_name {
            properties.insert("handlerName".into(), Value::String(handler_name.clone()));
        }
        if !self.middleware.is_empty() {
            properties.insert(
                "middleware".into(),
                Value::Array(self.middleware.iter().cloned().map(Value::String).collect()),
            );
        }
        if !self.response_keys.is_empty() {
            properties.insert(
                "responseKeys".into(),
                Value::Array(
                    self.response_keys
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        if !self.error_keys.is_empty() {
            properties.insert(
                "errorKeys".into(),
                Value::Array(self.error_keys.iter().cloned().map(Value::String).collect()),
            );
        }
        NodeRec {
            id: self.id.clone(),
            label: "Route".into(),
            properties,
        }
    }

    fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::new();
        if let Some(handler_id) = &self.handler_id {
            out.push(EdgeRec {
                id: format!("{}:handles:{:016x}", self.id, stable_hash(handler_id)),
                source_id: handler_id.clone(),
                target_id: self.id.clone(),
                edge_type: "HANDLES_ROUTE".into(),
                confidence: 0.65,
                reason: "aka route synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "route-handler",
                    "route": self.route,
                })),
            });
        }
        for consumer in &self.consumers {
            out.push(EdgeRec {
                id: format!(
                    "{}:fetches:{:016x}",
                    self.id,
                    stable_hash(&consumer.node_id)
                ),
                source_id: consumer.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: "FETCHES".into(),
                confidence: 0.6,
                reason: fetch_reason(&consumer.keys, consumer.fetch_count),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "fetch-consumer",
                    "route": self.route,
                    "accessedKeys": consumer.keys,
                    "fetchCount": consumer.fetch_count,
                })),
            });
        }
        for process_id in &self.process_ids {
            out.push(EdgeRec {
                id: format!("{}:entry-process:{:016x}", self.id, stable_hash(process_id)),
                source_id: self.id.clone(),
                target_id: process_id.clone(),
                edge_type: "ENTRY_POINT_OF".into(),
                confidence: 0.55,
                reason: "aka route process linkage".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "route-entry-process",
                    "route": self.route,
                })),
            });
        }
        out
    }
}

#[derive(Debug, Clone)]
struct SynthTool {
    id: String,
    name: String,
    file_path: String,
    emit_node: bool,
    description: String,
    handler_id: Option<String>,
    process_ids: Vec<String>,
}

impl SynthTool {
    fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("toolSource".into(), Value::String("source-scan".into()));
        if !self.description.is_empty() {
            properties.insert(
                "description".into(),
                Value::String(self.description.clone()),
            );
        }
        if let Some(handler_id) = &self.handler_id {
            properties.insert("handlerId".into(), Value::String(handler_id.clone()));
        }
        NodeRec {
            id: self.id.clone(),
            label: "Tool".into(),
            properties,
        }
    }

    fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::new();
        if let Some(handler_id) = &self.handler_id {
            out.push(EdgeRec {
                id: format!("{}:handles:{:016x}", self.id, stable_hash(handler_id)),
                source_id: handler_id.clone(),
                target_id: self.id.clone(),
                edge_type: "HANDLES_TOOL".into(),
                confidence: 0.6,
                reason: "aka tool synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "tool-handler",
                    "tool": self.name,
                })),
            });
        }
        for process_id in &self.process_ids {
            out.push(EdgeRec {
                id: format!("{}:entry-process:{:016x}", self.id, stable_hash(process_id)),
                source_id: self.id.clone(),
                target_id: process_id.clone(),
                edge_type: "ENTRY_POINT_OF".into(),
                confidence: 0.5,
                reason: "aka tool process linkage".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "tool-entry-process",
                    "tool": self.name,
                })),
            });
        }
        out
    }
}

impl SynthProcess {
    fn node_rec(&self) -> NodeRec {
        let entry = self.steps.first().expect("process has entry");
        let terminal = self.steps.last().expect("process has terminal");
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert(
            "processType".into(),
            Value::String(self.process_type.clone()),
        );
        properties.insert(
            "communities".into(),
            Value::Array(
                self.communities
                    .iter()
                    .map(|c| json!({"id": c.id.clone(), "label": c.label.clone()}))
                    .collect(),
            ),
        );
        properties.insert(
            "communityIds".into(),
            Value::Array(
                self.communities
                    .iter()
                    .map(|c| Value::String(c.id.clone()))
                    .collect(),
            ),
        );
        properties.insert(
            "communityLabels".into(),
            Value::Array(
                self.communities
                    .iter()
                    .map(|c| Value::String(c.label.clone()))
                    .collect(),
            ),
        );
        properties.insert("stepCount".into(), Value::from(self.steps.len() as u64));
        properties.insert("entryPointId".into(), Value::String(entry.aka_id.clone()));
        properties.insert("terminalId".into(), Value::String(terminal.aka_id.clone()));
        properties.insert(
            "trace".into(),
            Value::Array(
                self.steps
                    .iter()
                    .map(|step| Value::String(step.aka_id.clone()))
                    .collect(),
            ),
        );
        properties.insert("heuristicLabel".into(), Value::String(self.name.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        NodeRec {
            id: self.id.clone(),
            label: "Process".into(),
            properties,
        }
    }

    fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::with_capacity(self.steps.len() + 1);
        let entry = self.steps.first().expect("process has entry");
        out.push(EdgeRec {
            id: format!("{}:entry", self.id),
            source_id: entry.aka_id.clone(),
            target_id: self.id.clone(),
            edge_type: "ENTRY_POINT_OF".into(),
            confidence: 0.7,
            reason: "aka process synthesis".into(),
            step: None,
            evidence: Some(json!({"source": "aka-cbm-synth", "kind": "entry"})),
        });
        for (idx, step) in self.steps.iter().enumerate() {
            let step_no = (idx + 1) as u32;
            out.push(EdgeRec {
                id: format!("{}:step:{step_no}", self.id),
                source_id: step.aka_id.clone(),
                target_id: self.id.clone(),
                edge_type: "STEP_IN_PROCESS".into(),
                confidence: 0.7,
                reason: "aka process synthesis".into(),
                step: Some(step_no),
                evidence: Some(json!({"source": "aka-cbm-synth", "kind": "call-chain"})),
            });
        }
        out
    }
}

fn synthesize_graph_with_progress(
    conn: &Connection,
    project: &str,
    repo: &Path,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<SynthGraph, EngineError> {
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:native-labels",
        0,
        0,
    );
    let native_communities = has_native_label(conn, project, "Community")?;
    let native_processes = has_native_label(conn, project, "Process")?;
    let native_routes = load_native_app_nodes(conn, project, "Route")?;
    let native_tools = load_native_app_nodes(conn, project, "Tool")?;
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:nodes",
        0,
        0,
    );
    let nodes = load_synth_nodes(conn, project)?;
    if nodes.is_empty() {
        return Ok(SynthGraph::default());
    }
    emit_phase(
        on_event,
        format!(
            "codebase-memory:export-artifacts:synthesize:calls ({} process-step nodes)",
            nodes.len()
        ),
        0,
        0,
    );
    let calls = load_call_graph(conn, project, &nodes)?;

    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:communities",
        0,
        0,
    );
    let communities = if native_communities {
        Vec::new()
    } else {
        synthesize_communities(&nodes, &calls.edges)
    };
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:community-memberships",
        0,
        0,
    );
    let community_memberships = if native_communities {
        load_native_community_memberships(conn, project, &nodes)?
    } else {
        community_memberships_from_synth(&communities)
    };
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:processes",
        0,
        0,
    );
    let processes = if native_processes {
        Vec::new()
    } else {
        synthesize_processes_from_calls(
            &nodes,
            &calls.adjacency,
            &calls.indegree,
            &community_memberships,
        )
    };
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:routes",
        0,
        0,
    );
    let routes = synthesize_routes_from_sources(repo, &nodes, &processes, &native_routes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:tools",
        0,
        0,
    );
    let tools = synthesize_tools_from_sources(repo, &nodes, &processes, &native_tools);

    Ok(SynthGraph {
        communities,
        processes,
        routes,
        tools,
    })
}

#[cfg(test)]
fn synthesize_graph(
    conn: &Connection,
    project: &str,
    repo: &Path,
) -> Result<SynthGraph, EngineError> {
    fn sink(_: &EngineEvent) {}
    synthesize_graph_with_progress(conn, project, repo, &mut sink)
}

fn has_native_label(conn: &Connection, project: &str, label: &str) -> Result<bool, EngineError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND label = ?2",
        [project, label],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[derive(Debug, Clone)]
struct NativeAppNode {
    id: String,
    name: String,
    file_path: String,
}

fn load_native_app_nodes(
    conn: &Connection,
    project: &str,
    label: &str,
) -> Result<Vec<NativeAppNode>, EngineError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, qualified_name, file_path, properties \
         FROM nodes WHERE project = ?1 AND label = ?2 ORDER BY id",
    )?;
    let mut rows = stmt.query([project, label])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let name = text_col(row, 1)?;
        let qn = text_col(row, 2)?;
        let file_path = text_col(row, 3)?;
        let props = parse_props(&text_col(row, 4)?);
        let name = props
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&name)
            .to_string();
        out.push(NativeAppNode {
            id: aka_node_id(cbm_id, &qn),
            name,
            file_path,
        });
    }
    Ok(out)
}

fn load_synth_nodes(
    conn: &Connection,
    project: &str,
) -> Result<BTreeMap<String, SynthNode>, EngineError> {
    let mut nodes = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT id, label, name, qualified_name, file_path, properties \
         FROM nodes WHERE project = ?1 ORDER BY id",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let label = text_col(row, 1)?;
        let name = text_col(row, 2)?;
        let qn = text_col(row, 3)?;
        let file_path = text_col(row, 4)?;
        let props = parse_props(&text_col(row, 5)?);
        if !is_process_step_label(&label) || is_noisy_source_path(&file_path) {
            continue;
        }
        let aka_id = aka_node_id(cbm_id, &qn);
        let language = props
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let ast_framework_multiplier = props
            .get("astFrameworkMultiplier")
            .and_then(Value::as_f64)
            .unwrap_or(1.0);
        let ast_framework_reason = props
            .get("astFrameworkReason")
            .and_then(Value::as_str)
            .map(str::to_string);
        let is_exported = props
            .get("isExported")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                props
                    .get("visibility")
                    .and_then(Value::as_str)
                    .is_some_and(|v| v.eq_ignore_ascii_case("public"))
            });
        nodes.insert(
            aka_id.clone(),
            SynthNode {
                aka_id,
                label,
                name,
                file_path,
                language,
                is_exported,
                ast_framework_multiplier,
                ast_framework_reason,
            },
        );
    }
    Ok(nodes)
}

#[derive(Debug, Clone, Default)]
struct CallGraph {
    adjacency: BTreeMap<String, BTreeSet<String>>,
    indegree: BTreeMap<String, usize>,
    edges: Vec<(String, String)>,
}

fn load_call_graph(
    conn: &Connection,
    project: &str,
    nodes: &BTreeMap<String, SynthNode>,
) -> Result<CallGraph, EngineError> {
    let mut graph = CallGraph::default();
    let mut stmt = conn.prepare(
        "SELECT e.source_id, e.target_id, e.properties, s.qualified_name, t.qualified_name \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id AND s.project = e.project \
         JOIN nodes t ON t.id = e.target_id AND t.project = e.project \
         WHERE e.project = ?1 \
           AND e.type = 'CALLS' \
           AND s.label IN ('Function','Method','Class','Interface','Struct','Enum','Trait','Type') \
           AND t.label IN ('Function','Method','Class','Interface','Struct','Enum','Trait','Type') \
           AND s.file_path != '' \
           AND t.file_path != '' \
         ORDER BY e.id",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let props = props_value(&text_col(row, 2)?);
        if props
            .get("confidence")
            .and_then(Value::as_f64)
            .is_some_and(|confidence| confidence < MIN_TRACE_CONFIDENCE)
        {
            continue;
        }
        let source_qn = text_col(row, 3)?;
        let target_qn = text_col(row, 4)?;
        let source = aka_node_id(source_id, &source_qn);
        let target = aka_node_id(target_id, &target_qn);
        if !nodes.contains_key(&source) || !nodes.contains_key(&target) || source == target {
            continue;
        }
        let inserted = graph
            .adjacency
            .entry(source.clone())
            .or_default()
            .insert(target.clone());
        if inserted {
            *graph.indegree.entry(target.clone()).or_default() += 1;
            graph.edges.push((source, target));
        }
    }
    Ok(graph)
}

fn load_native_community_memberships(
    conn: &Connection,
    project: &str,
    nodes: &BTreeMap<String, SynthNode>,
) -> Result<BTreeMap<String, Vec<CommunityRef>>, EngineError> {
    let mut memberships: BTreeMap<String, BTreeSet<CommunityRef>> = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT e.source_id, s.qualified_name, c.id, c.qualified_name, c.name, c.properties \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id AND s.project = e.project \
         JOIN nodes c ON c.id = e.target_id AND c.project = e.project \
         WHERE e.project = ?1 AND c.label = 'Community' AND e.type = 'MEMBER_OF' \
         ORDER BY e.id",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let source_id: i64 = row.get(0)?;
        let source_qn = text_col(row, 1)?;
        let community_id: i64 = row.get(2)?;
        let community_qn = text_col(row, 3)?;
        let community_name = text_col(row, 4)?;
        let community_props = parse_props(&text_col(row, 5)?);
        let source = aka_node_id(source_id, &source_qn);
        if !nodes.contains_key(&source) {
            continue;
        }
        let id = aka_node_id(community_id, &community_qn);
        let label = community_props
            .get("heuristicLabel")
            .and_then(Value::as_str)
            .or_else(|| community_props.get("label").and_then(Value::as_str))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| {
                if community_name.is_empty() {
                    &community_qn
                } else {
                    &community_name
                }
            })
            .to_string();
        memberships
            .entry(source)
            .or_default()
            .insert(CommunityRef { id, label });
    }
    Ok(memberships
        .into_iter()
        .map(|(id, refs)| (id, refs.into_iter().collect()))
        .collect())
}

fn synthesize_communities(
    nodes: &BTreeMap<String, SynthNode>,
    call_edges: &[(String, String)],
) -> Vec<SynthCommunity> {
    let labels = propagated_community_labels(nodes, call_edges);
    let mut groups: BTreeMap<String, Vec<SynthNode>> = BTreeMap::new();
    for node in nodes.values() {
        groups
            .entry(
                labels
                    .get(&node.aka_id)
                    .cloned()
                    .unwrap_or_else(|| community_key(&node.file_path)),
            )
            .or_default()
            .push(node.clone());
    }
    if groups.is_empty() || nodes.len() < MIN_SYNTH_COMMUNITY_SIZE {
        return Vec::new();
    }

    let mut node_group = BTreeMap::new();
    for (key, members) in &groups {
        for member in members {
            node_group.insert(member.aka_id.clone(), key.clone());
        }
    }

    let mut internal_calls: BTreeMap<String, usize> = BTreeMap::new();
    let mut incident_calls: BTreeMap<String, usize> = BTreeMap::new();
    for (source, target) in call_edges {
        let Some(source_group) = node_group.get(source) else {
            continue;
        };
        let Some(target_group) = node_group.get(target) else {
            continue;
        };
        if source_group == target_group {
            *internal_calls.entry(source_group.clone()).or_default() += 1;
            *incident_calls.entry(source_group.clone()).or_default() += 1;
        } else {
            *incident_calls.entry(source_group.clone()).or_default() += 1;
            *incident_calls.entry(target_group.clone()).or_default() += 1;
        }
    }

    groups
        .into_iter()
        .filter(|(_, members)| members.len() >= MIN_SYNTH_COMMUNITY_SIZE)
        .map(|(key, mut members)| {
            members.sort_by(|a, b| {
                a.file_path
                    .cmp(&b.file_path)
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.aka_id.cmp(&b.aka_id))
            });
            let incident = *incident_calls.get(&key).unwrap_or(&0);
            let internal = *internal_calls.get(&key).unwrap_or(&0);
            let cohesion = if incident == 0 {
                1.0
            } else {
                internal as f64 / incident as f64
            };
            let heuristic_label = community_label(&key, &members);
            SynthCommunity {
                id: format!("community:heuristic:{:016x}", stable_hash(&key)),
                heuristic_label,
                cohesion: round3(cohesion),
                members,
            }
        })
        .collect()
}

fn propagated_community_labels(
    nodes: &BTreeMap<String, SynthNode>,
    call_edges: &[(String, String)],
) -> BTreeMap<String, String> {
    let initial: BTreeMap<String, String> = nodes
        .values()
        .map(|node| (node.aka_id.clone(), community_key(&node.file_path)))
        .collect();
    if call_edges.is_empty() {
        return initial;
    }

    let mut initial_sizes: BTreeMap<String, usize> = BTreeMap::new();
    for label in initial.values() {
        *initial_sizes.entry(label.clone()).or_default() += 1;
    }

    let mut neighbors: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (source, target) in call_edges {
        if !initial.contains_key(source) || !initial.contains_key(target) || source == target {
            continue;
        }
        neighbors
            .entry(source.clone())
            .or_default()
            .insert(target.clone());
        neighbors
            .entry(target.clone())
            .or_default()
            .insert(source.clone());
    }

    let mut labels = initial;
    for _ in 0..COMMUNITY_LABEL_PROPAGATION_PASSES {
        let mut next = labels.clone();
        let mut changed = false;
        for (node_id, current_label) in &labels {
            let Some(node_neighbors) = neighbors.get(node_id) else {
                continue;
            };
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for neighbor in node_neighbors {
                if let Some(label) = labels.get(neighbor) {
                    *counts.entry(label.clone()).or_default() += 1;
                }
            }
            let Some((best_label, best_count)) = counts
                .iter()
                .max_by(|(label_a, count_a), (label_b, count_b)| {
                    count_a.cmp(count_b).then_with(|| label_b.cmp(label_a))
                })
                .map(|(label, count)| (label.clone(), *count))
            else {
                continue;
            };
            let own_count = counts.get(current_label).copied().unwrap_or(0);
            let own_initial_size = initial_sizes.get(current_label).copied().unwrap_or(0);
            let should_adopt = best_label != *current_label
                && best_count > own_count
                && (best_count >= 2 || own_initial_size < MIN_SYNTH_COMMUNITY_SIZE);
            if should_adopt {
                next.insert(node_id.clone(), best_label);
                changed = true;
            }
        }
        labels = next;
        if !changed {
            break;
        }
    }

    labels
}

fn community_memberships_from_synth(
    communities: &[SynthCommunity],
) -> BTreeMap<String, Vec<CommunityRef>> {
    let mut out = BTreeMap::new();
    for community in communities {
        let community_ref = CommunityRef {
            id: community.id.clone(),
            label: community.heuristic_label.clone(),
        };
        for member in &community.members {
            out.entry(member.aka_id.clone())
                .or_insert_with(Vec::new)
                .push(community_ref.clone());
        }
    }
    out
}

fn synthesize_processes_from_calls(
    nodes: &BTreeMap<String, SynthNode>,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    indegree: &BTreeMap<String, usize>,
    community_memberships: &BTreeMap<String, Vec<CommunityRef>>,
) -> Vec<SynthProcess> {
    if adjacency.is_empty() {
        return Vec::new();
    }

    let max_processes = dynamic_process_cap(nodes.len());
    let mut starts = find_entry_points(nodes, adjacency, indegree);
    if starts.is_empty() {
        starts = adjacency.keys().cloned().collect();
        starts.sort_by(|a, b| {
            let na = &nodes[a];
            let nb = &nodes[b];
            na.file_path
                .cmp(&nb.file_path)
                .then_with(|| na.name.cmp(&nb.name))
                .then_with(|| a.cmp(b))
        });
    }
    starts.truncate(PROCESS_MAX_STARTS);

    let mut traces = Vec::new();
    for start in starts {
        traces.extend(trace_from_entry_point(&start, nodes, adjacency));
        if traces.len() >= max_processes * 2 {
            break;
        }
    }
    let mut traces = deduplicate_by_endpoints(deduplicate_traces(traces));
    traces.sort_by_key(|trace| Reverse(trace.len()));
    traces.truncate(max_processes);

    traces
        .into_iter()
        .filter_map(|trace| process_from_trace(&trace, nodes, community_memberships))
        .collect()
}

fn find_entry_points(
    nodes: &BTreeMap<String, SynthNode>,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    indegree: &BTreeMap<String, usize>,
) -> Vec<String> {
    let mut candidates = Vec::new();
    for (id, node) in nodes {
        if !matches!(node.label.as_str(), "Function" | "Method") || is_test_file(&node.file_path) {
            continue;
        }
        let Some(callees) = adjacency.get(id) else {
            continue;
        };
        if callees.is_empty() {
            continue;
        }
        let callers = *indegree.get(id).unwrap_or(&0);
        let score = entry_score(node, callers, callees.len());
        if score > 0.0 {
            candidates.push((id.clone(), score));
        }
    }
    candidates.sort_by(|(a_id, a_score), (b_id, b_score)| {
        b_score.total_cmp(a_score).then_with(|| {
            let a = &nodes[a_id];
            let b = &nodes[b_id];
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a_id.cmp(b_id))
        })
    });
    candidates.into_iter().map(|(id, _)| id).collect()
}

fn trace_from_entry_point(
    entry_id: &str,
    nodes: &BTreeMap<String, SynthNode>,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<Vec<String>> {
    let mut traces = Vec::new();
    let mut queue = VecDeque::new();
    queue.push_back((entry_id.to_string(), vec![entry_id.to_string()]));

    while let Some((current, path)) = queue.pop_front() {
        if traces.len() >= PROCESS_BRANCH_LIMIT * 3 {
            break;
        }
        let callees = adjacency.get(&current);
        if path.len() >= PROCESS_MAX_STEPS || callees.is_none_or(BTreeSet::is_empty) {
            if path.len() >= PROCESS_MIN_STEPS {
                traces.push(path);
            }
            continue;
        }

        let mut ranked: Vec<&String> = callees.expect("checked").iter().collect();
        ranked.sort_by(|a, b| {
            let na = &nodes[*a];
            let nb = &nodes[*b];
            step_score(nb)
                .cmp(&step_score(na))
                .then_with(|| na.file_path.cmp(&nb.file_path))
                .then_with(|| na.name.cmp(&nb.name))
                .then_with(|| a.cmp(b))
        });
        let mut advanced = false;
        for next in ranked.into_iter().take(PROCESS_BRANCH_LIMIT) {
            if path.iter().any(|id| id == next) {
                continue;
            }
            let mut next_path = path.clone();
            next_path.push(next.clone());
            queue.push_back((next.clone(), next_path));
            advanced = true;
        }
        if !advanced && path.len() >= PROCESS_MIN_STEPS {
            traces.push(path);
        }
    }

    traces
}

fn deduplicate_traces(mut traces: Vec<Vec<String>>) -> Vec<Vec<String>> {
    traces.sort_by_key(|trace| Reverse(trace.len()));
    let mut unique: Vec<Vec<String>> = Vec::new();
    for trace in traces {
        if !unique
            .iter()
            .any(|existing| contains_trace(existing, &trace))
        {
            unique.push(trace);
        }
    }
    unique
}

fn deduplicate_by_endpoints(mut traces: Vec<Vec<String>>) -> Vec<Vec<String>> {
    traces.sort_by_key(|trace| Reverse(trace.len()));
    let mut seen_endpoints: BTreeSet<(String, String)> = BTreeSet::new();
    let mut out = Vec::new();
    for trace in traces {
        let (Some(first), Some(last)) = (trace.first(), trace.last()) else {
            continue;
        };
        if seen_endpoints.insert((first.clone(), last.clone())) {
            out.push(trace);
        }
    }
    out
}

fn contains_trace(existing: &[String], candidate: &[String]) -> bool {
    candidate.len() <= existing.len()
        && existing
            .windows(candidate.len())
            .any(|window| window == candidate)
}

fn process_from_trace(
    path: &[String],
    nodes: &BTreeMap<String, SynthNode>,
    community_memberships: &BTreeMap<String, Vec<CommunityRef>>,
) -> Option<SynthProcess> {
    if path.len() < PROCESS_MIN_STEPS {
        return None;
    }
    let key = path.join(">");
    let steps: Vec<SynthNode> = path
        .iter()
        .filter_map(|id| nodes.get(id).cloned())
        .collect();
    if steps.len() < PROCESS_MIN_STEPS {
        return None;
    }
    let entry = steps.first().expect("steps").display_name();
    let terminal = steps.last().expect("steps").display_name();
    let id = format!("process:call-chain:{:016x}", stable_hash(&key));
    let communities = process_communities(path, community_memberships);
    let process_type = if communities.len() > 1 {
        "cross_community"
    } else {
        "intra_community"
    }
    .to_string();
    Some(SynthProcess {
        id,
        name: format!("{entry} → {terminal}"),
        process_type,
        communities,
        steps,
    })
}

fn process_communities(
    path: &[String],
    community_memberships: &BTreeMap<String, Vec<CommunityRef>>,
) -> Vec<CommunityRef> {
    let mut communities = BTreeSet::new();
    for node_id in path {
        if let Some(node_communities) = community_memberships.get(node_id) {
            communities.extend(node_communities.iter().cloned());
        }
    }
    communities.into_iter().collect()
}

fn synthesize_routes_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    processes: &[SynthProcess],
    native_routes: &[NativeAppNode],
) -> Vec<SynthRoute> {
    let mut by_file = web_nodes_by_file(nodes);
    let mut routes: BTreeMap<(String, String), SynthRoute> = BTreeMap::new();
    for native in native_routes {
        let route = route_from_path(&native.file_path)
            .unwrap_or_else(|| normalize_route_literal(trim_route_suffix(&native.name)));
        routes.insert(
            (route.clone(), native.file_path.clone()),
            SynthRoute {
                id: native.id.clone(),
                route,
                file_path: native.file_path.clone(),
                emit_node: false,
                handler_id: None,
                handler_name: None,
                middleware: Vec::new(),
                response_keys: Vec::new(),
                error_keys: Vec::new(),
                consumers: Vec::new(),
                process_ids: process_ids_for_entry(processes, &native.file_path, None),
            },
        );
    }
    for (file_path, file_nodes) in &mut by_file {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        let mut route_names = BTreeSet::new();
        if let Some(route) = route_from_path(file_path) {
            route_names.insert(route);
        }
        route_names.extend(extract_route_handler_literals(&text));
        if route_names.is_empty() {
            continue;
        }
        let handler = pick_handler_node(file_nodes);
        let response_keys = extract_response_keys(&text);
        let error_keys = extract_error_keys(&response_keys, &text);
        let middleware = extract_middleware(&text);
        for route in route_names {
            let key = (route.clone(), file_path.clone());
            match routes.get_mut(&key) {
                Some(existing) => {
                    if existing.handler_id.is_none() {
                        existing.handler_id = handler.map(|n| n.aka_id.clone());
                        existing.handler_name = handler.map(|n| n.display_name().to_string());
                    }
                    merge_strings(&mut existing.middleware, &middleware);
                    merge_strings(&mut existing.response_keys, &response_keys);
                    merge_strings(&mut existing.error_keys, &error_keys);
                    merge_strings(
                        &mut existing.process_ids,
                        &process_ids_for_entry(
                            processes,
                            file_path,
                            handler.map(|n| n.aka_id.as_str()),
                        ),
                    );
                }
                None => {
                    routes.insert(
                        key,
                        SynthRoute {
                            id: format!(
                                "route:heuristic:{:016x}",
                                stable_hash(&format!("{route}|{file_path}"))
                            ),
                            route,
                            file_path: file_path.clone(),
                            emit_node: true,
                            handler_id: handler.map(|n| n.aka_id.clone()),
                            handler_name: handler.map(|n| n.display_name().to_string()),
                            middleware: middleware.clone(),
                            response_keys: response_keys.clone(),
                            error_keys: error_keys.clone(),
                            consumers: Vec::new(),
                            process_ids: process_ids_for_entry(
                                processes,
                                file_path,
                                handler.map(|n| n.aka_id.as_str()),
                            ),
                        },
                    );
                }
            }
        }
    }

    attach_route_consumers(repo, nodes, &mut routes);

    let mut out: Vec<SynthRoute> = routes.into_values().collect();
    out.sort_by(|a, b| {
        a.route
            .cmp(&b.route)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    out
}

fn synthesize_tools_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    processes: &[SynthProcess],
    native_tools: &[NativeAppNode],
) -> Vec<SynthTool> {
    let mut by_file = web_nodes_by_file(nodes);
    let mut tools: BTreeMap<(String, String), SynthTool> = BTreeMap::new();
    for native in native_tools {
        tools.insert(
            (native.name.clone(), native.file_path.clone()),
            SynthTool {
                id: native.id.clone(),
                name: native.name.clone(),
                file_path: native.file_path.clone(),
                emit_node: false,
                description: String::new(),
                handler_id: None,
                process_ids: process_ids_for_entry(processes, &native.file_path, None),
            },
        );
    }
    for (file_path, file_nodes) in &mut by_file {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        let defs = extract_tool_defs(&text);
        if defs.is_empty() {
            continue;
        }
        let handler = pick_handler_node(file_nodes);
        for def in defs {
            let key = (def.name.clone(), file_path.clone());
            match tools.get_mut(&key) {
                Some(existing) => {
                    if existing.description.is_empty() {
                        existing.description = def.description;
                    }
                    if existing.handler_id.is_none() {
                        existing.handler_id = handler.map(|n| n.aka_id.clone());
                    }
                    merge_strings(
                        &mut existing.process_ids,
                        &process_ids_for_entry(
                            processes,
                            file_path,
                            handler.map(|n| n.aka_id.as_str()),
                        ),
                    );
                }
                None => {
                    tools.insert(
                        key,
                        SynthTool {
                            id: format!(
                                "tool:heuristic:{:016x}",
                                stable_hash(&format!("{}|{file_path}", def.name))
                            ),
                            name: def.name,
                            file_path: file_path.clone(),
                            emit_node: true,
                            description: def.description,
                            handler_id: handler.map(|n| n.aka_id.clone()),
                            process_ids: process_ids_for_entry(
                                processes,
                                file_path,
                                handler.map(|n| n.aka_id.as_str()),
                            ),
                        },
                    );
                }
            }
        }
    }
    let mut out: Vec<SynthTool> = tools.into_values().collect();
    out.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    out
}

fn nodes_by_file(nodes: &BTreeMap<String, SynthNode>) -> BTreeMap<String, Vec<&SynthNode>> {
    let mut by_file: BTreeMap<String, Vec<&SynthNode>> = BTreeMap::new();
    for node in nodes.values() {
        if node.file_path.is_empty() || is_noisy_source_path(&node.file_path) {
            continue;
        }
        by_file
            .entry(node.file_path.clone())
            .or_default()
            .push(node);
    }
    for file_nodes in by_file.values_mut() {
        file_nodes.sort_by(|a, b| {
            handler_rank(a)
                .cmp(&handler_rank(b))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.aka_id.cmp(&b.aka_id))
        });
    }
    by_file
}

fn web_nodes_by_file(nodes: &BTreeMap<String, SynthNode>) -> BTreeMap<String, Vec<&SynthNode>> {
    nodes_by_file(nodes)
        .into_iter()
        .filter(|(file_path, file_nodes)| {
            is_js_ts_source_path(file_path)
                || file_nodes
                    .iter()
                    .any(|node| is_js_ts_language(&node.language))
        })
        .collect()
}

fn is_js_ts_source_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    matches!(
        Path::new(&lower).extension().and_then(|ext| ext.to_str()),
        Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts")
    )
}

fn is_js_ts_language(language: &str) -> bool {
    matches!(
        language.to_ascii_lowercase().as_str(),
        "javascript" | "typescript" | "tsx" | "jsx"
    )
}

fn read_repo_text(repo: &Path, file_path: &str) -> Option<String> {
    std::fs::read_to_string(repo.join(file_path)).ok()
}

fn pick_handler_node<'a>(nodes: &'a [&'a SynthNode]) -> Option<&'a SynthNode> {
    nodes
        .iter()
        .copied()
        .find(|n| matches!(n.label.as_str(), "Function" | "Method") && handler_rank(n) <= 1)
        .or_else(|| {
            nodes
                .iter()
                .copied()
                .find(|n| matches!(n.label.as_str(), "Function" | "Method"))
        })
        .or_else(|| nodes.first().copied())
}

fn handler_rank(node: &SynthNode) -> u8 {
    let lower = node.name.to_ascii_lowercase();
    if lower == "handler" || lower == "handle" || lower.starts_with("handle") {
        0
    } else if matches!(
        lower.as_str(),
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options"
    ) || lower.ends_with("handler")
        || lower.ends_with("controller")
    {
        1
    } else if matches!(node.label.as_str(), "Function" | "Method") {
        2
    } else {
        3
    }
}

fn route_from_path(file_path: &str) -> Option<String> {
    let normalized = file_path.replace('\\', "/");
    let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if let Some(idx) = find_segment_pair(&segments, "pages", "api") {
        return route_from_segments(&segments[idx + 2..], true);
    }
    if let Some(idx) = find_segment_pair(&segments, "app", "api") {
        return route_from_segments(&segments[idx + 2..], true);
    }
    for marker in ["routes", "controllers", "handlers"] {
        if let Some(idx) = segments.iter().position(|s| s.eq_ignore_ascii_case(marker)) {
            if let Some(route) = route_from_segments(&segments[idx + 1..], true) {
                return Some(route);
            }
        }
    }
    None
}

fn find_segment_pair(segments: &[&str], a: &str, b: &str) -> Option<usize> {
    segments
        .windows(2)
        .position(|w| w[0].eq_ignore_ascii_case(a) && w[1].eq_ignore_ascii_case(b))
}

fn route_from_segments(segments: &[&str], api_prefix: bool) -> Option<String> {
    let mut parts = Vec::new();
    for segment in segments {
        let stem = file_stem_label(segment);
        if matches!(
            stem.as_str(),
            "route" | "index" | "page" | "layout" | "handler" | "controller"
        ) {
            continue;
        }
        let part = stem
            .trim_start_matches('[')
            .trim_end_matches(']')
            .trim_start_matches("...")
            .trim_start_matches('$');
        if part.is_empty() {
            continue;
        }
        let part = if stem.starts_with('[') || stem.starts_with('$') {
            format!(":{part}")
        } else {
            part.to_string()
        };
        parts.push(part);
    }
    if parts.is_empty() && !api_prefix {
        return None;
    }
    let body = parts.join("/");
    if api_prefix {
        if body.is_empty() {
            Some("/api".into())
        } else {
            Some(format!("/api/{body}"))
        }
    } else if body.is_empty() {
        None
    } else {
        Some(format!("/{body}"))
    }
}

fn extract_route_handler_literals(text: &str) -> BTreeSet<String> {
    let mut routes = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if !is_ident_start(bytes[i] as char) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        let word = &text[start..i];
        let lower = word.to_ascii_lowercase();
        if !matches!(
            lower.as_str(),
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options" | "all" | "route"
        ) {
            continue;
        }
        let mut j = skip_ws(text, i);
        if j < bytes.len() && bytes[j] == b'(' {
            j = skip_ws(text, j + 1);
            if let Some((literal, _end)) = read_string_literal(text, j) {
                if literal.starts_with('/') && !literal.starts_with("//") {
                    routes.insert(normalize_route_literal(&literal));
                }
            }
        }
    }
    routes
}

fn attach_route_consumers(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    routes: &mut BTreeMap<(String, String), SynthRoute>,
) {
    if routes.is_empty() {
        return;
    }
    let by_file = web_nodes_by_file(nodes);
    let route_names: Vec<String> = routes.keys().map(|(route, _)| route.clone()).collect();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        let Some(consumer) = pick_handler_node(&file_nodes) else {
            continue;
        };
        for (route, occurrences) in route_fetch_counts(&text, &route_names) {
            let keys = extract_accessed_keys_near_route(&text, route);
            for candidate in routes.values_mut().filter(|r| &r.route == route) {
                if candidate.file_path == file_path {
                    continue;
                }
                candidate.consumers.push(SynthRouteConsumer {
                    node_id: consumer.aka_id.clone(),
                    keys: keys.clone(),
                    fetch_count: occurrences,
                });
            }
        }
    }
    for route in routes.values_mut() {
        route.consumers.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        route.consumers.dedup_by(|a, b| {
            if a.node_id == b.node_id {
                b.fetch_count = b.fetch_count.saturating_add(a.fetch_count);
                b.keys.extend(a.keys.clone());
                b.keys.sort();
                b.keys.dedup();
                true
            } else {
                false
            }
        });
    }
}

fn route_fetch_counts<'a>(text: &str, route_names: &'a [String]) -> Vec<(&'a String, u32)> {
    if route_names.is_empty() {
        return Vec::new();
    }
    let fetch_windows = fetch_literal_windows(text);
    if fetch_windows.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for route in route_names {
        let mut count = 0u32;
        for window in &fetch_windows {
            count = count.saturating_add(window.matches(route.as_str()).count() as u32);
        }
        if count > 0 {
            out.push((route, count));
        }
    }
    out
}

fn fetch_literal_windows(text: &str) -> Vec<&str> {
    let mut windows = Vec::new();
    for marker in ["fetch(", "axios.", ".get(", ".post(", "http.", "client."] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(marker) {
            let start = offset + pos;
            let end = (start + 600).min(text.len());
            windows.push(&text[start..end]);
            offset = start + marker.len();
        }
    }
    windows
}

fn literal_occurrences(text: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(needle) {
        let idx = offset + pos;
        out.push(idx);
        offset = idx + needle.len();
    }
    out
}

fn extract_accessed_keys_near_route(text: &str, route: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for idx in literal_occurrences(text, route) {
        let end = (idx + 2000).min(text.len());
        let window = &text[idx..end];
        for key in dotted_property_names(window) {
            if !is_common_property(&key) {
                keys.insert(key);
            }
        }
    }
    keys.into_iter().take(16).collect()
}

fn dotted_property_names(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] != b'.' || !is_ident_start(bytes[i + 1] as char) {
            i += 1;
            continue;
        }
        let start = i + 1;
        i = start + 1;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        out.push(text[start..i].to_string());
    }
    out
}

fn is_common_property(key: &str) -> bool {
    matches!(
        key,
        "then"
            | "catch"
            | "finally"
            | "json"
            | "text"
            | "ok"
            | "status"
            | "headers"
            | "map"
            | "filter"
            | "reduce"
            | "length"
            | "push"
            | "slice"
            | "data"
    )
}

fn extract_response_keys(text: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for marker in [".json(", "json(", "return "] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(marker) {
            let idx = offset + pos + marker.len();
            let idx = skip_ws(text, idx);
            if text.as_bytes().get(idx) == Some(&b'{') {
                if let Some(body) = balanced_brace_body(text, idx) {
                    keys.extend(top_level_object_keys(body));
                }
            }
            offset = idx.saturating_add(1);
        }
    }
    keys.into_iter().take(32).collect()
}

fn extract_error_keys(response_keys: &[String], text: &str) -> Vec<String> {
    let mut keys: BTreeSet<String> = response_keys
        .iter()
        .filter(|key| matches!(key.as_str(), "error" | "errors" | "message" | "code"))
        .cloned()
        .collect();
    let lower = text.to_ascii_lowercase();
    for key in ["error", "errors", "message", "code"] {
        if lower.contains(key) {
            keys.insert(key.to_string());
        }
    }
    keys.into_iter().collect()
}

fn extract_middleware(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for word in ident_words(text) {
        if word.starts_with("with") && word.len() > 4 {
            out.insert(word);
        }
    }
    for name in ["auth", "requireAuth", "rateLimit", "cors", "csrf"] {
        if text.contains(name) {
            out.insert(name.to_string());
        }
    }
    out.into_iter().take(12).collect()
}

#[derive(Debug)]
struct ToolDef {
    name: String,
    description: String,
}

fn extract_tool_defs(text: &str) -> Vec<ToolDef> {
    let mut tools: BTreeMap<String, ToolDef> = BTreeMap::new();
    for marker in [".tool(", "server.tool(", "tool("] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(marker) {
            let idx = offset + pos + marker.len();
            let idx = skip_ws(text, idx);
            if let Some((name, end)) = read_string_literal(text, idx) {
                if is_plausible_tool_name(&name) {
                    let desc = extract_description_near(text, end);
                    tools.entry(name.clone()).or_insert(ToolDef {
                        name,
                        description: desc,
                    });
                }
                offset = end;
            } else {
                offset = idx.saturating_add(1);
            }
        }
    }
    for idx in property_name_offsets(text, "name") {
        let window_start = idx.saturating_sub(240);
        let window_end = (idx + 400).min(text.len());
        let window = &text[window_start..window_end];
        let lower = window.to_ascii_lowercase();
        if !(lower.contains("tool") || lower.contains("inputschema") || lower.contains("schema")) {
            continue;
        }
        let value_start = skip_ws(text, idx + "name".len());
        let value_start = if text.as_bytes().get(value_start) == Some(&b':') {
            skip_ws(text, value_start + 1)
        } else {
            continue;
        };
        if let Some((name, end)) = read_string_literal(text, value_start) {
            if is_plausible_tool_name(&name) {
                let desc = extract_description_near(text, end);
                tools.entry(name.clone()).or_insert(ToolDef {
                    name,
                    description: desc,
                });
            }
        }
    }
    tools.into_values().collect()
}

fn extract_description_near(text: &str, idx: usize) -> String {
    let start = idx.saturating_sub(120);
    let end = (idx + 600).min(text.len());
    let window = &text[start..end];
    for key in ["description", "title"] {
        if let Some(pos) = window.find(key) {
            let colon = skip_ws(window, pos + key.len());
            let value_start = if window.as_bytes().get(colon) == Some(&b':') {
                skip_ws(window, colon + 1)
            } else {
                continue;
            };
            if let Some((desc, _)) = read_string_literal(window, value_start) {
                return desc.chars().take(240).collect();
            }
        }
    }
    String::new()
}

fn is_plausible_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/'))
        && name.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn process_ids_for_entry(
    processes: &[SynthProcess],
    file_path: &str,
    handler_id: Option<&str>,
) -> Vec<String> {
    let mut ids = BTreeSet::new();
    for process in processes {
        let touches_handler =
            handler_id.is_some_and(|id| process.steps.iter().any(|step| step.aka_id == id));
        let touches_file = process.steps.iter().any(|step| step.file_path == file_path);
        if touches_handler || touches_file {
            ids.insert(process.id.clone());
        }
    }
    ids.into_iter().collect()
}

fn fetch_reason(keys: &[String], fetch_count: u32) -> String {
    let mut out = String::from("fetch-url-match");
    if !keys.is_empty() {
        out.push_str("|keys:");
        out.push_str(&keys.join(","));
    }
    out.push_str("|fetches:");
    out.push_str(&fetch_count.max(1).to_string());
    out
}

fn normalize_route_literal(route: &str) -> String {
    let mut out = route.trim().to_string();
    if out.len() > 1 {
        while out.ends_with('/') {
            out.pop();
        }
    }
    out
}

fn trim_route_suffix(route: &str) -> &str {
    route
        .strip_suffix("/route")
        .or_else(|| route.strip_suffix("/index"))
        .unwrap_or(route)
}

fn merge_strings(target: &mut Vec<String>, source: &[String]) {
    target.extend(source.iter().cloned());
    target.sort();
    target.dedup();
}

fn balanced_brace_body(text: &str, open_idx: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.get(open_idx) != Some(&b'{') {
        return None;
    }
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    let mut i = open_idx;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' | b'`' => quote = Some(b),
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open_idx + 1..i]);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn top_level_object_keys(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut keys = Vec::new();
    let mut i = 0usize;
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' | b'`' if depth > 0 => quote = Some(b),
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => depth -= 1,
            b'\'' | b'"' if depth == 0 => {
                if let Some((key, end)) = read_string_literal(body, i) {
                    let after = skip_ws(body, end);
                    if body.as_bytes().get(after) == Some(&b':') && is_object_key(&key) {
                        keys.push(key);
                    }
                    i = after.saturating_add(1);
                    continue;
                }
            }
            _ if depth == 0 && is_ident_start(b as char) => {
                let start = i;
                i += 1;
                while i < bytes.len() && is_ident_continue(bytes[i] as char) {
                    i += 1;
                }
                let key = &body[start..i];
                let after = skip_ws(body, i);
                if body.as_bytes().get(after) == Some(&b':') && is_object_key(key) {
                    keys.push(key.to_string());
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    keys.sort();
    keys.dedup();
    keys
}

fn is_object_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '$'))
}

fn ident_words(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if !is_ident_start(bytes[i] as char) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        out.push(text[start..i].to_string());
    }
    out
}

fn property_name_offsets(text: &str, name: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i + name.len() <= bytes.len() {
        if &text[i..i + name.len()] == name {
            let before = i
                .checked_sub(1)
                .and_then(|idx| text.as_bytes().get(idx))
                .copied()
                .map(char::from);
            let after = text.as_bytes().get(i + name.len()).copied().map(char::from);
            if before.is_none_or(|ch| !is_ident_continue(ch))
                && after.is_none_or(|ch| !is_ident_continue(ch))
            {
                out.push(i);
            }
            i += name.len();
        } else {
            i += 1;
        }
    }
    out
}

fn read_string_literal(text: &str, start: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let quote = *bytes.get(start)?;
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut out = String::new();
    let mut escape = false;
    let mut i = start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            out.push(b as char);
            escape = false;
        } else if b == b'\\' {
            escape = true;
        } else if b == quote {
            return Some((out, i + 1));
        } else {
            out.push(b as char);
        }
        i += 1;
    }
    None
}

fn skip_ws(text: &str, mut idx: usize) -> usize {
    let bytes = text.as_bytes();
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || matches!(ch, '_' | '$')
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$')
}

impl SynthNode {
    fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.aka_id
        } else {
            &self.name
        }
    }
}

fn dynamic_process_cap(symbol_count: usize) -> usize {
    (symbol_count / 10)
        .clamp(PROCESS_MIN_COUNT, PROCESS_MAX_COUNT)
        .max(PROCESS_MIN_COUNT)
}

fn entry_score(node: &SynthNode, caller_count: usize, callee_count: usize) -> f64 {
    if callee_count == 0 {
        return 0.0;
    }
    let base_score = callee_count as f64 / (caller_count as f64 + 1.0);
    let export_multiplier = if node.is_exported { 2.0 } else { 1.0 };
    let name_multiplier = if is_utility_name(&node.name) {
        0.3
    } else if is_entry_name(&node.name, &node.language) {
        1.5
    } else {
        1.0
    };
    let framework_multiplier = framework_multiplier_from_path(&node.file_path);
    let ast_multiplier = if node.ast_framework_reason.is_some() {
        node.ast_framework_multiplier.max(1.0)
    } else {
        node.ast_framework_multiplier
    };
    base_score * export_multiplier * name_multiplier * framework_multiplier * ast_multiplier
}

fn is_hard_entry_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "main" | "start" | "run" | "init" | "bootstrap"
    )
}

fn is_entry_name(name: &str, language: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    is_hard_entry_name(&lower)
        || lower.starts_with("handle")
        || lower.starts_with("process")
        || lower.starts_with("execute")
        || lower.starts_with("perform")
        || lower.starts_with("dispatch")
        || lower.starts_with("trigger")
        || lower.starts_with("fire")
        || lower.starts_with("emit")
        || lower.starts_with("on")
        || lower.ends_with("handler")
        || lower.ends_with("controller")
        || language_entry_name(&lower, language)
}

fn language_entry_name(lower_name: &str, language: &str) -> bool {
    match language {
        "python" => lower_name == "__main__" || lower_name.starts_with("view_"),
        "go" => lower_name == "init" || lower_name == "servehttp",
        "java" | "kotlin" | "csharp" => {
            lower_name == "main"
                || lower_name.ends_with("controller")
                || lower_name.ends_with("handler")
        }
        "rust" => lower_name == "main" || lower_name.starts_with("run_"),
        _ => false,
    }
}

fn is_utility_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with('_')
        || lower.starts_with("get")
        || lower.starts_with("set")
        || lower.starts_with("is")
        || lower.starts_with("has")
        || lower.starts_with("can")
        || lower.starts_with("should")
        || lower.starts_with("format")
        || lower.starts_with("parse")
        || lower.starts_with("validate")
        || lower.starts_with("convert")
        || lower.starts_with("transform")
        || lower.starts_with("to")
        || lower.starts_with("from")
        || lower.starts_with("encode")
        || lower.starts_with("decode")
        || lower.starts_with("serialize")
        || lower.starts_with("deserialize")
        || lower.starts_with("clone")
        || lower.starts_with("copy")
        || lower.starts_with("merge")
        || lower.starts_with("filter")
        || lower.starts_with("map")
        || lower.starts_with("reduce")
        || matches!(
            lower.as_str(),
            "log" | "debug" | "error" | "warn" | "info" | "utils" | "helpers"
        )
        || lower.ends_with("helper")
        || lower.ends_with("util")
        || lower.ends_with("utils")
}

fn framework_multiplier_from_path(path: &str) -> f64 {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    if p.contains("/pages/api/")
        || p.contains("/app/api/")
        || p.contains("/routes/")
        || p.contains("/controllers/")
        || p.contains("/handlers/")
        || p.contains("/views/")
        || p.ends_with("controller.ts")
        || p.ends_with("controller.js")
        || p.ends_with("controller.py")
        || p.ends_with("handler.ts")
        || p.ends_with("handler.js")
        || p.ends_with("handler.py")
    {
        2.0
    } else if is_utility_file(&p) {
        0.6
    } else {
        1.0
    }
}

fn is_test_file(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    p.contains(".test.")
        || p.contains(".spec.")
        || p.contains("__tests__/")
        || p.contains("__mocks__/")
        || p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/testing/")
        || p.ends_with("_test.py")
        || p.contains("/test_")
        || p.ends_with("_test.go")
        || p.contains("/src/test/")
        || p.ends_with("tests.swift")
        || p.ends_with("test.swift")
        || p.contains("uitests/")
        || p.ends_with("tests.cs")
        || p.ends_with("test.cs")
        || p.contains(".tests/")
        || p.contains(".test/")
        || p.ends_with("test.php")
        || p.ends_with("spec.php")
        || p.ends_with("_spec.rb")
        || p.ends_with("_test.rb")
        || p.contains("/spec/")
}

fn is_utility_file(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    p.contains("/utils/")
        || p.contains("/util/")
        || p.contains("/helpers/")
        || p.contains("/helper/")
        || p.contains("/common/")
        || p.contains("/shared/")
        || p.ends_with("/utils.ts")
        || p.ends_with("/utils.js")
        || p.ends_with("/helpers.ts")
        || p.ends_with("/helpers.js")
        || p.ends_with("_utils.py")
        || p.ends_with("_helpers.py")
}

fn step_score(node: &SynthNode) -> i32 {
    match node.label.as_str() {
        "Function" | "Method" => 30,
        "Class" | "Interface" | "Struct" | "Trait" => 15,
        _ => 0,
    }
}

fn is_process_step_label(label: &str) -> bool {
    matches!(
        label,
        "Function" | "Method" | "Class" | "Interface" | "Struct" | "Enum" | "Trait" | "Type"
    )
}

fn is_noisy_source_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    path.split('/').any(|segment| {
        matches!(
            segment,
            "node_modules"
                | "vendor"
                | "vendors"
                | "dist"
                | "build"
                | "target"
                | "coverage"
                | "__pycache__"
                | ".venv"
                | "venv"
                | "generated"
                | "third_party"
                | "third-party"
        )
    }) || path.ends_with(".min.js")
}

fn community_key(file_path: &str) -> String {
    let path = file_path.replace('\\', "/");
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    match segments.as_slice() {
        [] => "(unknown)".into(),
        [only] => file_stem_label(only),
        [root, file] if looks_like_file(file) => (*root).to_string(),
        [root, name, ..]
            if matches!(
                *root,
                "apps" | "crates" | "libs" | "packages" | "services" | "tools"
            ) =>
        {
            format!("{root}/{name}")
        }
        [root, name, ..]
            if matches!(
                *root,
                "app" | "cmd" | "internal" | "lib" | "pkg" | "src" | "test" | "tests"
            ) && !looks_like_file(name) =>
        {
            format!("{root}/{name}")
        }
        [root, ..] => (*root).to_string(),
    }
}

fn looks_like_file(segment: &str) -> bool {
    segment
        .rsplit_once('.')
        .is_some_and(|(_, ext)| !ext.is_empty())
}

fn community_label(key: &str, members: &[SynthNode]) -> String {
    let folder = key
        .split('/')
        .rfind(|part| !part.is_empty() && *part != "(unknown)");
    if let Some(folder) = folder {
        return capitalize(folder);
    }

    let names: Vec<&str> = members
        .iter()
        .map(|node| node.name.as_str())
        .filter(|name| !name.is_empty())
        .collect();
    if names.len() > 2 {
        let prefix = common_prefix(&names);
        if prefix.len() > 2 {
            return capitalize(&prefix);
        }
    }
    "Cluster".into()
}

fn community_keywords(members: &[SynthNode]) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for member in members {
        for token in keyword_tokens(&member.name) {
            *counts.entry(token).or_default() += 2;
        }
        for segment in member
            .file_path
            .replace('\\', "/")
            .split('/')
            .filter(|segment| !segment.is_empty())
        {
            if looks_like_file(segment) {
                continue;
            }
            for token in keyword_tokens(segment) {
                *counts.entry(token).or_default() += 1;
            }
        }
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|(a_token, a_count), (b_token, b_count)| {
        b_count
            .cmp(a_count)
            .then_with(|| a_token.len().cmp(&b_token.len()))
            .then_with(|| a_token.cmp(b_token))
    });
    ranked.into_iter().take(8).map(|(token, _)| token).collect()
}

fn keyword_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .flat_map(split_identifier)
        .filter(|token| {
            token.len() >= 3
                && !matches!(
                    token.as_str(),
                    "src" | "lib" | "core" | "utils" | "common" | "shared" | "test" | "tests"
                )
        })
        .collect()
}

fn split_identifier(raw: &str) -> Vec<String> {
    let raw = raw.trim_matches('_');
    if raw.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut start = 0usize;
    let chars: Vec<(usize, char)> = raw.char_indices().collect();
    for i in 1..chars.len() {
        let prev = chars[i - 1].1;
        let current = chars[i].1;
        let next = chars.get(i + 1).map(|(_, ch)| *ch);
        let boundary = (prev.is_ascii_lowercase() && current.is_ascii_uppercase())
            || (prev.is_ascii_uppercase()
                && current.is_ascii_uppercase()
                && next.is_some_and(|ch| ch.is_ascii_lowercase()))
            || (prev.is_ascii_alphabetic() && current.is_ascii_digit())
            || (prev.is_ascii_digit() && current.is_ascii_alphabetic());
        if boundary {
            let off = chars[i].0;
            parts.push(raw[start..off].to_ascii_lowercase());
            start = off;
        }
    }
    parts.push(raw[start..].to_ascii_lowercase());
    parts
}

fn file_stem_label(file_name: &str) -> String {
    file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .filter(|stem| !stem.is_empty())
        .unwrap_or(file_name)
        .to_string()
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn common_prefix(strings: &[&str]) -> String {
    let Some(first) = strings.iter().min() else {
        return String::new();
    };
    let Some(last) = strings.iter().max() else {
        return String::new();
    };
    first
        .chars()
        .zip(last.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a)
        .collect()
}

fn round3(value: f64) -> f64 {
    (value.clamp(0.0, 1.0) * 1000.0).round() / 1000.0
}

fn stable_hash(s: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn export_chunks(
    conn: &Connection,
    project: &str,
    repo: &Path,
    path: &Path,
    total: u64,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<u64, EngineError> {
    let file = File::create(path)?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let mut stmt = conn.prepare(
        "SELECT id, qualified_name, label, file_path, start_line, end_line \
         FROM nodes \
         WHERE project = ?1 AND file_path != '' AND label NOT IN ('File','Folder','Project','Package','Module') \
         ORDER BY id",
    )?;
    let mut rows = stmt.query([project])?;
    let mut count = 0;
    let mut sources = SourceCache::new(repo);
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let qn = text_col(row, 1)?;
        let label = text_col(row, 2)?;
        let file_path = text_col(row, 3)?;
        let start_line: i64 = row.get(4)?;
        let end_line: i64 = row.get(5)?;
        let text = sources
            .read_line_span(&file_path, start_line, end_line)
            .unwrap_or_default();
        let chunk = json!({
            "nodeId": aka_node_id(cbm_id, &qn),
            "kind": format!("ast-{}", label.to_ascii_lowercase()),
            "filePath": file_path,
            "startLine": to_artifact_line(start_line),
            "endLine": to_artifact_line(end_line),
            "text": text,
        });
        serde_json::to_writer(&mut out, &chunk)?;
        out.write_all(b"\n")?;
        count += 1;
        emit_export_progress(on_event, "chunks", count, total);
    }
    emit_export_progress(on_event, "chunks", count, total);
    Ok(count)
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
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    self.files.insert(
                        file_path.to_string(),
                        text.lines().map(str::to_string).collect(),
                    );
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

fn parse_props(text: &str) -> Map<String, Value> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

fn text_col(row: &Row<'_>, idx: usize) -> Result<String, rusqlite::Error> {
    match row.get_ref(idx)? {
        ValueRef::Null => Ok(String::new()),
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
            Ok(String::from_utf8_lossy(bytes).into_owned())
        }
        ValueRef::Integer(v) => Ok(v.to_string()),
        ValueRef::Real(v) => Ok(v.to_string()),
    }
}

fn props_value(text: &str) -> Value {
    serde_json::from_str::<Value>(text).unwrap_or(Value::Null)
}

fn insert_if_missing(props: &mut Map<String, Value>, key: &str, value: Value) {
    props.entry(key.to_string()).or_insert(value);
}

fn sanitize_string_array_prop(props: &mut Map<String, Value>, key: &str) {
    let Some(values) = props.get(key).and_then(Value::as_array) else {
        return;
    };
    let mut out: Vec<Value> = values
        .iter()
        .filter_map(Value::as_str)
        .map(|s| s.trim_matches(['"', '\'']).to_string())
        .filter(|s| !s.is_empty() && s != "null" && s != "undefined")
        .map(Value::String)
        .collect();
    out.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    out.dedup();
    props.insert(key.to_string(), Value::Array(out));
}

fn to_artifact_line(line_1based: i64) -> u32 {
    if line_1based <= 0 {
        0
    } else {
        (line_1based - 1) as u32
    }
}

fn aka_node_id(cbm_id: i64, qn: &str) -> String {
    let mut out = String::with_capacity(qn.len() + 24);
    out.push_str("cbm:");
    out.push_str(&cbm_id.to_string());
    out.push(':');
    for ch in qn.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn git_head(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

fn wait_for_done_exit(
    child: &mut std::process::Child,
    grace: Duration,
) -> Result<ExitStatus, std::io::Error> {
    let deadline = Instant::now() + grace;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return child.wait();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE nodes (
                id INTEGER PRIMARY KEY,
                project TEXT NOT NULL,
                label TEXT NOT NULL,
                name TEXT,
                qualified_name TEXT,
                file_path TEXT,
                start_line INTEGER,
                end_line INTEGER,
                properties TEXT NOT NULL DEFAULT '{}'
            );
            CREATE TABLE edges (
                id INTEGER PRIMARY KEY,
                project TEXT NOT NULL,
                source_id INTEGER NOT NULL,
                target_id INTEGER NOT NULL,
                type TEXT NOT NULL,
                properties TEXT NOT NULL DEFAULT '{}'
            );",
        )
        .unwrap();
        conn
    }

    fn insert_node(conn: &Connection, id: i64, label: &str, name: &str, qn: &str, file: &str) {
        conn.execute(
            "INSERT INTO nodes (id, project, label, name, qualified_name, file_path, start_line, end_line, properties)
             VALUES (?1, 'demo', ?2, ?3, ?4, ?5, 1, 3, '{}')",
            rusqlite::params![id, label, name, qn, file],
        )
        .unwrap();
    }

    fn insert_edge(conn: &Connection, id: i64, src: i64, dst: i64, ty: &str) {
        conn.execute(
            "INSERT INTO edges (id, project, source_id, target_id, type, properties)
             VALUES (?1, 'demo', ?2, ?3, ?4, '{}')",
            rusqlite::params![id, src, dst, ty],
        )
        .unwrap();
    }

    fn temp_repo(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aka-core-engine-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn synthesize_graph_quiet(conn: &Connection, repo: &Path) -> Result<SynthGraph, EngineError> {
        synthesize_graph(conn, "demo", repo)
    }

    #[test]
    fn synthesizes_call_chain_processes_from_cbm_calls() {
        let conn = test_conn();
        insert_node(
            &conn,
            1,
            "Function",
            "main",
            "src/main.ts::main",
            "src/main.ts",
        );
        insert_node(
            &conn,
            2,
            "Function",
            "handleRequest",
            "src/handler.ts::handleRequest",
            "src/handler.ts",
        );
        insert_node(
            &conn,
            3,
            "Function",
            "parseConfig",
            "src/config.ts::parseConfig",
            "src/config.ts",
        );
        insert_edge(&conn, 1, 1, 2, "CALLS");
        insert_edge(&conn, 2, 2, 3, "CALLS");

        let processes = synthesize_graph_quiet(&conn, std::path::Path::new("."))
            .unwrap()
            .processes;
        assert_eq!(processes.len(), 1);
        let p = &processes[0];
        assert_eq!(p.name, "main → parseConfig");
        assert_eq!(
            p.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["main", "handleRequest", "parseConfig"]
        );

        let node = p.node_rec();
        assert_eq!(node.label, "Process");
        assert_eq!(node.properties["processType"], "intra_community");
        assert_eq!(node.properties["stepCount"], 3);
        assert_eq!(node.properties["entryPointId"], p.steps[0].aka_id);
        assert_eq!(node.properties["terminalId"], p.steps[2].aka_id);
        assert_eq!(node.properties["trace"].as_array().expect("trace").len(), 3);
        assert_eq!(
            node.properties["communities"]
                .as_array()
                .expect("communities")
                .len(),
            1
        );

        let edges = p.edge_recs();
        assert_eq!(
            edges
                .iter()
                .filter(|e| e.edge_type == "ENTRY_POINT_OF")
                .count(),
            1
        );
        let steps: Vec<u32> = edges
            .iter()
            .filter(|e| e.edge_type == "STEP_IN_PROCESS")
            .filter_map(|e| e.step)
            .collect();
        assert_eq!(steps, [1, 2, 3]);
    }

    #[test]
    fn synthesizes_community_nodes_and_member_edges() {
        let conn = test_conn();
        insert_node(
            &conn,
            1,
            "Function",
            "main",
            "src/main.ts::main",
            "src/main.ts",
        );
        insert_node(
            &conn,
            2,
            "Function",
            "next",
            "src/main.ts::next",
            "src/main.ts",
        );
        insert_node(
            &conn,
            3,
            "Function",
            "store",
            "src/store.ts::store",
            "src/store.ts",
        );
        insert_edge(&conn, 1, 1, 2, "CALLS");
        insert_edge(&conn, 2, 2, 3, "CALLS");

        let synth = synthesize_graph_quiet(&conn, std::path::Path::new(".")).unwrap();
        assert_eq!(synth.communities.len(), 1);
        let main = synth
            .communities
            .iter()
            .find(|c| c.heuristic_label == "Src")
            .expect("src community");
        assert_eq!(main.members.len(), 3);

        let node = main.node_rec();
        assert_eq!(node.label, "Community");
        assert_eq!(node.properties["heuristicLabel"], "Src");
        assert_eq!(node.properties["symbolCount"], 3);
        assert_eq!(node.properties["source"], "aka-cbm-synth");
        assert_eq!(main.edge_recs().len(), 3);
        assert!(main
            .edge_recs()
            .iter()
            .all(|edge| edge.edge_type == "MEMBER_OF"));

        let process = synth.processes.first().expect("process");
        assert_eq!(process.process_type, "intra_community");
        assert_eq!(process.communities.len(), 1);
    }

    #[test]
    fn marks_cross_module_processes_as_cross_community() {
        let conn = test_conn();
        insert_node(
            &conn,
            1,
            "Function",
            "main",
            "src/api/main.ts::main",
            "src/api/main.ts",
        );
        insert_node(
            &conn,
            2,
            "Function",
            "handle",
            "src/api/handler.ts::handle",
            "src/api/handler.ts",
        );
        insert_node(
            &conn,
            3,
            "Function",
            "save",
            "src/db/store.ts::save",
            "src/db/store.ts",
        );
        insert_node(
            &conn,
            4,
            "Function",
            "commit",
            "src/db/store.ts::commit",
            "src/db/store.ts",
        );
        insert_edge(&conn, 1, 1, 2, "CALLS");
        insert_edge(&conn, 2, 2, 3, "CALLS");
        insert_edge(&conn, 3, 3, 4, "CALLS");

        let synth = synthesize_graph_quiet(&conn, std::path::Path::new(".")).unwrap();
        assert_eq!(synth.communities.len(), 2);
        let process = synth.processes.first().expect("process");
        assert_eq!(process.process_type, "cross_community");
        assert_eq!(process.communities.len(), 2);
    }

    #[test]
    fn marks_single_community_processes_as_intra_community() {
        let conn = test_conn();
        insert_node(
            &conn,
            1,
            "Function",
            "main",
            "src/main.ts::main",
            "src/main.ts",
        );
        insert_node(
            &conn,
            2,
            "Function",
            "next",
            "src/main.ts::next",
            "src/main.ts",
        );
        insert_node(
            &conn,
            3,
            "Function",
            "done",
            "src/main.ts::done",
            "src/main.ts",
        );
        insert_edge(&conn, 1, 1, 2, "CALLS");
        insert_edge(&conn, 2, 2, 3, "CALLS");

        let processes = synthesize_graph_quiet(&conn, std::path::Path::new("."))
            .unwrap()
            .processes;
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].process_type, "intra_community");
        assert_eq!(processes[0].communities.len(), 1);
        let node = processes[0].node_rec();
        assert_eq!(node.properties["processType"], "intra_community");
    }

    #[test]
    fn skips_synthesis_when_engine_already_emits_processes() {
        let conn = test_conn();
        insert_node(&conn, 1, "Process", "native", "process:native", "");
        insert_node(
            &conn,
            2,
            "Function",
            "main",
            "src/main.ts::main",
            "src/main.ts",
        );
        insert_node(
            &conn,
            3,
            "Function",
            "next",
            "src/main.ts::next",
            "src/main.ts",
        );
        insert_edge(&conn, 1, 2, 3, "CALLS");
        insert_edge(&conn, 2, 2, 1, "STEP_IN_PROCESS");

        let processes = synthesize_graph_quiet(&conn, std::path::Path::new("."))
            .unwrap()
            .processes;
        assert!(processes.is_empty());
    }

    #[test]
    fn skips_synthesis_when_engine_emits_process_nodes_without_step_edges() {
        let conn = test_conn();
        insert_node(
            &conn,
            1,
            "Process",
            "native-empty",
            "process:native-empty",
            "",
        );
        insert_node(
            &conn,
            2,
            "Function",
            "main",
            "src/main.ts::main",
            "src/main.ts",
        );
        insert_node(
            &conn,
            3,
            "Function",
            "next",
            "src/main.ts::next",
            "src/main.ts",
        );
        insert_edge(&conn, 1, 2, 3, "CALLS");

        let processes = synthesize_graph_quiet(&conn, std::path::Path::new("."))
            .unwrap()
            .processes;
        assert!(processes.is_empty());
    }

    #[test]
    fn skips_community_synthesis_when_engine_already_emits_communities() {
        let conn = test_conn();
        insert_node(&conn, 1, "Community", "native", "community:native", "");
        insert_node(
            &conn,
            2,
            "Function",
            "main",
            "src/main.ts::main",
            "src/main.ts",
        );
        insert_node(
            &conn,
            3,
            "Function",
            "next",
            "src/main.ts::next",
            "src/main.ts",
        );
        insert_node(
            &conn,
            4,
            "Function",
            "done",
            "src/main.ts::done",
            "src/main.ts",
        );
        insert_edge(&conn, 1, 2, 3, "CALLS");
        insert_edge(&conn, 2, 3, 4, "CALLS");
        insert_edge(&conn, 3, 2, 1, "MEMBER_OF");
        insert_edge(&conn, 4, 3, 1, "MEMBER_OF");

        let synth = synthesize_graph_quiet(&conn, std::path::Path::new(".")).unwrap();
        assert!(synth.communities.is_empty());
        assert_eq!(synth.processes.len(), 1);
        assert_eq!(synth.processes[0].process_type, "intra_community");
        assert_eq!(synth.processes[0].communities.len(), 1);
    }

    #[test]
    fn process_synthesis_uses_gitnexus_like_entry_scoring_and_dedup() {
        let conn = test_conn();
        insert_node(
            &conn,
            1,
            "Function",
            "validateInput",
            "src/api/handler.ts::validateInput",
            "src/api/handler.ts",
        );
        insert_node(
            &conn,
            2,
            "Function",
            "handleLogin",
            "src/api/handler.ts::handleLogin",
            "src/api/handler.ts",
        );
        insert_node(
            &conn,
            3,
            "Function",
            "loadUser",
            "src/auth/user.ts::loadUser",
            "src/auth/user.ts",
        );
        insert_node(
            &conn,
            4,
            "Function",
            "commitSession",
            "src/auth/session.ts::commitSession",
            "src/auth/session.ts",
        );
        insert_node(
            &conn,
            5,
            "Function",
            "handleLoginSpec",
            "src/api/handler.test.ts::handleLoginSpec",
            "src/api/handler.test.ts",
        );
        insert_node(
            &conn,
            6,
            "Function",
            "assertSession",
            "src/api/handler.test.ts::assertSession",
            "src/api/handler.test.ts",
        );
        insert_edge(&conn, 1, 2, 3, "CALLS");
        insert_edge(&conn, 2, 3, 4, "CALLS");
        insert_edge(&conn, 3, 1, 2, "CALLS");
        insert_edge(&conn, 4, 5, 6, "CALLS");

        let synth = synthesize_graph_quiet(&conn, std::path::Path::new(".")).unwrap();
        let processes = synth.processes;
        assert_eq!(processes.len(), 1);
        let process = &processes[0];
        assert_eq!(process.name, "validateInput → commitSession");
        assert_eq!(
            process
                .steps
                .iter()
                .map(|step| step.name.as_str())
                .collect::<Vec<_>>(),
            ["validateInput", "handleLogin", "loadUser", "commitSession"]
        );
        assert!(
            process
                .steps
                .iter()
                .all(|step| !step.file_path.contains(".test.")),
            "test-file entry points should not produce processes"
        );
    }

    #[test]
    fn synthesizes_route_nodes_consumers_and_entry_flows() {
        let repo = temp_repo("routes");
        std::fs::create_dir_all(repo.join("src/pages/api/config")).unwrap();
        std::fs::create_dir_all(repo.join("src/components")).unwrap();
        std::fs::write(
            repo.join("src/pages/api/config/route.ts"),
            "export async function GET() { return Response.json({ data: [], pagination: {}, error: null }); }",
        )
        .unwrap();
        std::fs::write(
            repo.join("src/components/config-panel.tsx"),
            "export async function ConfigPanel() { const res = await fetch('/api/config'); const data = await res.json(); return data.pagination.total + data.missing; }",
        )
        .unwrap();

        let conn = test_conn();
        insert_node(
            &conn,
            1,
            "Function",
            "GET",
            "src/pages/api/config/route.ts::GET",
            "src/pages/api/config/route.ts",
        );
        insert_node(
            &conn,
            2,
            "Function",
            "loadConfig",
            "src/pages/api/config/route.ts::loadConfig",
            "src/pages/api/config/route.ts",
        );
        insert_node(
            &conn,
            3,
            "Function",
            "ConfigPanel",
            "src/components/config-panel.tsx::ConfigPanel",
            "src/components/config-panel.tsx",
        );
        insert_edge(&conn, 1, 1, 2, "CALLS");

        let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
        assert_eq!(synth.routes.len(), 1);
        let route = &synth.routes[0];
        assert_eq!(route.route, "/api/config");
        assert!(route.response_keys.contains(&"data".to_string()));
        assert!(route.response_keys.contains(&"pagination".to_string()));
        assert!(route.error_keys.contains(&"error".to_string()));
        assert_eq!(route.consumers.len(), 1);
        assert_eq!(route.consumers[0].fetch_count, 1);
        assert!(route.consumers[0].keys.contains(&"pagination".to_string()));

        let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
        assert!(edge_types.contains(&"HANDLES_ROUTE".to_string()));
        assert!(edge_types.contains(&"FETCHES".to_string()));
    }

    #[test]
    fn synthesizes_tool_nodes_and_handler_edges() {
        let repo = temp_repo("tools");
        std::fs::create_dir_all(repo.join("src/mcp")).unwrap();
        std::fs::write(
            repo.join("src/mcp/server.ts"),
            "export function handleIndexRepo() {}\nserver.tool('index_repo', { description: 'Index a repository' }, handleIndexRepo);",
        )
        .unwrap();

        let conn = test_conn();
        insert_node(
            &conn,
            1,
            "Function",
            "handleIndexRepo",
            "src/mcp/server.ts::handleIndexRepo",
            "src/mcp/server.ts",
        );

        let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
        assert_eq!(synth.tools.len(), 1);
        let tool = &synth.tools[0];
        assert_eq!(tool.name, "index_repo");
        assert_eq!(tool.description, "Index a repository");
        assert!(tool.handler_id.is_some());
        assert_eq!(tool.edge_recs()[0].edge_type, "HANDLES_TOOL");
    }
}
