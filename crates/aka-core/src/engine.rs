//! Engine runner backed by codebase-memory-mcp.
//!
//! `aka` still consumes the artifact contract in `docs/contracts/artifacts.md`,
//! but the producer is now the native C codebase-memory indexer instead of the
//! previous parser sidecar.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use chrono::Utc;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};
use serde_json::{json, Map, Value};

use crate::types::{ArtifactStats, EdgeRec, EngineEvent, Manifest, NodeRec, CONTRACT_VERSION};

mod cache_synth;
mod command_synth;
mod event_synth;
mod graphql_synth;
mod job_synth;
mod persistence_synth;
mod policy_synth;
mod property_synth;
mod resource_synth;
mod route_shape;
mod source_scan;
mod tool_synth;
mod topic_synth;
mod transaction_synth;
use cache_synth::{synthesize_caches_from_sources, SynthCache};
use command_synth::{synthesize_commands_from_sources, SynthCommand};
use event_synth::{synthesize_events_from_sources, SynthEvent};
use graphql_synth::{synthesize_graphql_from_sources, SynthGraphqlOperation};
use job_synth::{synthesize_jobs_from_sources, SynthJob};
use persistence_synth::{synthesize_persistence_from_sources, SynthPersistenceGraph};
use policy_synth::{synthesize_policies_from_sources, SynthPolicy};
#[cfg(test)]
use property_synth::extract_python_class_properties;
use property_synth::{synthesize_python_properties, SynthProperty};
use resource_synth::{synthesize_resources_from_sources, SynthResource};
use route_shape::{
    extract_accessed_keys_near_route, extract_error_keys, extract_middleware,
    extract_response_keys, fetch_literal_windows, is_route_parameter_segment, literal_occurrences,
    route_occurrences,
};
use source_scan::{
    find_call_args, find_matching_paren, is_ident_continue, is_noisy_source_path, node_at_offset,
    nodes_by_file, pick_handler_node, read_repo_text, skip_ws, split_top_level_commas, stable_hash,
};
use tool_synth::{synthesize_tools_from_sources, SynthTool};
use topic_synth::{synthesize_topics_from_sources, SynthTopic};
use transaction_synth::{synthesize_transactions_from_sources, SynthTransaction};

const DEFAULT_CBM_MODE: &str = "fast";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PROCESS_MAX_STARTS: usize = 200;
const PROCESS_MIN_COUNT: usize = 20;
const PROCESS_MAX_COUNT: usize = 300;
const PROCESS_MAX_STEPS: usize = 10;
const PROCESS_BRANCH_LIMIT: usize = 4;
const PROCESS_MIN_STEPS: usize = 3;
const MIN_SYNTH_COMMUNITY_SIZE: usize = 2;
const MIN_TRACE_CONFIDENCE: f64 = 0.5;
const COMMUNITY_LABEL_PROPAGATION_PASSES: usize = 4;

#[cfg(windows)]
fn hide_child_console(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_child_console(_cmd: &mut Command) {}

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
        hide_child_console(&mut cmd);
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
    if let Err(err) = warn_missing_source_extensions(repo, &conn, project, on_event) {
        on_event(&EngineEvent::Warning {
            message: format!("source language coverage check failed: {err}"),
        });
    }

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

fn warn_missing_source_extensions(
    repo: &Path,
    conn: &Connection,
    project: &str,
    on_event: &mut impl FnMut(&EngineEvent),
) -> Result<(), EngineError> {
    let repo_exts = repo_source_extensions(repo)?;
    if repo_exts.is_empty() {
        return Ok(());
    }
    let indexed_exts = indexed_source_extensions(conn, project)?;
    for (ext, language) in [
        ("java", "Java"),
        ("py", "Python"),
        ("go", "Go"),
        ("rs", "Rust"),
        ("ts", "TypeScript"),
        ("tsx", "TSX"),
        ("js", "JavaScript"),
        ("jsx", "JSX"),
    ] {
        if repo_exts.contains(ext) && !indexed_exts.contains(ext) {
            on_event(&EngineEvent::Warning {
                message: format!(
                    "CBM indexed 0 {language} source files even though the repository contains .{ext} files; graph/search may be incomplete. Try AKA_CBM_MODE=full or sync/fix the CBM engine discovery rules."
                ),
            });
        }
    }
    Ok(())
}

fn indexed_source_extensions(
    conn: &Connection,
    project: &str,
) -> Result<HashSet<String>, EngineError> {
    let mut exts = HashSet::new();
    if let Some(file_hash_col) = file_hashes_path_column(conn)? {
        let sql = format!("SELECT {file_hash_col} FROM file_hashes WHERE project = ?1");
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query([project])?;
        while let Some(row) = rows.next()? {
            if let Some(ext) = source_extension(&text_col(row, 0)?) {
                exts.insert(ext);
            }
        }
    }
    let mut stmt = conn
        .prepare("SELECT DISTINCT file_path FROM nodes WHERE project = ?1 AND file_path != ''")?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        if let Some(ext) = source_extension(&text_col(row, 0)?) {
            exts.insert(ext);
        }
    }
    Ok(exts)
}

fn file_hashes_path_column(conn: &Connection) -> Result<Option<&'static str>, EngineError> {
    let mut stmt = conn.prepare("PRAGMA table_info(file_hashes)")?;
    let mut rows = stmt.query([])?;
    let mut has_rel_path = false;
    let mut has_file_path = false;
    while let Some(row) = rows.next()? {
        let name = text_col(row, 1)?;
        if name == "rel_path" {
            has_rel_path = true;
        } else if name == "file_path" {
            has_file_path = true;
        }
    }
    Ok(if has_rel_path {
        Some("rel_path")
    } else if has_file_path {
        Some("file_path")
    } else {
        None
    })
}

fn repo_source_extensions(repo: &Path) -> Result<HashSet<String>, EngineError> {
    let mut exts = HashSet::new();
    collect_repo_source_extensions(repo, repo, &mut exts)?;
    Ok(exts)
}

fn collect_repo_source_extensions(
    repo: &Path,
    dir: &Path,
    exts: &mut HashSet<String>,
) -> Result<(), EngineError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if file_type.is_dir() {
            if is_source_discovery_skip_dir(name) {
                continue;
            }
            collect_repo_source_extensions(repo, &path, exts)?;
        } else if file_type.is_file() {
            let Ok(rel) = path.strip_prefix(repo) else {
                continue;
            };
            if let Some(ext) = source_extension(&rel.to_string_lossy()) {
                exts.insert(ext);
            }
        }
    }
    Ok(())
}

fn source_extension(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let ext = Path::new(&path)
        .extension()
        .and_then(|v| v.to_str())?
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "java" | "py" | "go" | "rs" | "ts" | "tsx" | "js" | "jsx"
    )
    .then_some(ext)
}

