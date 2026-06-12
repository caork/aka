//! Engine runner backed by codebase-memory-mcp.
//!
//! `aka` still consumes the artifact contract in `docs/contracts/artifacts.md`,
//! but the producer is now the native C codebase-memory indexer instead of the
//! previous parser sidecar.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use chrono::Utc;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};
use serde_json::{json, Map, Value};

use crate::types::{ArtifactStats, EdgeRec, EngineEvent, Manifest, NodeRec, CONTRACT_VERSION};

const DEFAULT_CBM_MODE: &str = "fast";
const PROCESS_MAX_STARTS: usize = 256;
const PROCESS_MAX_COUNT: usize = 1000;
const PROCESS_MAX_STEPS: usize = 8;
const PROCESS_BRANCH_LIMIT: usize = 2;

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
        let stderr_handle = std::thread::spawn(move || {
            let mut tail: Vec<String> = Vec::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
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
        }

        let status = wait_for_done_exit(&mut child, Self::DONE_EXIT_GRACE)?;
        let stderr_tail = stderr_handle.join().unwrap_or_default();
        if !status.success() {
            return Err(EngineError::Failed {
                code: status.code(),
                stderr_tail,
            });
        }

        emit_phase(&mut on_event, "codebase-memory:export-artifacts", 0, 0);
        let (project, db_path) = find_single_project_db(&cache_root)?;
        let stats = export_artifacts(repo, out_dir, &db_path, &project, no_chunks)?;
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
) -> Result<ArtifactStats, EngineError> {
    let conn = open_cbm_db(db_path)?;
    let processes = synthesize_processes(&conn, project)?;
    let mut stats = ArtifactStats {
        files: count_files(&conn, project)?,
        nodes: export_nodes(&conn, project, &out_dir.join("nodes.ndjson"), &processes)?,
        edges: export_edges(&conn, project, &out_dir.join("edges.ndjson"), &processes)?,
        chunks: 0,
    };
    if no_chunks {
        let _ = std::fs::remove_file(out_dir.join("chunks.ndjson"));
    } else {
        stats.chunks = export_chunks(&conn, project, repo, &out_dir.join("chunks.ndjson"))?;
    }

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
    processes: &[SynthProcess],
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
        let name = text_col(row, 2)?;
        let qn = text_col(row, 3)?;
        let file_path = text_col(row, 4)?;
        let start_line: i64 = row.get(5)?;
        let end_line: i64 = row.get(6)?;
        let props_text = text_col(row, 7)?;

        let mut properties = parse_props(&props_text);
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
    }
    for process in processes {
        let node = process.node_rec();
        serde_json::to_writer(&mut out, &node)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    Ok(count)
}

fn export_edges(
    conn: &Connection,
    project: &str,
    path: &Path,
    processes: &[SynthProcess],
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
    }
    for edge in processes.iter().flat_map(SynthProcess::edge_recs) {
        serde_json::to_writer(&mut out, &edge)?;
        out.write_all(b"\n")?;
        count += 1;
    }
    Ok(count)
}

#[derive(Debug, Clone)]
struct SynthNode {
    aka_id: String,
    label: String,
    name: String,
    file_path: String,
}

#[derive(Debug, Clone)]
struct SynthProcess {
    id: String,
    name: String,
    steps: Vec<SynthNode>,
}

impl SynthProcess {
    fn node_rec(&self) -> NodeRec {
        let entry = self.steps.first().expect("process has entry");
        let terminal = self.steps.last().expect("process has terminal");
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("processType".into(), Value::String("call-chain".into()));
        properties.insert("stepCount".into(), Value::from(self.steps.len() as u64));
        properties.insert("entryPointId".into(), Value::String(entry.aka_id.clone()));
        properties.insert("terminalId".into(), Value::String(terminal.aka_id.clone()));
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

fn synthesize_processes(
    conn: &Connection,
    project: &str,
) -> Result<Vec<SynthProcess>, EngineError> {
    let native_process_steps: u64 = conn.query_row(
        "SELECT COUNT(*) \
         FROM edges e \
         JOIN nodes p ON p.id = e.target_id AND p.project = e.project \
         WHERE e.project = ?1 AND p.label = 'Process' AND UPPER(e.type) = 'STEP_IN_PROCESS'",
        [project],
        |row| row.get(0),
    )?;
    if native_process_steps > 0 {
        return Ok(Vec::new());
    }

    let mut nodes = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT id, label, name, qualified_name, file_path \
         FROM nodes WHERE project = ?1 ORDER BY id",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let cbm_id: i64 = row.get(0)?;
        let label = text_col(row, 1)?;
        let name = text_col(row, 2)?;
        let qn = text_col(row, 3)?;
        let file_path = text_col(row, 4)?;
        if !is_process_step_label(&label) || is_noisy_source_path(&file_path) {
            continue;
        }
        let aka_id = aka_node_id(cbm_id, &qn);
        nodes.insert(
            aka_id.clone(),
            SynthNode {
                aka_id,
                label,
                name,
                file_path,
            },
        );
    }
    if nodes.is_empty() {
        return Ok(Vec::new());
    }

    let mut adjacency: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut indegree: BTreeMap<String, usize> = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT e.source_id, e.target_id, s.qualified_name, t.qualified_name \
         FROM edges e \
         JOIN nodes s ON s.id = e.source_id \
         JOIN nodes t ON t.id = e.target_id \
         WHERE e.project = ?1 AND UPPER(e.type) = 'CALLS' \
         ORDER BY e.id",
    )?;
    let mut rows = stmt.query([project])?;
    while let Some(row) = rows.next()? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let source_qn = text_col(row, 2)?;
        let target_qn = text_col(row, 3)?;
        let source = aka_node_id(source_id, &source_qn);
        let target = aka_node_id(target_id, &target_qn);
        if !nodes.contains_key(&source) || !nodes.contains_key(&target) || source == target {
            continue;
        }
        adjacency
            .entry(source.clone())
            .or_default()
            .insert(target.clone());
        *indegree.entry(target).or_default() += 1;
    }
    if adjacency.is_empty() {
        return Ok(Vec::new());
    }

    let mut starts: Vec<String> = adjacency
        .keys()
        .filter(|id| *indegree.get(*id).unwrap_or(&0) == 0)
        .cloned()
        .collect();
    if starts.is_empty() {
        starts = adjacency
            .keys()
            .filter(|id| is_hard_entry_name(&nodes[*id].name))
            .cloned()
            .collect();
    }
    if starts.is_empty() {
        starts = adjacency.keys().cloned().collect();
    }
    starts.sort_by(|a, b| {
        let na = &nodes[a];
        let nb = &nodes[b];
        entry_score(nb, *indegree.get(b).unwrap_or(&0))
            .cmp(&entry_score(na, *indegree.get(a).unwrap_or(&0)))
            .then_with(|| na.file_path.cmp(&nb.file_path))
            .then_with(|| na.name.cmp(&nb.name))
            .then_with(|| a.cmp(b))
    });
    starts.truncate(PROCESS_MAX_STARTS);

    let mut processes = Vec::new();
    let mut seen = BTreeSet::new();
    for start in starts {
        let mut path = vec![start.clone()];
        collect_chains(
            &start,
            &nodes,
            &adjacency,
            &mut path,
            &mut seen,
            &mut processes,
        );
        if processes.len() >= PROCESS_MAX_COUNT {
            break;
        }
    }
    Ok(processes)
}

