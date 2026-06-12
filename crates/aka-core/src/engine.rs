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
    let synth = synthesize_graph(&conn, project)?;
    let mut stats = ArtifactStats {
        files: count_files(&conn, project)?,
        nodes: export_nodes(&conn, project, &out_dir.join("nodes.ndjson"), &synth)?,
        edges: export_edges(&conn, project, &out_dir.join("edges.ndjson"), &synth)?,
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
    synth: &SynthGraph,
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
    Ok(count)
}

fn export_edges(
    conn: &Connection,
    project: &str,
    path: &Path,
    synth: &SynthGraph,
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
    for edge in synth
        .communities
        .iter()
        .flat_map(SynthCommunity::edge_recs)
        .chain(synth.processes.iter().flat_map(SynthProcess::edge_recs))
    {
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
    language: String,
    is_exported: bool,
    ast_framework_multiplier: f64,
    ast_framework_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct SynthGraph {
    communities: Vec<SynthCommunity>,
    processes: Vec<SynthProcess>,
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

fn synthesize_graph(conn: &Connection, project: &str) -> Result<SynthGraph, EngineError> {
    let native_communities = has_native_label(conn, project, "Community")?;
    let native_processes = has_native_label(conn, project, "Process")?;
    let nodes = load_synth_nodes(conn, project)?;
    if nodes.is_empty() {
        return Ok(SynthGraph::default());
    }
    let calls = load_call_graph(conn, project, &nodes)?;

    let communities = if native_communities {
        Vec::new()
    } else {
        synthesize_communities(&nodes, &calls.edges)
    };
    let community_memberships = if native_communities {
        load_native_community_memberships(conn, project, &nodes)?
    } else {
        community_memberships_from_synth(&communities)
    };
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

    Ok(SynthGraph {
        communities,
        processes,
    })
}

fn has_native_label(conn: &Connection, project: &str, label: &str) -> Result<bool, EngineError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM nodes WHERE project = ?1 AND label = ?2",
        [project, label],
        |row| row.get(0),
    )?;
    Ok(count > 0)
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
         WHERE e.project = ?1 AND UPPER(e.type) = 'CALLS' \
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
         WHERE e.project = ?1 AND c.label = 'Community' AND UPPER(e.type) = 'MEMBER_OF' \
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

        let processes = synthesize_graph(&conn, "demo").unwrap().processes;
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

        let synth = synthesize_graph(&conn, "demo").unwrap();
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

        let synth = synthesize_graph(&conn, "demo").unwrap();
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

        let processes = synthesize_graph(&conn, "demo").unwrap().processes;
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

        let processes = synthesize_graph(&conn, "demo").unwrap().processes;
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

        let processes = synthesize_graph(&conn, "demo").unwrap().processes;
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

        let synth = synthesize_graph(&conn, "demo").unwrap();
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

        let synth = synthesize_graph(&conn, "demo").unwrap();
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
}