fn is_source_discovery_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | "vendor"
            | "vendors"
            | "target"
            | "build"
            | "dist"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".idea"
            | ".vscode"
            | ".next"
            | ".nuxt"
            | ".turbo"
            | "coverage"
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
    for property in &synth.properties {
        let node = property.node_rec();
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
    for command in &synth.commands {
        let node = command.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for job in &synth.jobs {
        let node = job.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for topic in &synth.topics {
        let node = topic.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for cache in &synth.caches {
        let node = cache.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for event in &synth.events {
        let node = event.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for policy in &synth.policies {
        let node = policy.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for resource in &synth.resources {
        let node = resource.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for operation in &synth.graphql {
        let node = operation.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for node in synth.persistence.node_recs() {
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for transaction in &synth.transactions {
        let node = transaction.node_rec();
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
                s.qualified_name, t.qualified_name, s.label, t.label \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id \
         JOIN nodes t ON t.id = e.target_id \
         WHERE e.project = ?1 ORDER BY e.id",
    )?;
    let mut rows = stmt.query([project])?;
    let mut count = 0;
    let mut semantic = SemanticEdgeSynthesizer::load(conn, project)?;
    while let Some(row) = rows.next()? {
        let edge_id: i64 = row.get(0)?;
        let source_id: i64 = row.get(1)?;
        let target_id: i64 = row.get(2)?;
        let edge_type = text_col(row, 3)?;
        let props_text = text_col(row, 4)?;
        let source_qn = text_col(row, 5)?;
        let target_qn = text_col(row, 6)?;
        let props = props_value(&props_text);
        let source_label = text_col(row, 7)?;
        let target_label = text_col(row, 8)?;
        semantic.record(
            SemanticEndpoint::new(source_id, &source_qn, &source_label),
            SemanticEndpoint::new(target_id, &target_qn, &target_label),
            &edge_type,
        );
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
    for edge in semantic.edge_recs() {
        serde_json::to_writer(&mut out, &edge)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for edge in synth.properties.iter().map(SynthProperty::edge_rec) {
        serde_json::to_writer(&mut out, &edge)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    for edge in synth
        .communities
        .iter()
        .flat_map(SynthCommunity::edge_recs)
        .chain(synth.processes.iter().flat_map(SynthProcess::edge_recs))
        .chain(synth.routes.iter().flat_map(SynthRoute::edge_recs))
        .chain(synth.tools.iter().flat_map(SynthTool::edge_recs))
        .chain(synth.commands.iter().flat_map(SynthCommand::edge_recs))
        .chain(synth.jobs.iter().flat_map(SynthJob::edge_recs))
        .chain(synth.topics.iter().flat_map(SynthTopic::edge_recs))
        .chain(synth.caches.iter().flat_map(SynthCache::edge_recs))
        .chain(synth.events.iter().flat_map(SynthEvent::edge_recs))
        .chain(synth.policies.iter().flat_map(SynthPolicy::edge_recs))
        .chain(synth.resources.iter().flat_map(SynthResource::edge_recs))
        .chain(
            synth
                .graphql
                .iter()
                .flat_map(SynthGraphqlOperation::edge_recs),
        )
        .chain(synth.persistence.edge_recs())
        .chain(
            synth
                .transactions
                .iter()
                .flat_map(SynthTransaction::edge_recs),
        )
        .chain(synth.edges.iter().cloned())
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

#[derive(Debug, Clone, Default)]
struct SemanticEdgeSynthesizer {
    seen: HashSet<(String, String, String)>,
    out: Vec<EdgeRec>,
    nodes_by_qn: BTreeMap<String, SemanticNode>,
    methods_by_owner_name: BTreeMap<(String, String), Vec<SemanticNode>>,
    type_by_qn: BTreeMap<String, SemanticNode>,
    implements: Vec<(String, String)>,
}

impl SemanticEdgeSynthesizer {
    fn load(conn: &Connection, project: &str) -> Result<Self, rusqlite::Error> {
        let mut this = Self::default();
        let mut stmt = conn.prepare(
            "SELECT id, label, name, qualified_name \
             FROM nodes \
             WHERE project = ?1 AND label IN ('Class','Interface','Method','Field','Variable','Property') \
             ORDER BY id",
        )?;
        let mut rows = stmt.query([project])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let label = text_col(row, 1)?;
            let name = text_col(row, 2)?;
            let qn = text_col(row, 3)?;
            let node = SemanticNode {
                id: aka_node_id(id, &qn),
                label,
                name,
                qn: qn.clone(),
            };
            if matches!(node.label.as_str(), "Class" | "Interface") {
                this.type_by_qn.insert(qn.clone(), node.clone());
            }
            if node.label == "Method" {
                if let Some(owner) = semantic_owner_qn(&qn, &node.name) {
                    this.methods_by_owner_name
                        .entry((owner.to_string(), node.name.clone()))
                        .or_default()
                        .push(node.clone());
                }
            }
            this.nodes_by_qn.insert(qn, node);
        }
        this.add_member_properties();
        Ok(this)
    }

    fn record(
        &mut self,
        source: SemanticEndpoint<'_>,
        target: SemanticEndpoint<'_>,
        edge_type: &str,
    ) {
        match edge_type {
            "DEFINES_METHOD" => {
                self.add(
                    source.node_id(),
                    target.node_id(),
                    "HAS_METHOD",
                    "aka semantic edge from DEFINES_METHOD",
                );
            }
            "USAGE" if matches!(target.label, "Field" | "Variable" | "Property") => {
                self.add(
                    source.node_id(),
                    target.node_id(),
                    "ACCESSES",
                    "aka semantic edge from symbol usage",
                );
            }
            "INHERITS" | "IMPLEMENTS"
                if matches!(source.label, "Class") && matches!(target.label, "Interface") =>
            {
                self.add(
                    source.node_id(),
                    target.node_id(),
                    "IMPLEMENTS",
                    "aka semantic edge from inheritance reference",
                );
                self.implements
                    .push((source.qn.to_string(), target.qn.to_string()));
            }
            _ => {}
        }
    }

    fn edge_recs(mut self) -> Vec<EdgeRec> {
        for (class_qn, iface_qn) in self.implements.clone() {
            self.add_method_implements(&class_qn, &iface_qn);
        }
        self.out
    }

    fn add_member_properties(&mut self) {
        let nodes: Vec<_> = self.nodes_by_qn.values().cloned().collect();
        for member in nodes
            .iter()
            .filter(|n| matches!(n.label.as_str(), "Field" | "Property"))
        {
            let Some(owner_qn) = semantic_owner_qn(&member.qn, &member.name) else {
                continue;
            };
            let Some(owner) = self.type_by_qn.get(owner_qn) else {
                continue;
            };
            self.add(
                owner.id.clone(),
                member.id.clone(),
                "HAS_PROPERTY",
                "aka semantic edge from owned property",
            );
        }
    }

    fn add_method_implements(&mut self, class_qn: &str, iface_qn: &str) {
        let iface_methods: Vec<_> = self
            .methods_by_owner_name
            .iter()
            .filter(|((owner, _), _)| owner == iface_qn)
            .map(|((_, name), methods)| (name.clone(), methods.clone()))
            .collect();
        for (method_name, interface_methods) in iface_methods {
            let Some(class_methods) = self
                .methods_by_owner_name
                .get(&(class_qn.to_string(), method_name))
                .cloned()
            else {
                continue;
            };
            for class_method in &class_methods {
                for interface_method in &interface_methods {
                    self.add(
                        class_method.id.clone(),
                        interface_method.id.clone(),
                        "METHOD_IMPLEMENTS",
                        "aka semantic edge from class/interface method match",
                    );
                }
            }
        }
    }

    fn add(&mut self, source: String, target: String, edge_type: &str, reason: &str) {
        let key = (source.clone(), edge_type.to_string(), target.clone());
        if !self.seen.insert(key) {
            return;
        }
        let evidence = json!({
            "source": "aka-cbm-synth",
            "kind": "semantic-compat",
            "from": "codebase-memory"
        });
        self.out.push(EdgeRec {
            id: format!(
                "semantic:{}:{:016x}",
                edge_type.to_ascii_lowercase(),
                stable_hash(&format!("{source}|{edge_type}|{target}"))
            ),
            source_id: source,
            target_id: target,
            edge_type: edge_type.into(),
            confidence: 0.86,
            reason: reason.into(),
            step: None,
            evidence: Some(evidence),
        });
    }
}

#[derive(Debug, Clone)]
struct SemanticNode {
    id: String,
    label: String,
    name: String,
    qn: String,
}

#[derive(Debug, Clone, Copy)]
struct SemanticEndpoint<'a> {
    id: i64,
    qn: &'a str,
    label: &'a str,
}

impl<'a> SemanticEndpoint<'a> {
    fn new(id: i64, qn: &'a str, label: &'a str) -> Self {
        Self { id, qn, label }
    }

    fn node_id(self) -> String {
        aka_node_id(self.id, self.qn)
    }
}

fn semantic_owner_qn<'a>(member_qn: &'a str, name: &str) -> Option<&'a str> {
    let tail = member_qn.rsplit('.').next()?;
    if tail != name {
        return None;
    }
    member_qn.rsplit_once('.').map(|(owner, _)| owner)
}

#[derive(Debug, Clone)]
pub(super) struct SynthNode {
    aka_id: String,
    qn: String,
    label: String,
    name: String,
    file_path: String,
    start_line: i64,
    end_line: i64,
    language: String,
    route_path: Option<String>,
    route_method: Option<String>,
    decorators: Vec<String>,
    parent_class: Option<String>,
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
    commands: Vec<SynthCommand>,
    jobs: Vec<SynthJob>,
    topics: Vec<SynthTopic>,
    caches: Vec<SynthCache>,
    events: Vec<SynthEvent>,
    policies: Vec<SynthPolicy>,
    resources: Vec<SynthResource>,
    graphql: Vec<SynthGraphqlOperation>,
    persistence: SynthPersistenceGraph,
    transactions: Vec<SynthTransaction>,
    properties: Vec<SynthProperty>,
    edges: Vec<EdgeRec>,
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
pub(super) struct SynthProcess {
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
    method: Option<String>,
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

#[derive(Debug, Clone)]
struct RouteCandidate {
    route: String,
    method: Option<String>,
    handler_id: Option<String>,
    handler_name: Option<String>,
}

impl SynthRoute {
    fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.route.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("routeSource".into(), Value::String("source-scan".into()));
        if let Some(method) = &self.method {
            properties.insert("method".into(), Value::String(method.clone()));
        }
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
    let existing_node_ids = load_existing_node_ids(conn, project)?;
    let properties = synthesize_python_properties(conn, project, repo, &existing_node_ids)?;
    if nodes.is_empty() {
        return Ok(SynthGraph {
            properties,
            ..SynthGraph::default()
        });
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
    let existing_call_pairs = load_existing_call_pairs(conn, project)?;
    let synthetic_edges = synthesize_dependency_call_edges(repo, &nodes, &existing_call_pairs);
    let calls = load_call_graph(conn, project, &nodes, &synthetic_edges)?;

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
        let symbol_count = count_process_symbol_basis(conn, project)?;
        synthesize_processes_from_calls(
            &nodes,
            &calls.adjacency,
            &calls.indegree,
            &community_memberships,
            symbol_count,
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
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:commands",
        0,
        0,
    );
    let commands = synthesize_commands_from_sources(repo, &nodes, &processes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:jobs",
        0,
        0,
    );
    let jobs = synthesize_jobs_from_sources(repo, &nodes, &processes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:topics",
        0,
        0,
    );
    let topics = synthesize_topics_from_sources(repo, &nodes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:caches",
        0,
        0,
    );
    let caches = synthesize_caches_from_sources(repo, &nodes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:events",
        0,
        0,
    );
    let events = synthesize_events_from_sources(repo, &nodes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:policies",
        0,
        0,
    );
    let policies = synthesize_policies_from_sources(repo, &nodes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:resources",
        0,
        0,
    );
    let resources = synthesize_resources_from_sources(repo, &nodes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:graphql",
        0,
        0,
    );
    let graphql = synthesize_graphql_from_sources(repo, &nodes, &processes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:persistence",
        0,
        0,
    );
    let persistence = synthesize_persistence_from_sources(repo, &nodes);
    emit_phase(
        on_event,
        "codebase-memory:export-artifacts:synthesize:transactions",
        0,
        0,
    );
    let transactions = synthesize_transactions_from_sources(repo, &nodes);

    Ok(SynthGraph {
        communities,
        processes,
        routes,
        tools,
        commands,
        jobs,
        topics,
        caches,
        events,
        policies,
        resources,
        graphql,
        persistence,
        transactions,
        properties,
        edges: synthetic_edges,
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
        "SELECT id, label, name, qualified_name, file_path, start_line, end_line, properties \
         FROM nodes WHERE project = ?1 ORDER BY id",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let label = text_col(row, 1)?;
        let name = text_col(row, 2)?;
        let qn = text_col(row, 3)?;
        let file_path = text_col(row, 4)?;
        let start_line: i64 = row.get(5)?;
        let end_line: i64 = row.get(6)?;
        let props = parse_props(&text_col(row, 7)?);
        if !is_process_step_label(&label) || is_noisy_source_path(&file_path) {
            continue;
        }
        let aka_id = aka_node_id(cbm_id, &qn);
        let language = props
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let route_path = props
            .get("route_path")
            .or_else(|| props.get("routePath"))
            .and_then(Value::as_str)
            .map(normalize_route_literal);
        let route_method = props
            .get("route_method")
            .or_else(|| props.get("routeMethod"))
            .and_then(Value::as_str)
            .map(|v| v.to_ascii_uppercase());
        let decorators = props
            .get("decorators")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let parent_class = props
            .get("parent_class")
            .or_else(|| props.get("parentClass"))
            .and_then(Value::as_str)
            .map(str::to_string);
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
                qn,
                label,
                name,
                file_path,
                start_line,
                end_line,
                language,
                route_path,
                route_method,
                decorators,
                parent_class,
                is_exported,
                ast_framework_multiplier,
                ast_framework_reason,
            },
        );
    }
    Ok(nodes)
}

fn load_existing_node_ids(
    conn: &Connection,
    project: &str,
) -> Result<HashSet<String>, EngineError> {
    let mut ids = HashSet::new();
    let mut stmt = conn.prepare("SELECT id, qualified_name FROM nodes WHERE project = ?1")?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let qn = text_col(row, 1)?;
        ids.insert(aka_node_id(cbm_id, &qn));
    }
    Ok(ids)
}

fn count_process_symbol_basis(conn: &Connection, project: &str) -> Result<usize, EngineError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND label != 'File'",
        [project],
        |row| row.get(0),
    )?;
    Ok(count as usize)
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
    synthetic_edges: &[EdgeRec],
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
    for edge in synthetic_edges
        .iter()
        .filter(|edge| edge.edge_type == "CALLS")
    {
        if !nodes.contains_key(&edge.source_id)
            || !nodes.contains_key(&edge.target_id)
            || edge.source_id == edge.target_id
        {
            continue;
        }
        let inserted = graph
            .adjacency
            .entry(edge.source_id.clone())
            .or_default()
            .insert(edge.target_id.clone());
        if inserted {
            *graph.indegree.entry(edge.target_id.clone()).or_default() += 1;
            graph
                .edges
                .push((edge.source_id.clone(), edge.target_id.clone()));
        }
    }
    Ok(graph)
}

fn load_existing_call_pairs(
    conn: &Connection,
    project: &str,
) -> Result<HashSet<(String, String)>, EngineError> {
    let mut pairs = HashSet::new();
    let mut stmt = conn.prepare(
        "SELECT e.source_id, e.target_id, s.qualified_name, t.qualified_name \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id AND s.project = e.project \
         JOIN nodes t ON t.id = e.target_id AND t.project = e.project \
         WHERE e.project = ?1 AND e.type = 'CALLS'",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let source_qn = text_col(row, 2)?;
        let target_qn = text_col(row, 3)?;
        pairs.insert((
            aka_node_id(source_id, &source_qn),
            aka_node_id(target_id, &target_qn),
        ));
    }
    Ok(pairs)
}

fn synthesize_dependency_call_edges(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    existing_call_pairs: &HashSet<(String, String)>,
) -> Vec<EdgeRec> {
    let python_functions: Vec<&SynthNode> = nodes
        .values()
        .filter(|node| {
            matches!(node.label.as_str(), "Function" | "Method")
                && (node.language.eq_ignore_ascii_case("python") || node.file_path.ends_with(".py"))
                && !node.file_path.is_empty()
        })
        .collect();
    if python_functions.is_empty() {
        return Vec::new();
    }

    let mut by_name: HashMap<String, Vec<&SynthNode>> = HashMap::new();
    let mut by_file_name: HashMap<(String, String), Vec<&SynthNode>> = HashMap::new();
    for node in &python_functions {
        by_name.entry(node.name.clone()).or_default().push(*node);
        by_file_name
            .entry((node.file_path.clone(), node.name.clone()))
            .or_default()
            .push(*node);
    }

    let mut by_file: BTreeMap<String, Vec<&SynthNode>> = BTreeMap::new();
    for node in python_functions {
        by_file
            .entry(node.file_path.clone())
            .or_default()
            .push(node);
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (file_path, mut file_nodes) in by_file {
        file_nodes.sort_by_key(|node| node.start_line_key());
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for node in file_nodes {
            if !is_python_route_node(node) {
                continue;
            }
            let Some(source) = node_source_slice(&text, node) else {
                continue;
            };
            for dep in extract_fastapi_depends_targets(source) {
                let Some(target) =
                    resolve_python_dependency_target(&file_path, &dep, &by_file_name, &by_name)
                else {
                    continue;
                };
                if target.aka_id == node.aka_id {
                    continue;
                }
                if existing_call_pairs.contains(&(node.aka_id.clone(), target.aka_id.clone())) {
                    continue;
                }
                let key = (node.aka_id.clone(), target.aka_id.clone(), dep.clone());
                if !seen.insert(key) {
                    continue;
                }
                out.push(EdgeRec {
                    id: format!(
                        "python-depends-call:{:016x}",
                        stable_hash(&format!("{}|{}|{dep}", node.aka_id, target.aka_id))
                    ),
                    source_id: node.aka_id.clone(),
                    target_id: target.aka_id.clone(),
                    edge_type: "CALLS".into(),
                    confidence: 0.74,
                    reason: "aka FastAPI Depends dependency call".into(),
                    step: None,
                    evidence: Some(json!({
                        "source": "aka-cbm-synth",
                        "kind": "fastapi-depends",
                        "dependency": dep,
                    })),
                });
            }
        }
    }
    out
}

fn node_source_slice<'a>(text: &'a str, node: &SynthNode) -> Option<&'a str> {
    let start_line = node.start_line_key().max(1) as usize;
    let end_line = node.end_line_key().max(start_line as i64) as usize;
    let mut start_idx = 0usize;
    let mut line = 1usize;
    for (idx, ch) in text.char_indices() {
        if line == start_line {
            start_idx = idx;
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    if line < start_line {
        return None;
    }
    let mut end_idx = text.len();
    for (idx, ch) in text[start_idx..].char_indices() {
        if line > end_line {
            end_idx = start_idx + idx;
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    Some(&text[start_idx..end_idx])
}

fn extract_fastapi_depends_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find("Depends") {
        let start = offset + pos;
        if !is_word_boundary(text, start, "Depends".len()) {
            offset = start + "Depends".len();
            continue;
        }
        let open = skip_ws(text, start + "Depends".len());
        if text.as_bytes().get(open) != Some(&b'(') {
            offset = start + "Depends".len();
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        let args = &text[open + 1..close];
        if let Some(dep) = first_depends_callable(args) {
            out.push(dep);
        }
        offset = close + 1;
    }
    out.sort();
    out.dedup();
    out
}

fn first_depends_callable(args: &str) -> Option<String> {
    for arg in split_top_level_commas(args) {
        let arg = arg.trim();
        if arg.is_empty() {
            continue;
        }
        let value = if let Some((key, value)) = arg.split_once('=') {
            if !matches!(key.trim(), "dependency" | "call" | "callable") {
                continue;
            }
            value.trim()
        } else {
            arg
        };
        let value = value.trim_start_matches("lambda ").trim();
        if value.starts_with('"') || value.starts_with('\'') || value.starts_with("None") {
            continue;
        }
        let name = value
            .split_once('(')
            .map(|(name, _)| name)
            .unwrap_or(value)
            .trim();
        if is_python_callable_expr(name) {
            return Some(name.to_string());
        }
    }
    None
}

fn is_python_callable_expr(expr: &str) -> bool {
    let mut parts = expr.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    is_python_identifier(first) && parts.all(is_python_identifier)
}

fn is_python_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn resolve_python_dependency_target<'a>(
    file_path: &str,
    dep: &str,
    by_file_name: &HashMap<(String, String), Vec<&'a SynthNode>>,
    by_name: &HashMap<String, Vec<&'a SynthNode>>,
) -> Option<&'a SynthNode> {
    let dep_name = dep.rsplit('.').next().unwrap_or(dep);
    by_file_name
        .get(&(file_path.to_string(), dep_name.to_string()))
        .and_then(|nodes| nodes.first().copied())
        .or_else(|| {
            by_name
                .get(dep_name)
                .and_then(|nodes| (nodes.len() == 1).then(|| nodes[0]))
        })
}

fn is_word_boundary(text: &str, start: usize, len: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[start + len..].chars().next();
    before.is_none_or(|ch| !is_ident_continue(ch)) && after.is_none_or(|ch| !is_ident_continue(ch))
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
    symbol_count: usize,
) -> Vec<SynthProcess> {
    if adjacency.is_empty() {
        return Vec::new();
    }

    let max_processes = dynamic_process_cap(symbol_count);
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
    let mut by_file = route_nodes_by_file(nodes);
    let python_prefixes = python_router_prefixes_by_file(repo, by_file.keys().map(String::as_str));
    let java_interface_routes = java_interface_routes_by_method(repo, nodes);
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
                method: None,
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
        let handler = pick_handler_node(file_nodes);
        let mut route_candidates = Vec::new();
        if let Some(route) = route_from_path(file_path) {
            route_candidates.push(RouteCandidate {
                route,
                method: None,
                handler_id: handler.map(|n| n.aka_id.clone()),
                handler_name: handler.map(|n| n.display_name().to_string()),
            });
        }
        if is_js_ts_route_source(file_path, file_nodes) {
            route_candidates.extend(extract_route_handler_literals(&text).into_iter().map(
                |route| RouteCandidate {
                    route,
                    method: None,
                    handler_id: handler.map(|n| n.aka_id.clone()),
                    handler_name: handler.map(|n| n.display_name().to_string()),
                },
            ));
        }
        route_candidates.extend(extract_annotated_routes(
            file_nodes,
            python_prefixes.get(file_path),
            &java_interface_routes,
        ));
        dedup_route_candidates(&mut route_candidates);
        if route_candidates.is_empty() {
            continue;
        }
        let response_keys = extract_response_keys(&text);
        let error_keys = extract_error_keys(&response_keys, &text);
        let middleware = extract_middleware(&text);
        for candidate in route_candidates {
            let route = candidate.route;
            let key = (route.clone(), file_path.clone());
            match routes.get_mut(&key) {
                Some(existing) => {
                    if existing.method.is_none() {
                        existing.method = candidate.method.clone();
                    }
                    if existing.handler_id.is_none() {
                        existing.handler_id = candidate.handler_id.clone();
                        existing.handler_name = candidate.handler_name.clone();
                    }
                    merge_strings(&mut existing.middleware, &middleware);
                    merge_strings(&mut existing.response_keys, &response_keys);
                    merge_strings(&mut existing.error_keys, &error_keys);
                    merge_strings(
                        &mut existing.process_ids,
                        &process_ids_for_entry(
                            processes,
                            file_path,
                            candidate.handler_id.as_deref(),
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
                            method: candidate.method,
                            handler_id: candidate.handler_id.clone(),
                            handler_name: candidate.handler_name,
                            middleware: middleware.clone(),
                            response_keys: response_keys.clone(),
                            error_keys: error_keys.clone(),
                            consumers: Vec::new(),
                            process_ids: process_ids_for_entry(
                                processes,
                                file_path,
                                candidate.handler_id.as_deref(),
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

fn string_literals(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some(byte) = text.as_bytes().get(idx).copied() else {
            break;
        };
        if matches!(byte, b'\'' | b'"' | b'`') {
            if let Some((literal, end)) = read_string_literal(text, idx) {
                if is_topic_literal(&literal) {
                    values.push(literal);
                }
                idx = end;
                continue;
            }
        }
        idx += 1;
    }
    values.sort();
    values.dedup();
    values
}

fn is_topic_literal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.starts_with('/')
        && !value.contains("://")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/'))
}

pub(super) fn web_nodes_by_file(
    nodes: &BTreeMap<String, SynthNode>,
) -> BTreeMap<String, Vec<&SynthNode>> {
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

fn route_nodes_by_file(nodes: &BTreeMap<String, SynthNode>) -> BTreeMap<String, Vec<&SynthNode>> {
    nodes_by_file(nodes)
        .into_iter()
        .filter(|(file_path, file_nodes)| {
            is_web_backend_source_path(file_path)
                || file_nodes.iter().any(|node| {
                    is_web_backend_language(&node.language)
                        || node.route_path.is_some()
                        || node.decorators.iter().any(|decorator| {
                            decorator.contains("Mapping")
                                || decorator.contains("RestController")
                                || decorator.contains(".route")
                                || decorator.contains(".get")
                                || decorator.contains(".post")
                        })
                })
        })
        .collect()
}

fn is_js_ts_route_source(file_path: &str, file_nodes: &[&SynthNode]) -> bool {
    is_js_ts_source_path(file_path)
        || file_nodes
            .iter()
            .any(|node| is_js_ts_language(&node.language))
}

fn is_js_ts_source_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    matches!(
        Path::new(&lower).extension().and_then(|ext| ext.to_str()),
        Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts")
    )
}

fn is_web_backend_source_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    is_js_ts_source_path(&lower)
        || matches!(
            Path::new(&lower).extension().and_then(|ext| ext.to_str()),
            Some("java" | "kt" | "kts" | "scala" | "groovy" | "py")
        )
}

fn is_js_ts_language(language: &str) -> bool {
    matches!(
        language.to_ascii_lowercase().as_str(),
        "javascript" | "typescript" | "tsx" | "jsx"
    )
}

fn is_web_backend_language(language: &str) -> bool {
    matches!(
        language.to_ascii_lowercase().as_str(),
        "java"
            | "kotlin"
            | "scala"
            | "groovy"
            | "python"
            | "javascript"
            | "typescript"
            | "tsx"
            | "jsx"
    )
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

fn extract_annotated_routes(
    nodes: &[&SynthNode],
    python_prefixes: Option<&PythonRoutePrefixes>,
    java_interface_routes: &HashMap<String, Vec<RouteCandidate>>,
) -> Vec<RouteCandidate> {
    let java_method_route_names: BTreeSet<String> = nodes
        .iter()
        .filter(|node| {
            matches!(node.label.as_str(), "Method")
                && !is_python_route_node(node)
                && (node.route_path.is_some() || spring_mapping_path(&node.decorators).is_some())
        })
        .map(|node| node.name.clone())
        .collect();
    let mut class_prefixes: BTreeMap<String, String> = BTreeMap::new();
    let mut owner_labels: BTreeMap<String, String> = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        owner_labels.insert(node.aka_id.clone(), node.label.clone());
        owner_labels.insert(node.qn.clone(), node.label.clone());
        let Some(prefix) = spring_mapping_path(&node.decorators) else {
            continue;
        };
        class_prefixes.insert(node.aka_id.clone(), prefix.clone());
        class_prefixes.insert(node.qn.clone(), prefix);
    }

    let mut routes = Vec::new();
    for node in nodes {
        let Some(method_path) = node
            .route_path
            .clone()
            .or_else(|| spring_mapping_path(&node.decorators))
        else {
            continue;
        };
        if !matches!(node.label.as_str(), "Function" | "Method") {
            if !method_path.is_empty() {
                routes.push(RouteCandidate {
                    route: normalize_route_literal(&method_path),
                    method: node.route_method.clone(),
                    handler_id: None,
                    handler_name: None,
                });
            }
            continue;
        }
        if !is_python_route_node(node) {
            if node.label == "Function" && java_method_route_names.contains(&node.name) {
                continue;
            }
            if node
                .parent_class
                .as_ref()
                .and_then(|parent| owner_labels.get(parent))
                .is_some_and(|label| label == "Interface")
            {
                continue;
            }
        }
        let prefixes: Vec<String> = if is_python_route_node(node) {
            python_route_prefixes_for_node(python_prefixes, node)
        } else {
            vec![node
                .parent_class
                .as_ref()
                .and_then(|parent| class_prefixes.get(parent))
                .map(String::as_str)
                .unwrap_or("")
                .to_string()]
        };
        for prefix in prefixes {
            routes.push(RouteCandidate {
                route: join_route_paths(&prefix, &method_path),
                method: node.route_method.clone(),
                handler_id: Some(node.aka_id.clone()),
                handler_name: Some(node.display_name().to_string()),
            });
        }
    }
    routes.extend(inherited_java_interface_routes(
        nodes,
        java_interface_routes,
    ));
    routes
}

fn java_interface_routes_by_method(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> HashMap<String, Vec<RouteCandidate>> {
    let mut owner_labels: BTreeMap<String, String> = BTreeMap::new();
    let mut class_prefixes: BTreeMap<String, String> = BTreeMap::new();
    let mut type_aliases: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in nodes
        .values()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        let aliases = java_type_aliases(node);
        for alias in &aliases {
            owner_labels.insert(alias.clone(), node.label.clone());
        }
        type_aliases.insert(node.aka_id.clone(), aliases);
        if let Some(prefix) = spring_mapping_path(&node.decorators) {
            for alias in java_type_aliases(node) {
                class_prefixes.insert(alias, prefix.clone());
            }
        }
    }

    let mut out: HashMap<String, Vec<RouteCandidate>> = HashMap::new();
    for node in nodes.values().filter(|node| {
        matches!(node.label.as_str(), "Function" | "Method")
            && !is_python_route_node(node)
            && node
                .parent_class
                .as_ref()
                .and_then(|parent| owner_labels.get(parent))
                .is_some_and(|label| label == "Interface")
    }) {
        let Some(method_path) = node
            .route_path
            .clone()
            .or_else(|| spring_mapping_path(&node.decorators))
        else {
            continue;
        };
        let prefix = node
            .parent_class
            .as_ref()
            .and_then(|parent| class_prefixes.get(parent))
            .map(String::as_str)
            .unwrap_or("");
        let candidate = RouteCandidate {
            route: join_route_paths(prefix, &method_path),
            method: node.route_method.clone(),
            handler_id: Some(node.aka_id.clone()),
            handler_name: Some(node.display_name().to_string()),
        };
        for key in java_method_route_keys(node) {
            out.entry(key).or_default().push(candidate.clone());
        }
    }
    let interface_routes = out.clone();
    for class_node in nodes
        .values()
        .filter(|node| matches!(node.label.as_str(), "Class"))
    {
        let Some(text) = read_repo_text(repo, &class_node.file_path) else {
            continue;
        };
        let implemented = java_implemented_interfaces(&text, &class_node.name);
        if implemented.is_empty() {
            continue;
        }
        let class_aliases = type_aliases
            .get(&class_node.aka_id)
            .cloned()
            .unwrap_or_else(|| java_type_aliases(class_node));
        for method in nodes.values().filter(|node| {
            matches!(node.label.as_str(), "Function" | "Method")
                && node
                    .parent_class
                    .as_ref()
                    .is_some_and(|parent| class_aliases.iter().any(|alias| alias == parent))
                && node.route_path.is_none()
                && spring_mapping_path(&node.decorators).is_none()
        }) {
            for iface in &implemented {
                let possible = possible_interface_method_ids_from_name(
                    &class_node.qn,
                    &class_node.name,
                    iface,
                    &method.name,
                );
                for iface_method in possible {
                    if let Some(routes) = interface_routes.get(&iface_method) {
                        out.entry(method.aka_id.clone())
                            .or_default()
                            .extend(routes.clone());
                        out.entry(method.qn.clone())
                            .or_default()
                            .extend(routes.clone());
                    }
                }
            }
        }
    }
    for routes in out.values_mut() {
        dedup_route_candidates(routes);
    }
    out
}

fn java_type_aliases(node: &SynthNode) -> Vec<String> {
    let mut aliases = vec![node.aka_id.clone(), node.qn.clone()];
    let qn = strip_aka_node_prefix(&node.qn);
    aliases.extend(possible_java_type_qns(qn));
    aliases.push(format!("{}.{name}", qn, name = node.name));
    if let Some((_, simple)) = qn.rsplit_once('.') {
        aliases.push(simple.to_string());
        aliases.push(format!("{}.{}", simple, node.name));
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn java_method_route_keys(node: &SynthNode) -> Vec<String> {
    let mut keys = vec![node.aka_id.clone(), node.qn.clone()];
    let qn = strip_aka_node_prefix(&node.qn);
    keys.push(qn.to_string());
    if let Some(parent) = node.parent_class.as_ref() {
        for owner in possible_java_type_qns(parent) {
            keys.push(format!("{owner}.{method}", method = node.name));
        }
    }
    if let Some(owner) = semantic_owner_qn(qn, &node.name) {
        keys.push(format!("{owner}.{method}", method = node.name));
    }
    keys.sort();
    keys.dedup();
    keys
}

fn possible_java_type_qns(type_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let qn = strip_aka_node_prefix(type_id);
    out.push(qn.to_string());
    if let Some((pkg, simple)) = qn.rsplit_once('.') {
        if let Some((base, duplicate)) = pkg.rsplit_once('.') {
            if duplicate == simple {
                out.push(pkg.to_string());
                out.push(base.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn java_implemented_interfaces(text: &str, class_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for class_pos in literal_occurrences(text, class_name) {
        let tail = &text[class_pos + class_name.len()..];
        let Some(implements_pos) = tail.find("implements") else {
            continue;
        };
        if tail[..implements_pos].contains('{') || tail[..implements_pos].contains(';') {
            continue;
        }
        let after = &tail[implements_pos + "implements".len()..];
        let end = after
            .find('{')
            .or_else(|| after.find("extends"))
            .unwrap_or(after.len());
        for raw in after[..end].split(',') {
            let name = raw
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim()
                .trim_end_matches('{');
            let name = name
                .split('<')
                .next()
                .unwrap_or(name)
                .rsplit('.')
                .next()
                .unwrap_or(name);
            if name.chars().next().is_some_and(is_ident_start) {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn inherited_java_interface_routes(
    nodes: &[&SynthNode],
    interface_method_routes: &HashMap<String, Vec<RouteCandidate>>,
) -> Vec<RouteCandidate> {
    if interface_method_routes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for node in nodes.iter().filter(|node| {
        matches!(node.label.as_str(), "Function" | "Method")
            && !is_python_route_node(node)
            && node.route_path.is_none()
            && spring_mapping_path(&node.decorators).is_none()
    }) {
        let mut inherited = Vec::new();
        for iface in possible_interface_method_ids(node) {
            if let Some(routes) = interface_method_routes.get(&iface) {
                inherited.extend(routes.clone());
            }
        }
        inherited.sort_by(|a, b| a.route.cmp(&b.route).then_with(|| a.method.cmp(&b.method)));
        inherited.dedup_by(|a, b| a.route == b.route && a.method == b.method);
        for route in inherited {
            out.push(RouteCandidate {
                route: route.route,
                method: route.method,
                handler_id: Some(node.aka_id.clone()),
                handler_name: Some(node.display_name().to_string()),
            });
        }
    }
    out
}

fn possible_interface_method_ids(node: &SynthNode) -> Vec<String> {
    let mut out = Vec::new();
    let Some(owner) = node.parent_class.as_ref() else {
        return out;
    };
    for iface_owner in possible_interface_owner_ids(owner) {
        out.push(format!("{iface_owner}.{}", node.name));
        if let Some(stripped) = iface_owner.strip_prefix("cbm:") {
            let mut parts = stripped.splitn(2, ':');
            if let (Some(id), Some(qn)) = (parts.next(), parts.next()) {
                out.push(format!("cbm:{id}:{qn}.{}", node.name));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn possible_interface_method_ids_from_name(
    class_owner_qn: &str,
    class_name: &str,
    interface_name: &str,
    method_name: &str,
) -> Vec<String> {
    let owner_qn = strip_aka_node_prefix(class_owner_qn);
    let pkg = owner_qn
        .strip_suffix(class_name)
        .and_then(|prefix| prefix.strip_suffix('.'))
        .unwrap_or_else(|| owner_qn.rsplit_once('.').map(|(pkg, _)| pkg).unwrap_or(""));
    let iface_qn = if pkg.is_empty() {
        interface_name.to_string()
    } else {
        format!("{pkg}.{interface_name}")
    };
    let mut out = vec![format!("{iface_qn}.{method_name}")];
    for class_owner in possible_java_type_qns(class_owner_qn) {
        if let Some((pkg, _)) = class_owner.rsplit_once('.') {
            out.push(format!("{pkg}.{interface_name}.{method_name}"));
        }
    }
    out.extend(
        possible_interface_owner_ids(class_owner_qn)
            .into_iter()
            .map(|owner| format!("{owner}.{method_name}")),
    );
    out.sort();
    out.dedup();
    out
}

fn possible_interface_owner_ids(owner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let owner_qn = strip_aka_node_prefix(owner);
    let Some((pkg, class_name)) = owner_qn.rsplit_once('.') else {
        return out;
    };
    for suffix in [
        "Controller",
        "ControllerImpl",
        "Resource",
        "ResourceImpl",
        "Endpoint",
    ] {
        if let Some(base) = class_name.strip_suffix(suffix) {
            if !base.is_empty() {
                out.push(format!("{pkg}.{base}Api"));
                out.push(format!("{pkg}.{base}Controller"));
                out.push(format!("{pkg}.{base}Resource"));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn strip_aka_node_prefix(id_or_qn: &str) -> &str {
    if let Some(rest) = id_or_qn.strip_prefix("cbm:") {
        let mut parts = rest.splitn(2, ':');
        if let (Some(_id), Some(qn)) = (parts.next(), parts.next()) {
            return qn;
        }
    }
    id_or_qn
}

fn python_route_prefixes_for_node(
    python_prefixes: Option<&PythonRoutePrefixes>,
    node: &SynthNode,
) -> Vec<String> {
    let Some(prefixes) = python_prefixes else {
        return vec![String::new()];
    };
    let include_prefixes: Vec<String> = if prefixes.include.is_empty() {
        vec![String::new()]
    } else {
        prefixes.include.clone()
    };
    let local_prefix = python_route_local_prefix(prefixes, node);
    include_prefixes
        .into_iter()
        .map(|prefix| {
            if let Some(local) = local_prefix {
                join_route_paths(&prefix, local)
            } else {
                prefix
            }
        })
        .collect()
}

fn python_route_local_prefix<'a>(
    prefixes: &'a PythonRoutePrefixes,
    node: &SynthNode,
) -> Option<&'a str> {
    router_name_from_python_decorators(&node.decorators)
        .and_then(|router| prefixes.local_by_router.get(router))
        .map(String::as_str)
}

fn is_python_route_node(node: &SynthNode) -> bool {
    node.language.eq_ignore_ascii_case("python")
        || node.file_path.to_ascii_lowercase().ends_with(".py")
}

fn python_router_prefixes_by_file<'a>(
    repo: &Path,
    file_paths: impl Iterator<Item = &'a str>,
) -> BTreeMap<String, PythonRoutePrefixes> {
    let mut python_files: Vec<String> = file_paths
        .filter(|path| path.to_ascii_lowercase().ends_with(".py"))
        .map(str::to_string)
        .collect();
    python_files.extend(repo_python_source_files(repo));
    python_files.sort();
    python_files.dedup();
    if python_files.is_empty() {
        return BTreeMap::new();
    }

    let mut short_to_files: HashMap<String, Vec<String>> = HashMap::new();
    let mut long_to_files: HashMap<String, Vec<String>> = HashMap::new();
    for file_path in &python_files {
        short_to_files
            .entry(python_file_short_key(file_path))
            .or_default()
            .push(file_path.clone());
        if let Some(long_key) = python_file_long_key(file_path) {
            long_to_files
                .entry(long_key)
                .or_default()
                .push(file_path.clone());
        }
    }

    let mut out: BTreeMap<String, PythonRoutePrefixes> = BTreeMap::new();
    for file_path in &python_files {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        let local_by_router = extract_python_local_router_prefixes(&text);
        if !local_by_router.is_empty() {
            out.entry(file_path.clone()).or_default().local_by_router = local_by_router;
        }
        let imports = python_router_imports(&text);
        for include in extract_python_router_includes(&text) {
            let targets: Vec<String> =
                if let Some((module_expr, _)) = python_router_module_expr(&include.router_expr) {
                    let module_name = module_expr.rsplit('.').next().unwrap_or(module_expr);
                    imports
                        .module_aliases
                        .get(module_name)
                        .and_then(|long_key| long_to_files.get(long_key))
                        .cloned()
                        .or_else(|| short_to_files.get(module_name).cloned())
                        .unwrap_or_default()
                } else if let Some(import) = imports.router_names.get(&include.router_expr) {
                    if let Some(long_key) = &import.long_key {
                        long_to_files.get(long_key).cloned().unwrap_or_default()
                    } else {
                        short_to_files
                            .get(&import.short_key)
                            .cloned()
                            .unwrap_or_default()
                    }
                } else {
                    Vec::new()
                };
            for target in targets {
                out.entry(target)
                    .or_default()
                    .include
                    .push(normalize_route_literal(&include.prefix));
            }
        }
    }
    for prefixes in out.values_mut() {
        prefixes.include.sort();
        prefixes.include.dedup();
    }
    out
}

#[derive(Debug, Default)]
struct PythonRoutePrefixes {
    include: Vec<String>,
    local_by_router: HashMap<String, String>,
}

fn repo_python_source_files(repo: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect_repo_python_source_files(repo, repo, &mut out);
    out
}

fn collect_repo_python_source_files(repo: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if file_type.is_dir() {
            if is_source_discovery_skip_dir(name) {
                continue;
            }
            collect_repo_python_source_files(repo, &path, out);
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|v| v.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("py"))
        {
            let Ok(rel) = path.strip_prefix(repo) else {
                continue;
            };
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[derive(Debug)]
struct PythonIncludeRouter {
    router_expr: String,
    prefix: String,
}

#[derive(Debug)]
struct PythonRouterImport {
    short_key: String,
    long_key: Option<String>,
}

#[derive(Debug, Default)]
struct PythonRouterImports {
    router_names: HashMap<String, PythonRouterImport>,
    module_aliases: HashMap<String, String>,
}

fn python_router_imports(text: &str) -> PythonRouterImports {
    let mut imports = PythonRouterImports::default();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("from ") else {
            continue;
        };
        let Some((module, imported)) = rest.split_once(" import ") else {
            continue;
        };
        for item in imported.split(',') {
            let item = item.trim();
            if item.is_empty() || item == "*" {
                continue;
            }
            let (name, alias) = split_python_import_alias(item);
            if is_python_router_export_name(name) {
                let local = alias.unwrap_or(name);
                let short_key = module
                    .trim_start_matches('.')
                    .rsplit('.')
                    .next()
                    .unwrap_or(module)
                    .to_string();
                imports.router_names.insert(
                    local.to_string(),
                    PythonRouterImport {
                        short_key,
                        long_key: python_module_long_key(module),
                    },
                );
            } else if let Some(long_key) = python_module_long_key(&format!("{module}.{name}")) {
                imports
                    .module_aliases
                    .insert(alias.unwrap_or(name).to_string(), long_key);
            }
        }
    }
    imports
}

fn is_python_router_export_name(name: &str) -> bool {
    matches!(name, "router" | "bp" | "blueprint")
}

fn python_router_module_expr(expr: &str) -> Option<(&str, &str)> {
    let (module_expr, export_name) = expr.rsplit_once('.')?;
    is_python_router_export_name(export_name).then_some((module_expr, export_name))
}

fn extract_python_local_router_prefixes(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    extend_python_constructor_prefixes(text, "APIRouter", "prefix", &mut out);
    extend_python_constructor_prefixes(text, "Blueprint", "url_prefix", &mut out);
    out
}

fn extend_python_constructor_prefixes(
    text: &str,
    constructor: &str,
    prefix_kw: &str,
    out: &mut HashMap<String, String>,
) {
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(constructor) {
        let name_start = offset + pos;
        let name_end = name_start + constructor.len();
        let Some(open_rel) = text[name_start..].find('(') else {
            break;
        };
        let open = name_start + open_rel;
        if !text[name_end..open].trim().is_empty() {
            offset = name_end;
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        if let Some(router_name) = assigned_name_before_call(text, name_start) {
            let args = &text[open + 1..close];
            if let Some(prefix) = keyword_string_arg(args, prefix_kw) {
                out.insert(router_name, normalize_route_literal(&prefix));
            }
        }
        offset = close + 1;
    }
}

fn assigned_name_before_call(text: &str, call_start: usize) -> Option<String> {
    let line_start = text[..call_start].rfind('\n').map_or(0, |idx| idx + 1);
    let before = text[line_start..call_start].trim();
    let lhs = before.split_once('=')?.0.trim();
    if lhs.contains(' ') || lhs.contains('.') || lhs.is_empty() {
        return None;
    }
    Some(lhs.to_string()).filter(|name| name.chars().all(|ch| ch == '_' || ch.is_alphanumeric()))
}

fn router_name_from_python_decorators(decorators: &[String]) -> Option<&str> {
    decorators.iter().find_map(|decorator| {
        let text = decorator.trim().trim_start_matches('@');
        let (receiver, method) = text.split_once('.')?;
        let method = method
            .split_once('(')
            .map(|(name, _)| name)
            .unwrap_or(method);
        if matches!(
            method,
            "get"
                | "post"
                | "put"
                | "patch"
                | "delete"
                | "head"
                | "options"
                | "api_route"
                | "route"
        ) {
            Some(receiver)
        } else {
            None
        }
    })
}

fn split_python_import_alias(item: &str) -> (&str, Option<&str>) {
    if let Some((name, alias)) = item.split_once(" as ") {
        (name.trim(), Some(alias.trim()))
    } else {
        (item.trim(), None)
    }
}

fn extract_python_router_includes(text: &str) -> Vec<PythonIncludeRouter> {
    let mut out = Vec::new();
    if text.contains("include_router") {
        out.extend(extract_python_router_mounts(
            text,
            ".include_router",
            "prefix",
        ));
    }
    if text.contains("register_blueprint") {
        out.extend(extract_python_router_mounts(
            text,
            ".register_blueprint",
            "url_prefix",
        ));
    }
    out
}

fn extract_python_router_mounts(
    text: &str,
    call_name: &str,
    prefix_kw: &str,
) -> Vec<PythonIncludeRouter> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(call_name) {
        let call_start = offset + pos;
        let Some(open_rel) = text[call_start..].find('(') else {
            break;
        };
        let open = call_start + open_rel;
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        let args = &text[open + 1..close];
        if let (Some(router_expr), Some(prefix)) = (
            first_call_argument(args),
            keyword_string_arg(args, prefix_kw),
        ) {
            out.push(PythonIncludeRouter {
                router_expr,
                prefix,
            });
        }
        offset = close + 1;
    }
    out
}

fn first_call_argument(args: &str) -> Option<String> {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    for (idx, byte) in args.bytes().enumerate() {
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == q {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => return clean_python_expr(&args[..idx]),
            _ => {}
        }
    }
    clean_python_expr(args)
}

fn clean_python_expr(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if expr.is_empty() || expr.contains('=') {
        None
    } else {
        Some(expr.to_string())
    }
}

fn keyword_string_arg(args: &str, keyword: &str) -> Option<String> {
    let needle = format!("{keyword}=");
    let compact = args.replace(' ', "");
    let pos = compact.find(&needle)?;
    let start = pos + needle.len();
    read_string_literal(&compact, start).map(|(literal, _)| literal)
}

fn python_file_short_key(rel: &str) -> String {
    let normalized = rel.replace('\\', "/");
    let file = normalized.rsplit('/').next().unwrap_or(&normalized);
    file.strip_suffix(".py").unwrap_or(file).to_string()
}

fn python_file_long_key(rel: &str) -> Option<String> {
    let normalized = rel.replace('\\', "/");
    let no_ext = normalized.strip_suffix(".py").unwrap_or(&normalized);
    let (parent_path, stem) = no_ext.rsplit_once('/')?;
    let parent = parent_path.rsplit('/').next().unwrap_or(parent_path);
    Some(format!("{parent}/{stem}"))
}

fn python_module_long_key(module: &str) -> Option<String> {
    let stripped = module.trim_start_matches('.');
    let (parent_path, stem) = stripped.rsplit_once('.')?;
    let parent = parent_path.rsplit('.').next().unwrap_or(parent_path);
    Some(format!("{parent}/{stem}"))
}

fn dedup_route_candidates(candidates: &mut Vec<RouteCandidate>) {
    candidates.sort_by(|a, b| {
        a.route
            .cmp(&b.route)
            .then_with(|| a.method.cmp(&b.method))
            .then_with(|| a.handler_id.cmp(&b.handler_id))
    });
    candidates.dedup_by(|a, b| {
        if a.route == b.route {
            if b.method.is_none() {
                b.method = a.method.clone();
            }
            if b.handler_id.is_none() {
                b.handler_id = a.handler_id.clone();
                b.handler_name = a.handler_name.clone();
            }
            true
        } else {
            false
        }
    });
}

fn spring_mapping_path(decorators: &[String]) -> Option<String> {
    for decorator in decorators {
        let Some(name_end) = decorator.find('(') else {
            continue;
        };
        let name = decorator[..name_end].trim_start_matches('@');
        if !matches!(
            name.rsplit('.').next().unwrap_or(name),
            "RequestMapping"
                | "GetMapping"
                | "PostMapping"
                | "PutMapping"
                | "DeleteMapping"
                | "PatchMapping"
        ) {
            continue;
        }
        let args_start = name_end + 1;
        let args_end = decorator.rfind(')').unwrap_or(decorator.len());
        if args_start >= args_end {
            return Some("/".into());
        }
        let args = &decorator[args_start..args_end];
        if let Some(path) = first_route_literal(args) {
            return Some(normalize_route_literal(&path));
        }
        return Some("/".into());
    }
    None
}

fn feign_client_path(decorators: &[String]) -> Option<String> {
    for decorator in decorators {
        let Some(name_end) = decorator.find('(') else {
            continue;
        };
        let name = decorator[..name_end].trim_start_matches('@');
        if name.rsplit('.').next().unwrap_or(name) != "FeignClient" {
            continue;
        }
        let args_start = name_end + 1;
        let args_end = decorator.rfind(')').unwrap_or(decorator.len());
        if args_start >= args_end {
            return Some("/".into());
        }
        let args = &decorator[args_start..args_end];
        let path = keyword_string_arg(args, "path")
            .or_else(|| keyword_string_arg(args, "url"))
            .or_else(|| first_route_literal(args));
        if let Some(path) = path {
            return Some(normalize_route_literal(&path));
        }
        return Some("/".into());
    }
    None
}

fn request_line_path(decorator: &str) -> Option<String> {
    let name_end = decorator.find('(')?;
    let name = decorator[..name_end].trim_start_matches('@');
    if name.rsplit('.').next().unwrap_or(name) != "RequestLine" {
        return None;
    }
    let args_start = name_end + 1;
    let args_end = decorator.rfind(')').unwrap_or(decorator.len());
    if args_start >= args_end {
        return None;
    }
    let literal = first_route_literal(&decorator[args_start..args_end])?;
    parse_request_line_path(&literal)
}

fn parse_request_line_path(literal: &str) -> Option<String> {
    let mut parts = literal.split_whitespace();
    let method = parts.next()?;
    if !matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
    ) {
        return None;
    }
    let path = parts.next()?;
    let path = path.split_once('?').map(|(path, _)| path).unwrap_or(path);
    path.starts_with('/').then(|| normalize_route_literal(path))
}

fn first_route_literal(text: &str) -> Option<String> {
    let mut i = 0usize;
    while i < text.len() {
        if let Some((literal, end)) = read_string_literal(text, i) {
            if literal.starts_with('/') || literal.starts_with('{') || !literal.contains('=') {
                return Some(literal);
            }
            i = end;
        } else {
            i += text[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        }
    }
    None
}

fn join_route_paths(prefix: &str, suffix: &str) -> String {
    let prefix = normalize_route_literal(prefix);
    let suffix = normalize_route_literal(suffix);
    if prefix.is_empty() || prefix == "/" {
        return if suffix.is_empty() {
            "/".into()
        } else if suffix.starts_with('/') {
            suffix
        } else {
            format!("/{suffix}")
        };
    }
    if suffix.is_empty() || suffix == "/" {
        return prefix;
    }
    format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    )
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
    let by_file = route_nodes_by_file(nodes);
    let route_names: Vec<String> = routes.keys().map(|(route, _)| route.clone()).collect();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        let Some(consumer) = pick_handler_node(&file_nodes) else {
            continue;
        };
        for (route, node_id) in java_feign_route_consumers(&file_nodes) {
            for candidate in routes.values_mut().filter(|r| r.route == route) {
                if candidate.file_path == file_path {
                    continue;
                }
                candidate.consumers.push(SynthRouteConsumer {
                    node_id: node_id.clone(),
                    keys: Vec::new(),
                    fetch_count: 1,
                });
            }
        }
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
    remove_parent_route_consumers(routes);
}

fn remove_parent_route_consumers(routes: &mut BTreeMap<(String, String), SynthRoute>) {
    let route_names: Vec<String> = routes.values().map(|route| route.route.clone()).collect();
    let mut removals: HashMap<String, BTreeSet<String>> = HashMap::new();
    for route in routes.values() {
        let parent_routes = parent_routes_for(&route.route, &route_names);
        if parent_routes.is_empty() {
            continue;
        }
        for consumer in &route.consumers {
            for parent in &parent_routes {
                removals
                    .entry(parent.clone())
                    .or_default()
                    .insert(consumer.node_id.clone());
            }
        }
    }
    for route in routes.values_mut() {
        let Some(remove_consumers) = removals.get(&route.route) else {
            continue;
        };
        route
            .consumers
            .retain(|consumer| !remove_consumers.contains(&consumer.node_id));
    }
}

fn parent_routes_for(route: &str, all_routes: &[String]) -> Vec<String> {
    if !route
        .split('/')
        .filter(|segment| !segment.is_empty())
        .any(is_route_parameter_segment)
    {
        return Vec::new();
    }
    let mut parents = Vec::new();
    let mut current = route.trim_end_matches('/').to_string();
    while let Some((parent, _)) = current.rsplit_once('/') {
        if parent.is_empty() {
            break;
        }
        if all_routes.iter().any(|route| route == parent) {
            parents.push(parent.to_string());
        }
        current = parent.to_string();
    }
    parents
}

fn java_feign_route_consumers(nodes: &[&SynthNode]) -> Vec<(String, String)> {
    let mut feign_prefixes: BTreeMap<String, String> = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        let Some(prefix) = feign_client_path(&node.decorators) else {
            continue;
        };
        feign_prefixes.insert(node.aka_id.clone(), prefix.clone());
        feign_prefixes.insert(node.qn.clone(), prefix);
    }

    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        let Some(parent) = node.parent_class.as_ref() else {
            continue;
        };
        let Some(prefix) = feign_prefixes.get(parent) else {
            continue;
        };
        if let Some(route) = node
            .decorators
            .iter()
            .find_map(|decorator| request_line_path(decorator))
        {
            out.push((join_route_paths(prefix, &route), node.aka_id.clone()));
            continue;
        }
        if let Some(method_path) = node
            .route_path
            .clone()
            .or_else(|| spring_mapping_path(&node.decorators))
        {
            out.push((join_route_paths(prefix, &method_path), node.aka_id.clone()));
        }
    }
    out.sort();
    out.dedup();
    out
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
        let mut matches = BTreeSet::new();
        for (window_start, window) in &fetch_windows {
            matches.extend(
                route_occurrences(window, route)
                    .into_iter()
                    .map(|idx| window_start + idx),
            );
        }
        let count = matches.len() as u32;
        if count > 0 {
            out.push((route, count));
        }
    }
    out
}

pub(super) fn clamp_char_boundary(text: &str, idx: usize) -> usize {
    let mut idx = idx.min(text.len());
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

pub(super) fn process_ids_for_entry(
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
    let mut out = normalize_flask_route_params(route.trim());
    if out.len() > 1 {
        while out.ends_with('/') {
            out.pop();
        }
    }
    out
}

fn normalize_flask_route_params(route: &str) -> String {
    let mut out = String::with_capacity(route.len());
    let mut rest = route;
    while let Some(open) = rest.find('<') {
        let (before, after_open) = rest.split_at(open);
        out.push_str(before);
        let after_open = &after_open[1..];
        let Some(close) = after_open.find('>') else {
            out.push('<');
            out.push_str(after_open);
            return out;
        };
        let raw_param = &after_open[..close];
        let param = raw_param
            .rsplit_once(':')
            .map(|(_, name)| name)
            .unwrap_or(raw_param)
            .trim();
        if param.is_empty() {
            out.push('<');
            out.push_str(raw_param);
            out.push('>');
        } else {
            out.push('{');
            out.push_str(param);
            out.push('}');
        }
        rest = &after_open[close + 1..];
    }
    out.push_str(rest);
    out
}

fn trim_route_suffix(route: &str) -> &str {
    route
        .strip_suffix("/route")
        .or_else(|| route.strip_suffix("/index"))
        .unwrap_or(route)
}

pub(super) fn merge_strings(target: &mut Vec<String>, source: &[String]) {
    target.extend(source.iter().cloned());
    target.sort();
    target.dedup();
}

pub(super) fn property_name_offsets(text: &str, name: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = text[search_from..].find(name) {
        let i = search_from + rel;
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
        search_from = i + name.len();
    }
    out
}

pub(super) fn read_string_literal(text: &str, start: usize) -> Option<(String, usize)> {
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
            let ch = text[i..].chars().next()?;
            out.push(ch);
            escape = false;
            i += ch.len_utf8();
            continue;
        }
        if b == b'\\' {
            escape = true;
        } else if b == quote {
            return Some((out, i + 1));
        } else {
            let ch = text[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
    }
    None
}

pub(super) fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || matches!(ch, '_' | '$')
}

impl SynthNode {
    fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.aka_id
        } else {
            &self.name
        }
    }

    fn start_line_key(&self) -> i64 {
        self.start_line
    }

    fn end_line_key(&self) -> i64 {
        self.end_line
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

    fn read_file(&mut self, file_path: &str) -> Option<String> {
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
        self.files.get(file_path).map(|lines| lines.join("\n"))
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
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).arg("rev-parse").arg("HEAD");
    hide_child_console(&mut cmd);
    let output = cmd.output().ok()?;
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
mod tests;