fn collect_chains(
    current: &str,
    nodes: &BTreeMap<String, SynthNode>,
    adjacency: &BTreeMap<String, BTreeSet<String>>,
    path: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<SynthProcess>,
) {
    if out.len() >= PROCESS_MAX_COUNT {
        return;
    }
    let nexts = adjacency.get(current);
    if path.len() >= PROCESS_MAX_STEPS || nexts.is_none_or(BTreeSet::is_empty) {
        push_process(path, nodes, seen, out);
        return;
    }
    let mut advanced = false;
    let mut ranked: Vec<&String> = nexts.unwrap().iter().collect();
    ranked.sort_by(|a, b| {
        let na = &nodes[*a];
        let nb = &nodes[*b];
        step_score(nb)
            .cmp(&step_score(na))
            .then_with(|| na.file_path.cmp(&nb.file_path))
            .then_with(|| na.name.cmp(&nb.name))
            .then_with(|| a.cmp(b))
    });
    for next in ranked.into_iter().take(PROCESS_BRANCH_LIMIT) {
        if path.iter().any(|id| id == next) {
            continue;
        }
        path.push(next.clone());
        collect_chains(next, nodes, adjacency, path, seen, out);
        path.pop();
        advanced = true;
        if out.len() >= PROCESS_MAX_COUNT {
            return;
        }
    }
    if !advanced {
        push_process(path, nodes, seen, out);
    }
}

fn push_process(
    path: &[String],
    nodes: &BTreeMap<String, SynthNode>,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<SynthProcess>,
) {
    if path.len() < 2 || out.len() >= PROCESS_MAX_COUNT {
        return;
    }
    let key = path.join(">");
    if !seen.insert(key.clone()) {
        return;
    }
    let steps: Vec<SynthNode> = path
        .iter()
        .filter_map(|id| nodes.get(id).cloned())
        .collect();
    if steps.len() < 2 {
        return;
    }
    let entry = steps.first().expect("steps").display_name();
    let terminal = steps.last().expect("steps").display_name();
    let id = format!("process:call-chain:{:016x}", stable_hash(&key));
    out.push(SynthProcess {
        id,
        name: format!("{entry} → {terminal}"),
        steps,
    });
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

fn entry_score(node: &SynthNode, indegree: usize) -> i32 {
    let name = node.name.to_ascii_lowercase();
    let mut score = if indegree == 0 { 50 } else { 0 };
    if is_hard_entry_name(&name) {
        score += 100;
    }
    if name.starts_with("start") || name.starts_with("run") || name.starts_with("handle") {
        score += 60;
    }
    if name.contains("request") || name.contains("route") || name.contains("handler") {
        score += 30;
    }
    score + step_score(node)
}

fn is_hard_entry_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "main" | "start" | "run" | "init" | "bootstrap"
    )
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
    }
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

        let processes = synthesize_processes(&conn, "demo").unwrap();
        assert_eq!(processes.len(), 1);
        let p = &processes[0];
        assert_eq!(p.name, "main → parseConfig");
        assert_eq!(
            p.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["main", "handleRequest", "parseConfig"]
        );

        let node = p.node_rec();
        assert_eq!(node.label, "Process");
        assert_eq!(node.properties["processType"], "call-chain");
        assert_eq!(node.properties["stepCount"], 3);
        assert_eq!(node.properties["entryPointId"], p.steps[0].aka_id);
        assert_eq!(node.properties["terminalId"], p.steps[2].aka_id);

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

        let processes = synthesize_processes(&conn, "demo").unwrap();
        assert!(processes.is_empty());
    }

    #[test]
    fn synthesizes_when_native_process_lacks_step_edges() {
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

        let processes = synthesize_processes(&conn, "demo").unwrap();
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].name, "main → next");
    }
}
