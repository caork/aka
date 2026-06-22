//! 真实 Backend：aka_mcp::Backend over 注册表 + 图存储 + 搜索索引。
//!
//! 每个仓库的句柄（图库 + 邻接 + 搜索）首次使用时打开并缓存。
//! `GraphStore`（rusqlite）非 Sync，包 Mutex；Adjacency / SearchIndex 只读共享。
//!
//! 仓库管理（导入 / 更新 / 删除）：
//! - 导入 / 更新是 202 语义——调用立即返回，clone / 解压 / analyze 在
//!   `std::thread` 后台执行，进度落在 [`JobInfo`]（name → indexing/failed），
//!   `list_repos` 合并该状态（无任务记录 = ready，未注册的进行中导入合成条目）。
//! - git / zip 来源 checkout 到 `~/.aka/checkouts/<name>`；删除仓库只清受管
//!   checkout，用户自己的本地路径绝不动。
//! - zip 解压防 zip-slip：entry 路径必须能 `enclosed_name()`（拒绝绝对路径与 `..`）。

use std::collections::{BTreeMap, HashMap, HashSet};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};

use crate::rename;
use aka_core::{
    aka_home, clamp_render_nodes, load_index_state, repo_dir_name, user_facing_path, ArtifactStats,
    EngineEvent, IndexState, Registry, RepoEntry, RepoPaths, DEFAULT_RENDER_MAX_NODES,
};
use aka_graph::{Adjacency, GraphStore, NodeRow};
use aka_mcp::{
    backend::dedup_symbol_refs, Backend, ChangeDetection, ChangedRange, ChangedSymbol,
    CodeLineMatch, CodeSearchHit, CodeSearchResult, DirectoryCount, GraphqlMapEntry,
    ImpactDirection, ProcessHit, QueryEnrichment, RenamePlan, RepoInfo, RepoProgress,
    RepoSettingsUpdate, RouteConsumer, RouteMapEntry, SearchHit, SymbolRef, SymbolSelector,
    ToolMapEntry, TopicEndpoint, TopicMapEntry,
};
use aka_search::SearchIndex;
use git2::{
    build::{CheckoutBuilder, RepoBuilder},
    BranchType, Cred, CredentialType, FetchOptions, RemoteCallbacks, Repository,
};

/// `/api/source` 单次最多返回的行数。
const MAX_SOURCE_LINES: usize = 2000;

const DEFINITION_LABELS: &[&str] = &[
    "Function",
    "Method",
    "Class",
    "Interface",
    "Struct",
    "Enum",
    "Trait",
    "Route",
    "GraphQL",
    "Tool",
    "Command",
    "Config",
    "Job",
    "Table",
    "Repository",
    "Migration",
    "Cache",
    "Event",
    "Policy",
    "Resource",
    "Transaction",
];

const JOB_LOG_LIMIT: usize = 240;
const JOB_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(8);
const AUTO_INDEX_SCAN_INTERVAL: Duration = Duration::from_secs(4);
const AUTO_INDEX_DEBOUNCE: Duration = Duration::from_secs(3);
const RENAME_REFERENCE_LIMIT: usize = 500;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

type JobEventSink = Arc<dyn Fn(String) + Send + Sync + 'static>;

#[cfg(windows)]
fn hide_child_console(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_child_console(_cmd: &mut Command) {}

pub struct RepoHandle {
    pub entry: RepoEntry,
    pub store: Mutex<GraphStore>,
    pub adj: Adjacency,
    pub search: SearchIndex,
}

impl RepoHandle {
    fn open(entry: RepoEntry) -> Result<Self> {
        let paths = RepoPaths {
            root: entry.data_dir.clone(),
        };
        let store = GraphStore::open(&paths.graph_db())
            .with_context(|| format!("图库未就绪（先 aka analyze）: {}", entry.name))?;
        let adj = Adjacency::build(&store)?;
        let search = SearchIndex::open(&paths.search_dir())
            .with_context(|| format!("搜索索引未就绪（先 aka analyze）: {}", entry.name))?;
        Ok(Self {
            entry,
            store: Mutex::new(store),
            adj,
            search,
        })
    }

    fn node_row(&self, node: u32) -> Result<Option<NodeRow>> {
        let id = self.adj.id_of(node).to_string();
        let store = self.store.lock().expect("store lock");
        store.node_by_id(&id).map_err(Into::into)
    }

    /// 符号名 → 邻接下标。精确名优先，定义类 label 优先。
    fn resolve_symbol(&self, symbol: &str) -> Result<Option<u32>> {
        let rows = {
            let store = self.store.lock().expect("store lock");
            store.nodes_by_name(symbol, 20)?
        };
        let pick = rows
            .iter()
            .filter(|r| r.name.as_deref() == Some(symbol))
            .find(|r| DEFINITION_LABELS.contains(&r.label.as_str()))
            .or_else(|| rows.iter().find(|r| r.name.as_deref() == Some(symbol)))
            .or_else(|| rows.first());
        Ok(pick.and_then(|r| self.adj.index_of_rowid(r.rowid)))
    }

    fn rows_by_selector(&self, selector: &SymbolSelector, limit: usize) -> Result<Vec<NodeRow>> {
        if let Some(uid) = selector.uid.as_deref().filter(|v| !v.is_empty()) {
            let store = self.store.lock().expect("store lock");
            let Some(row) = store.node_by_id(uid)? else {
                return Ok(Vec::new());
            };
            return Ok(row_matches_selector(&row, selector)
                .then_some(row)
                .into_iter()
                .collect());
        }
        let Some(symbol) = selector.symbol.as_deref().filter(|v| !v.is_empty()) else {
            return Ok(Vec::new());
        };
        let rows = {
            let store = self.store.lock().expect("store lock");
            store.nodes_by_name(symbol, limit)?
        };
        Ok(rows
            .into_iter()
            .filter(|row| row_matches_selector(row, selector))
            .collect())
    }

    fn definition_rows_by_selector(&self, selector: &SymbolSelector) -> Result<Vec<NodeRow>> {
        let rows = self.rows_by_selector(selector, 50)?;
        let exact_defs: Vec<NodeRow> = rows
            .iter()
            .filter(|r| {
                r.name.as_deref().is_some_and(|name| {
                    selector
                        .symbol
                        .as_deref()
                        .is_none_or(|symbol| symbol == name)
                }) && DEFINITION_LABELS.contains(&r.label.as_str())
            })
            .cloned()
            .collect();
        if exact_defs.is_empty() {
            Ok(rows
                .into_iter()
                .filter(|r| {
                    r.name.as_deref().is_some_and(|name| {
                        selector
                            .symbol
                            .as_deref()
                            .is_none_or(|symbol| symbol == name)
                    })
                })
                .collect())
        } else {
            Ok(exact_defs)
        }
    }

    fn resolve_selector(&self, selector: &SymbolSelector) -> Result<Vec<u32>> {
        Ok(self
            .definition_rows_by_selector(selector)?
            .into_iter()
            .filter_map(|r| self.adj.index_of_rowid(r.rowid))
            .collect())
    }
}

fn row_matches_selector(row: &NodeRow, selector: &SymbolSelector) -> bool {
    selector.matches_hit(&row_to_hit(row, 1.0))
}

fn row_to_hit(row: &NodeRow, score: f32) -> SearchHit {
    SearchHit {
        node_id: row.id.clone(),
        name: row.name.clone().unwrap_or_default(),
        label: row.label.clone(),
        kind: None,
        file_path: row.file_path.clone().unwrap_or_default(),
        start_line: row.start_line.unwrap_or(0),
        score,
        snippet: None,
    }
}

fn string_array_prop(props: &serde_json::Value, key: &str) -> Vec<String> {
    props
        .get(key)
        .and_then(|v| v.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim_matches(['"', '\'']).to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn string_prop(props: &serde_json::Value, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .filter(|s| !s.is_empty())
}

fn parse_fetch_reason(reason: &str) -> (Vec<String>, Option<u32>) {
    let mut accessed = Vec::new();
    let mut fetch_count = None;
    for part in reason.split('|') {
        if let Some(keys) = part.strip_prefix("keys:") {
            accessed = keys
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
        } else if let Some(n) = part.strip_prefix("fetches:") {
            fetch_count = n.parse::<u32>().ok();
        }
    }
    (accessed, fetch_count)
}

fn topic_endpoints_for(
    store: &GraphStore,
    topic_rowid: i64,
    edge_types: &[&str],
) -> Result<Vec<TopicEndpoint>> {
    let mut endpoints = Vec::new();
    for linked in store.incoming_linked_nodes(topic_rowid, edge_types)? {
        let mut flows = store
            .processes_of_node(linked.node.rowid)?
            .into_iter()
            .map(|process| process.name)
            .collect::<Vec<_>>();
        flows.sort();
        flows.dedup();
        endpoints.push(TopicEndpoint {
            name: linked
                .node
                .name
                .clone()
                .unwrap_or_else(|| linked.node.id.clone()),
            file_path: linked.node.file_path.unwrap_or_default(),
            flows,
        });
    }
    endpoints.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then_with(|| a.name.cmp(&b.name))
    });
    endpoints.dedup_by(|a, b| a.name == b.name && a.file_path == b.file_path);
    Ok(endpoints)
}

fn node_content(repo_root: &Path, row: &NodeRow) -> Result<Option<String>> {
    let Some(file) = row.file_path.as_deref() else {
        return Ok(None);
    };
    let Some(start) = row.start_line else {
        return Ok(None);
    };
    let end = row.end_line.or(row.start_line);
    let source = read_source_slice(repo_root, file, Some(start), end)?;
    let lines = source["lines"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    Ok((!lines.is_empty()).then_some(lines))
}

/// 后台任务状态（import / update）。不在 map 里 = ready。
#[derive(Debug, Clone)]
struct JobInfo {
    /// `indexing` / `ready` / `failed`。
    status: String,
    /// 失败原因（status = failed 时携带）。
    detail: Option<String>,
    /// 来源种类（合成 list 条目用）：local / git / zip。
    kind: String,
    /// git 来源 URL。
    url: Option<String>,
    /// 任务针对的仓库路径（合成 list 条目展示用）。
    path: PathBuf,
    /// 当前进度和日志尾部。
    progress: RepoProgress,
    /// 后台任务开始时间，用于日志定位卡点。
    started_at: Instant,
    /// 最后一次进度事件时间。
    last_event_at: Instant,
}

impl JobInfo {
    fn new(kind: &str, url: Option<String>, path: PathBuf) -> Self {
        let now = Instant::now();
        Self {
            status: "indexing".into(),
            detail: None,
            kind: kind.to_string(),
            url,
            path,
            progress: RepoProgress {
                stage: "queued".into(),
                message: "Queued for indexing".into(),
                percent: 1.0,
                current: None,
                total: None,
                files: 0,
                nodes: 0,
                edges: 0,
                chunks: 0,
                logs: vec!["t+0.0s queued: Queued for indexing".into()],
            },
            started_at: now,
            last_event_at: now,
        }
    }

    fn push_log(&mut self, line: impl Into<String>) {
        let body = line.into();
        self.last_event_at = Instant::now();
        if self
            .progress
            .logs
            .last()
            .and_then(|line| line.split_once(' ').map(|(_, body)| body))
            == Some(body.as_str())
        {
            return;
        }
        let line = format!("{} {body}", self.elapsed_label());
        self.progress.logs.push(line);
        if self.progress.logs.len() > JOB_LOG_LIMIT {
            let drop_count = self.progress.logs.len() - JOB_LOG_LIMIT;
            self.progress.logs.drain(0..drop_count);
        }
    }

    fn elapsed_label(&self) -> String {
        format!("t+{:.1}s", self.started_at.elapsed().as_secs_f32())
    }

    fn set_stage(&mut self, stage: &str, message: impl Into<String>, percent: f32) {
        let message = message.into();
        self.progress.stage = stage.to_string();
        self.progress.message = message.clone();
        self.progress.percent = self.progress.percent.max(percent).min(99.0);
        self.progress.current = None;
        self.progress.total = None;
        self.push_log(format!("{stage}: {message}"));
    }

    fn mark_done(&mut self) {
        self.status = "ready".into();
        self.detail = None;
        self.progress.stage = "done".into();
        self.progress.message = "Index ready".into();
        self.progress.percent = 100.0;
        self.progress.current = None;
        self.progress.total = None;
        self.push_log("done: Index ready");
    }

    fn maybe_heartbeat(&mut self) {
        if self.status != "indexing" || self.last_event_at.elapsed() < JOB_HEARTBEAT_INTERVAL {
            return;
        }
        let quiet_for = self.last_event_at.elapsed().as_secs();
        let stage = self.progress.stage.clone();
        let message = self.progress.message.clone();
        let counts = match (self.progress.current, self.progress.total) {
            (Some(current), Some(total)) if total > 0 => format!(" ({current}/{total})"),
            (Some(current), _) if current > 0 => format!(" ({current})"),
            _ => String::new(),
        };
        self.push_log(format!(
            "still working after {quiet_for}s without new events: {stage}: {message}{counts}"
        ));
    }

    fn apply_engine_event(&mut self, ev: &EngineEvent) {
        match ev {
            EngineEvent::Phase {
                phase,
                current,
                total,
            } => {
                let phase_lc = phase.to_ascii_lowercase();
                if phase_lc.contains("building graph") || phase_lc.contains("search index") {
                    self.progress.stage = "index".into();
                    self.progress.message = phase.clone();
                    self.progress.current = None;
                    self.progress.total = None;
                    self.progress.percent = self.progress.percent.clamp(86.0, 96.0);
                    self.push_log(format!("index: {phase}"));
                    return;
                }
                if phase_lc.contains("registering") {
                    self.progress.stage = "register".into();
                    self.progress.message = phase.clone();
                    self.progress.current = None;
                    self.progress.total = None;
                    self.progress.percent = self.progress.percent.clamp(96.0, 99.0);
                    self.push_log(format!("register: {phase}"));
                    return;
                }
                if let Some(index_phase) = phase.strip_prefix("index:") {
                    self.progress.stage = index_phase.to_string();
                    self.progress.message = phase.clone();
                    self.progress.current = (*total > 0 || *current > 0).then_some(*current);
                    self.progress.total = (*total > 0).then_some(*total);
                    self.progress.percent = index_percent(index_phase, *current, *total)
                        .max(self.progress.percent)
                        .min(96.0);
                    if *total > 0 {
                        self.push_log(format!("index: {index_phase} ({current}/{total})"));
                    } else {
                        self.push_log(format!("index: {index_phase}"));
                    }
                    return;
                }
                if phase_lc.contains("export-artifacts") {
                    self.progress.stage = "adapter".into();
                    self.progress.message = phase.clone();
                    self.progress.current = (*total > 0 || *current > 0).then_some(*current);
                    self.progress.total = (*total > 0).then_some(*total);
                    self.progress.percent = adapter_percent(phase, *current, *total)
                        .max(self.progress.percent)
                        .min(92.0);
                    if *total > 0 {
                        self.push_log(format!("adapter: {phase} ({current}/{total})"));
                    } else {
                        self.push_log(format!("adapter: {phase}"));
                    }
                    return;
                }
                self.progress.stage = "engine".into();
                self.progress.message = phase.clone();
                self.progress.current = (*total > 0 || *current > 0).then_some(*current);
                self.progress.total = (*total > 0).then_some(*total);
                self.progress.percent = engine_percent(phase, *current, *total)
                    .max(self.progress.percent)
                    .min(76.0);
                if *total > 0 {
                    self.push_log(format!("engine: {phase} ({current}/{total})"));
                } else {
                    self.push_log(format!("engine: {phase}"));
                }
            }
            EngineEvent::Warning { message } => {
                self.push_log(format!("warning: {message}"));
            }
            EngineEvent::Log { stream, line } => {
                self.push_log(format!("[{stream}] {line}"));
            }
            EngineEvent::Done { stats } => {
                self.progress.stage = "engine".into();
                self.progress.message = "Engine emit complete".into();
                self.progress.percent = 78.0;
                self.apply_stats(stats);
                self.push_log(format!(
                    "engine: done ({} files, {} nodes, {} edges, {} chunks)",
                    stats.files, stats.nodes, stats.edges, stats.chunks
                ));
            }
        }
    }

    fn apply_stats(&mut self, stats: &ArtifactStats) {
        self.progress.files = stats.files;
        self.progress.nodes = stats.nodes;
        self.progress.edges = stats.edges;
        self.progress.chunks = stats.chunks;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RepoQuickState {
    files: BTreeMap<String, QuickFingerprint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuickFingerprint {
    size: u64,
    modified: u128,
}

#[derive(Debug, Clone)]
struct AutoIndexState {
    quick: RepoQuickState,
    first_seen_dirty: Option<Instant>,
    last_dirty_quick: Option<RepoQuickState>,
}

fn adapter_percent(phase: &str, current: u64, total: u64) -> f32 {
    let phase = phase.to_ascii_lowercase();
    if phase.contains("dependency-edges") && total > 0 {
        let ratio = (current as f32 / total as f32).clamp(0.0, 1.0);
        return 80.0 + ratio * 6.0;
    }
    if phase.contains("nodes") && total > 0 {
        let ratio = (current as f32 / total as f32).clamp(0.0, 1.0);
        return 89.0 + ratio;
    }
    if phase.contains("edges") && total > 0 {
        let ratio = (current as f32 / total as f32).clamp(0.0, 1.0);
        return 90.0 + ratio;
    }
    if phase.contains("chunks") && total > 0 {
        let ratio = (current as f32 / total as f32).clamp(0.0, 1.0);
        return 91.0 + ratio;
    }
    if phase.contains("inspect-db") {
        77.0
    } else if phase.contains("synthesize:native-labels") {
        78.0
    } else if phase.contains("synthesize:nodes") {
        79.0
    } else if phase.contains("synthesize:dependency-edges") {
        80.0
    } else if phase.contains("synthesize:call-graph") {
        82.0
    } else if phase.contains("synthesize:project-subgraph") {
        83.0
    } else if phase.contains("synthesize:communities") {
        84.0
    } else if phase.contains("synthesize:processes") {
        87.0
    } else if phase.contains("synthesize:routes:done") {
        88.0
    } else if phase.contains("synthesize:routes:consumers") {
        progress_between(current, total, 87.7, 88.0)
    } else if phase.contains("synthesize:routes:source-files") {
        progress_between(current, total, 87.5, 87.7)
    } else if phase.contains("synthesize:routes") {
        87.5
    } else if phase.contains("synthesize:topics") {
        88.0
    } else if phase.contains("nodes") {
        89.0
    } else if phase.contains("edges") {
        90.0
    } else if phase.contains("chunks") {
        91.0
    } else if phase.contains("manifest") {
        92.0
    } else {
        81.0
    }
}

fn progress_between(current: u64, total: u64, start: f32, end: f32) -> f32 {
    if total == 0 {
        return start;
    }
    let ratio = (current as f32 / total as f32).clamp(0.0, 1.0);
    start + ratio * (end - start)
}

fn engine_percent(phase: &str, current: u64, total: u64) -> f32 {
    if total > 0 {
        let ratio = (current as f32 / total as f32).clamp(0.0, 1.0);
        return 18.0 + ratio * 54.0;
    }
    let phase = phase.to_ascii_lowercase();
    if phase.contains("discover") || phase.contains("scan") {
        24.0
    } else if phase.contains("parse") || phase.contains("ast") {
        44.0
    } else if phase.contains("edge") || phase.contains("relationship") {
        60.0
    } else if phase.contains("chunk") {
        70.0
    } else {
        34.0
    }
}

fn index_percent(phase: &str, current: u64, total: u64) -> f32 {
    if total > 0 {
        let ratio = (current as f32 / total as f32).clamp(0.0, 1.0);
        if phase.contains("nodes") {
            return 87.0 + ratio * 2.0;
        }
        if phase.contains("edges") {
            return 89.0 + ratio * 2.0;
        }
        if phase.contains("chunks") {
            return 93.0 + ratio * 2.0;
        }
    }
    if phase.contains("preflight") || phase.contains("slice") {
        86.5
    } else if phase.contains("graph") {
        88.0
    } else if phase.contains("layout") {
        91.5
    } else if phase.contains("search") {
        94.0
    } else if phase.contains("commit") || phase.contains("done") {
        95.5
    } else {
        90.0
    }
}

fn update_job(
    jobs: &Arc<Mutex<HashMap<String, JobInfo>>>,
    name: &str,
    update: impl FnOnce(&mut JobInfo),
) {
    if let Some(job) = jobs.lock().expect("jobs lock").get_mut(name) {
        update(job);
    }
}

fn mark_job_done(
    jobs: &Arc<Mutex<HashMap<String, JobInfo>>>,
    completed: &Arc<Mutex<HashMap<String, RepoProgress>>>,
    name: &str,
) {
    if let Some(progress) = jobs.lock().expect("jobs lock").get_mut(name).map(|job| {
        job.mark_done();
        job.progress.clone()
    }) {
        completed
            .lock()
            .expect("completed jobs lock")
            .insert(name.to_string(), progress);
    }
}

fn run_analyze_job(
    jobs: &Arc<Mutex<HashMap<String, JobInfo>>>,
    name: &str,
    repo: PathBuf,
    engine_dir: Option<PathBuf>,
) -> Result<()> {
    update_job(jobs, name, |job| {
        let engine = engine_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "auto-discover".into());
        job.push_log(format!(
            "runtime: analyze repo={} engine_dir={engine}",
            repo.display()
        ));
        job.set_stage("engine", "Starting AST parser", 14.0);
    });
    let mut on_progress = |ev: &EngineEvent| {
        update_job(jobs, name, |job| {
            job.apply_engine_event(ev);
        });
    };
    crate::run_analyze_with_progress(repo, engine_dir, false, Some(&mut on_progress))
        .map_err(|e| anyhow!("{e:#}"))?;
    update_job(jobs, name, |job| {
        job.set_stage("index", "Index artifacts ready", 96.0);
        job.push_log("runtime: analyze pipeline returned successfully");
    });
    Ok(())
}

fn run_auto_analyze_job(
    jobs: &Arc<Mutex<HashMap<String, JobInfo>>>,
    name: &str,
    repo: PathBuf,
    engine_dir: Option<PathBuf>,
) -> Result<()> {
    update_job(jobs, name, |job| {
        job.push_log("auto-index: workspace change detected");
    });
    run_analyze_job(jobs, name, repo, engine_dir)
}

fn job_matches_key(name: &str, job: &JobInfo, key: &str) -> bool {
    if name == key || job.path.to_string_lossy() == key {
        return true;
    }
    PathBuf::from(key)
        .canonicalize()
        .is_ok_and(|path| user_facing_path(&path) == job.path)
}

fn looks_like_local_path(key: &str) -> bool {
    let path = Path::new(key);
    path.is_absolute()
        || key.starts_with("./")
        || key.starts_with("../")
        || key == "."
        || key == ".."
        || key.contains('/')
        || key.contains('\\')
}

fn run_auto_indexer(
    auto: Arc<AutoIndexer>,
    jobs: Arc<Mutex<HashMap<String, JobInfo>>>,
    completed: Arc<Mutex<HashMap<String, RepoProgress>>>,
    handles: Arc<Mutex<HashMap<PathBuf, Arc<RepoHandle>>>>,
    engine_dir: Option<PathBuf>,
    job_event_sink: Option<JobEventSink>,
) {
    while !auto.stop.load(Ordering::Relaxed) {
        if let Ok(registry) = Registry::load() {
            auto_index_scan(
                &auto,
                &jobs,
                &completed,
                &handles,
                engine_dir.clone(),
                job_event_sink.clone(),
                registry,
            );
        }
        std::thread::sleep(AUTO_INDEX_SCAN_INTERVAL);
    }
    auto.running.store(false, Ordering::Relaxed);
}

fn auto_index_scan(
    auto: &Arc<AutoIndexer>,
    jobs: &Arc<Mutex<HashMap<String, JobInfo>>>,
    completed: &Arc<Mutex<HashMap<String, RepoProgress>>>,
    handles: &Arc<Mutex<HashMap<PathBuf, Arc<RepoHandle>>>>,
    engine_dir: Option<PathBuf>,
    job_event_sink: Option<JobEventSink>,
    registry: Registry,
) {
    if jobs
        .lock()
        .expect("jobs lock")
        .values()
        .any(|job| job.status == "indexing")
    {
        return;
    }

    let registered_names: HashSet<String> = registry.repos.iter().map(|r| r.name.clone()).collect();
    auto.states
        .lock()
        .expect("auto index states lock")
        .retain(|name, _| registered_names.contains(name));

    for entry in registry.repos {
        if entry.source_kind == "zip" {
            continue;
        }
        if jobs
            .lock()
            .expect("jobs lock")
            .get(&entry.name)
            .is_some_and(|job| job.status == "indexing")
        {
            continue;
        }
        let Ok(current_quick) = RepoQuickState::compute(&entry.repo_path) else {
            continue;
        };
        let should_analyze = {
            let mut states = auto.states.lock().expect("auto index states lock");
            if let Some(state) = states.get_mut(&entry.name) {
                auto_index_should_analyze(state, current_quick, Instant::now())
            } else {
                let is_dirty = auto_index_has_delta(&entry);
                states.insert(
                    entry.name.clone(),
                    auto_index_initial_state(current_quick, is_dirty, Instant::now()),
                );
                false
            }
        };
        if should_analyze && auto_index_has_delta(&entry) {
            spawn_auto_index_job(
                entry,
                Arc::clone(jobs),
                Arc::clone(completed),
                Arc::clone(handles),
                engine_dir.clone(),
                job_event_sink.clone(),
            );
            return;
        }
    }
}

fn auto_index_initial_state(
    current: RepoQuickState,
    is_dirty: bool,
    now: Instant,
) -> AutoIndexState {
    AutoIndexState {
        quick: current.clone(),
        first_seen_dirty: is_dirty.then_some(now),
        last_dirty_quick: is_dirty.then_some(current),
    }
}

fn auto_index_should_analyze(
    state: &mut AutoIndexState,
    current: RepoQuickState,
    now: Instant,
) -> bool {
    if current == state.quick && state.last_dirty_quick.is_none() {
        state.first_seen_dirty = None;
        state.last_dirty_quick = None;
        return false;
    }
    if state.last_dirty_quick.as_ref() != Some(&current) {
        state.first_seen_dirty = Some(now);
        state.last_dirty_quick = Some(current);
        return false;
    }
    if state
        .first_seen_dirty
        .is_some_and(|first| now.duration_since(first) >= AUTO_INDEX_DEBOUNCE)
    {
        state.quick = current;
        state.first_seen_dirty = None;
        state.last_dirty_quick = None;
        return true;
    }
    false
}

fn auto_index_has_delta(entry: &RepoEntry) -> bool {
    let paths = RepoPaths {
        root: entry.data_dir.clone(),
    };
    let previous = match load_index_state(&paths.index_state_path()) {
        Ok(previous) => previous,
        Err(_) => return true,
    };
    let current = match IndexState::compute(&entry.repo_path, entry.engine_sha.clone(), false) {
        Ok(current) => current,
        Err(_) => return false,
    };
    !current.delta_from(previous.as_ref()).is_empty()
}

fn spawn_auto_index_job(
    entry: RepoEntry,
    jobs: Arc<Mutex<HashMap<String, JobInfo>>>,
    completed: Arc<Mutex<HashMap<String, RepoProgress>>>,
    handles: Arc<Mutex<HashMap<PathBuf, Arc<RepoHandle>>>>,
    engine_dir: Option<PathBuf>,
    job_event_sink: Option<JobEventSink>,
) {
    {
        let mut jobs_guard = jobs.lock().expect("jobs lock");
        if jobs_guard
            .get(&entry.name)
            .is_some_and(|job| job.status == "indexing")
        {
            return;
        }
        let mut job = JobInfo::new(
            &entry.source_kind,
            entry.source_url.clone(),
            entry.repo_path.clone(),
        );
        job.set_stage("queued", "Workspace changed; refreshing index", 2.0);
        jobs_guard.insert(entry.name.clone(), job);
    }
    let name = entry.name.clone();
    let repo_path = entry.repo_path.clone();
    emit_job_event(
        &job_event_sink,
        format!("auto-index queued name={name} path={}", repo_path.display()),
    );
    std::thread::spawn(move || {
        emit_job_event(&job_event_sink, format!("auto-index started name={name}"));
        match run_auto_analyze_job(&jobs, &name, repo_path.clone(), engine_dir) {
            Ok(()) => {
                handles.lock().expect("handles lock").remove(&repo_path);
                mark_job_done(&jobs, &completed, &name);
                jobs.lock().expect("jobs lock").remove(&name);
                emit_job_event(&job_event_sink, format!("auto-index completed name={name}"));
            }
            Err(e) => {
                let detail = format!("{e:#}");
                if let Some(job) = jobs.lock().expect("jobs lock").get_mut(&name) {
                    job.status = "failed".into();
                    job.detail = Some(detail.clone());
                    job.progress.stage = "failed".into();
                    job.progress.message = "Auto index failed".into();
                    job.push_log(format!("failed: {detail}"));
                }
                emit_job_event(
                    &job_event_sink,
                    format!("auto-index failed name={name}: {detail}"),
                );
            }
        }
    });
}

fn emit_job_event(sink: &Option<JobEventSink>, message: impl Into<String>) {
    if let Some(sink) = sink {
        sink(message.into());
    }
}

impl RepoQuickState {
    fn compute(repo: &Path) -> std::io::Result<Self> {
        let mut files = BTreeMap::new();
        collect_quick_state(repo, repo, &mut files)?;
        Ok(Self { files })
    }
}

fn collect_quick_state(
    repo: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, QuickFingerprint>,
) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if file_type.is_dir() {
            if is_auto_index_skipped_dir(&name) {
                continue;
            }
            collect_quick_state(repo, &path, out)?;
        } else if file_type.is_file() {
            if is_auto_index_skipped_file(&name) {
                continue;
            }
            let Some(rel) = path.strip_prefix(repo).ok() else {
                continue;
            };
            let meta = entry.metadata()?;
            out.insert(
                rel.to_string_lossy().replace('\\', "/"),
                QuickFingerprint {
                    size: meta.len(),
                    modified: meta
                        .modified()
                        .ok()
                        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos())
                        .unwrap_or(0),
                },
            );
        }
    }
    Ok(())
}

fn is_auto_index_skipped_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | ".aka"
            | ".claude"
            | ".cursor"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "coverage"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".next"
            | ".nuxt"
            | ".turbo"
    )
}

fn is_auto_index_skipped_file(name: &str) -> bool {
    matches!(name, ".DS_Store")
}

pub struct AkaBackend {
    handles: Arc<Mutex<HashMap<PathBuf, Arc<RepoHandle>>>>,
    jobs: Arc<Mutex<HashMap<String, JobInfo>>>,
    completed_jobs: Arc<Mutex<HashMap<String, RepoProgress>>>,
    auto_indexer: Arc<AutoIndexer>,
    engine_dir: Option<PathBuf>,
    auto_discover_workspace: bool,
    job_event_sink: Option<JobEventSink>,
}

struct AutoIndexer {
    stop: AtomicBool,
    running: AtomicBool,
    states: Mutex<HashMap<String, AutoIndexState>>,
}

impl AutoIndexer {
    fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            running: AtomicBool::new(false),
            states: Mutex::new(HashMap::new()),
        }
    }
}

impl Drop for AutoIndexer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for AkaBackend {
    fn drop(&mut self) {
        self.auto_indexer.stop.store(true, Ordering::Relaxed);
    }
}

/// git / zip 导入的受管 checkout 根目录。
fn checkouts_dir() -> PathBuf {
    aka_home().join("checkouts")
}

/// 仓库名用于拼 checkout 路径，必须是单段目录名。
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        bail!("invalid repo name: {name:?}");
    }
    Ok(())
}

/// 从 git URL 推仓库名：末段去 `.git`，非常规字符折叠成 `-`。
fn derive_git_name(url: &str) -> Result<String> {
    let last = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".git");
    let name: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if name.trim_matches(['-', '.']).is_empty() {
        bail!("invalid git url (cannot derive repo name): {url}");
    }
    Ok(name)
}

fn derive_local_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "repo".into())
}

fn unique_local_name(registry: &Registry, path: &Path) -> String {
    let base = derive_local_name(path);
    if registry.find_by_name(&base).is_none() {
        return base;
    }
    let with_hash = repo_dir_name(path);
    if registry.find_by_name(&with_hash).is_none() {
        return with_hash;
    }
    for i in 2.. {
        let candidate = format!("{with_hash}-{i}");
        if registry.find_by_name(&candidate).is_none() {
            return candidate;
        }
    }
    unreachable!("unbounded local repo name search")
}

fn discover_workspace_root(start: &Path) -> Result<Option<PathBuf>> {
    let start = start
        .canonicalize()
        .with_context(|| format!("resolve workspace path {}", start.display()))?;
    let mut cmd = Command::new("git");
    cmd.current_dir(&start)
        .arg("rev-parse")
        .arg("--show-toplevel");
    hide_child_console(&mut cmd);
    if let Ok(output) = cmd.output() {
        if output.status.success() {
            let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !root.is_empty() {
                return Ok(Some(PathBuf::from(root).canonicalize()?));
            }
        }
    }
    let mut cursor = start.as_path();
    loop {
        if cursor.is_dir()
            && has_workspace_marker(cursor)
            && RepoQuickState::compute(cursor).is_ok_and(|state| !state.files.is_empty())
        {
            return Ok(Some(cursor.to_path_buf()));
        }
        let Some(parent) = cursor.parent() else {
            break;
        };
        cursor = parent;
    }
    Ok(None)
}

fn has_workspace_marker(path: &Path) -> bool {
    const MARKERS: &[&str] = &[
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "requirements.txt",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
        "go.mod",
        "composer.json",
        "Gemfile",
    ];
    MARKERS.iter().any(|marker| path.join(marker).is_file())
}

fn git_callbacks<'a>() -> RemoteCallbacks<'a> {
    let mut callbacks = RemoteCallbacks::new();
    callbacks.credentials(|_, username_from_url, allowed| {
        if allowed.contains(CredentialType::SSH_KEY) {
            Cred::ssh_key_from_agent(username_from_url.unwrap_or("git"))
        } else {
            Cred::default()
        }
    });
    callbacks
}

fn git_fetch_options<'a>() -> FetchOptions<'a> {
    let mut fetch = FetchOptions::new();
    fetch.remote_callbacks(git_callbacks());
    fetch
}

fn clone_git_repo(url: &str, dest: &Path) -> Result<()> {
    let mut builder = RepoBuilder::new();
    builder.fetch_options(git_fetch_options());
    builder
        .clone(url, dest)
        .with_context(|| format!("git clone {url} -> {}", dest.display()))?;
    Ok(())
}

fn fast_forward_git_repo(dir: &Path) -> Result<()> {
    let repo = Repository::open(dir).with_context(|| format!("open git repo {}", dir.display()))?;
    let head = repo.head().context("read git HEAD")?;
    if !head.is_branch() {
        bail!("git repo is in detached HEAD state; checkout a branch before updating");
    }
    let head_name = head.name().context("read git HEAD ref name")?.to_string();
    let branch_name = head
        .shorthand()
        .context("read git branch name")?
        .to_string();
    drop(head);

    let branch = repo
        .find_branch(&branch_name, BranchType::Local)
        .with_context(|| format!("find local git branch {branch_name}"))?;
    let upstream = branch
        .upstream()
        .with_context(|| format!("git branch {branch_name} has no upstream"))?;
    let upstream_name = upstream
        .get()
        .name()
        .context("read upstream ref name")?
        .to_string();
    let remote_name = upstream_name
        .strip_prefix("refs/remotes/")
        .and_then(|s| s.split('/').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("origin")
        .to_string();
    drop(upstream);
    drop(branch);

    let mut remote = repo
        .find_remote(&remote_name)
        .with_context(|| format!("find git remote {remote_name}"))?;
    remote
        .fetch(&[] as &[&str], Some(&mut git_fetch_options()), None)
        .with_context(|| format!("fetch git remote {remote_name}"))?;
    drop(remote);

    let upstream_ref = repo
        .find_reference(&upstream_name)
        .with_context(|| format!("find upstream ref {upstream_name}"))?;
    let upstream_commit = repo.reference_to_annotated_commit(&upstream_ref)?;
    let (analysis, _) = repo.merge_analysis(&[&upstream_commit])?;
    if analysis.is_up_to_date() {
        return Ok(());
    }
    if !analysis.is_fast_forward() {
        bail!("git update is not fast-forward; resolve the branch manually and retry");
    }

    let mut reference = repo
        .find_reference(&head_name)
        .with_context(|| format!("find local ref {head_name}"))?;
    reference.set_target(upstream_commit.id(), "fast-forward")?;
    repo.set_head(&head_name)?;
    repo.checkout_head(Some(CheckoutBuilder::new().force()))?;
    Ok(())
}

/// 解压 zip 到 `dest`（防 zip-slip），顶层只有单个目录时拍平一层。
fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(zip_path)
        .with_context(|| format!("open uploaded zip {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("read zip archive")?;
    std::fs::create_dir_all(dest)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        // 防 zip-slip：enclosed_name 拒绝绝对路径与含 `..` 的 entry。
        let Some(rel) = entry.enclosed_name() else {
            bail!(
                "invalid zip entry path (absolute or contains ..): {}",
                entry.name()
            );
        };
        // macOS 压缩垃圾不落盘。
        if rel
            .components()
            .next()
            .is_some_and(|c| c.as_os_str() == "__MACOSX")
        {
            continue;
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f =
                std::fs::File::create(&out).with_context(|| format!("write {}", out.display()))?;
            std::io::copy(&mut entry, &mut f)?;
        }
    }
    flatten_single_top_dir(dest)
}

/// zip 顶层只有一个目录（常见的 GitHub 导出形态）时把内容上提一层。
fn flatten_single_top_dir(dest: &Path) -> Result<()> {
    let entries: Vec<_> = std::fs::read_dir(dest)?.collect::<std::io::Result<Vec<_>>>()?;
    let [only] = entries.as_slice() else {
        return Ok(());
    };
    if !only.file_type()?.is_dir() {
        return Ok(());
    }
    // 先挪到临时名再搬内容，防内层有同名子项。
    let staged = dest.join(".aka-flatten-tmp");
    std::fs::rename(only.path(), &staged)?;
    for child in std::fs::read_dir(&staged)? {
        let child = child?;
        std::fs::rename(child.path(), dest.join(child.file_name()))?;
    }
    std::fs::remove_dir(&staged)?;
    Ok(())
}

/// 读取 `repo_root` 内 `rel` 文件的 1-based 含端行切片（`/api/source` 合同）。
///
/// - 安全：拒绝绝对路径；canonicalize 后必须仍位于 repo_root 内
///   （`..` 穿越与软链接逃逸都挡）。错误文案带 "invalid" → HTTP 400。
/// - 二进制（含 `\0`）/ 非 UTF-8 文件 → "invalid file" → 400。
/// - 文件不存在 → "file not found" → 404。
/// - start/end 缺省 = 整个文件；越界自动 clamp；单次最多
///   [`MAX_SOURCE_LINES`] 行，超出置 `truncated = true`。
fn read_source_slice(
    repo_root: &Path,
    rel: &str,
    start: Option<u32>,
    end: Option<u32>,
) -> Result<serde_json::Value> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        bail!("invalid path (must be repo-relative): {rel}");
    }
    let root = repo_root
        .canonicalize()
        .with_context(|| format!("repo path not found: {}", repo_root.display()))?;
    let abs = match root.join(rel_path).canonicalize() {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("file not found in repo: {rel}")
        }
        Err(e) => return Err(anyhow!("resolve {rel}: {e}")),
    };
    // canonicalize 已解析软链接：仍须落在 repo 根目录内。
    if !abs.starts_with(&root) {
        bail!("invalid path (escapes repo root): {rel}");
    }
    if !abs.is_file() {
        bail!("file not found (not a regular file): {rel}");
    }
    let bytes =
        std::fs::read(&abs).with_context(|| format!("read source file {}", abs.display()))?;
    if bytes.contains(&0) {
        bail!("invalid file (binary content, contains NUL byte): {rel}");
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        bail!("invalid file (not valid UTF-8 text): {rel}");
    };

    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    // 1-based 含端切片；越界自动 clamp。空文件返回 start=1/end=0/lines=[]。
    let (s, mut e) = if total == 0 {
        (1usize, 0usize)
    } else {
        let s = (start.unwrap_or(1).max(1) as usize).min(total);
        let e = (end.map(|v| v as usize).unwrap_or(total)).clamp(s, total);
        (s, e)
    };
    let mut truncated = false;
    if e >= s && e - s + 1 > MAX_SOURCE_LINES {
        e = s + MAX_SOURCE_LINES - 1;
        truncated = true;
    }
    let lines: Vec<&str> = if total == 0 {
        Vec::new()
    } else {
        all[s - 1..e].to_vec()
    };

    Ok(serde_json::json!({
        "path": rel,
        "abs_path": abs.to_string_lossy(),
        "total_lines": total,
        "start": s,
        "end": e,
        "lines": lines,
        "truncated": truncated,
    }))
}

fn should_disambiguate_for_backend(selector: &SymbolSelector, defs: &[SearchHit]) -> bool {
    defs.len() > 1 && !selector.is_narrowed()
}

/// `/api/file/symbols` 合同：`rows`（已按 start_line 升序）滤掉无行号节点
/// （File / Folder / Community 等聚合节点，源码里没有可点位置），映射成
/// `{path, symbols: [{id, name, label, file, line, end_line}]}`。
fn file_symbols_json(path: &str, rows: &[NodeRow]) -> serde_json::Value {
    let symbols: Vec<serde_json::Value> = rows
        .iter()
        .filter(|r| r.start_line.is_some())
        .map(|r| {
            serde_json::json!({
                "id": r.id,
                "name": r.name.clone().unwrap_or_default(),
                "label": r.label,
                "file": r.file_path.clone().unwrap_or_else(|| path.to_string()),
                "line": r.start_line.unwrap_or(0),
                "end_line": r.end_line.or(r.start_line).unwrap_or(0),
            })
        })
        .collect();
    serde_json::json!({ "path": path, "symbols": symbols })
}

fn code_search_in_handle(
    handle: &RepoHandle,
    query: &str,
    limit: usize,
    context: usize,
    regex: bool,
    path_filter: Option<&str>,
) -> Result<CodeSearchResult> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(CodeSearchResult {
            hits: Vec::new(),
            directories: Vec::new(),
        });
    }
    let re = if regex {
        Some(regex::Regex::new(query).with_context(|| format!("invalid regex: {query}"))?)
    } else {
        None
    };
    let needle = query.to_ascii_lowercase();
    let mut hits = Vec::new();
    let mut dirs: HashMap<String, usize> = HashMap::new();
    let files = {
        let store = handle.store.lock().expect("store lock");
        store.searchable_file_list()?
    };

    for (file_path, _) in files {
        if path_filter.is_some_and(|f| !file_path.contains(f)) {
            continue;
        }
        let lines = match read_source_lines(&handle.entry.repo_path, &file_path) {
            Ok(lines) => lines,
            Err(_) => continue,
        };
        let scan = find_code_line_matches(&lines, &needle, re.as_ref(), context);
        if scan.lines.is_empty() {
            continue;
        }
        let dir = top_level_dir(&file_path);
        *dirs.entry(dir).or_default() += scan.raw_count;
        let symbol = nearest_symbol_for_match(handle, &file_path, scan.first_line)?;
        hits.push(CodeSearchHit {
            node_id: symbol
                .as_ref()
                .map(|r| r.id.clone())
                .unwrap_or_else(|| format!("file:{file_path}")),
            name: symbol
                .as_ref()
                .and_then(|r| r.name.clone())
                .unwrap_or_else(|| file_path.clone()),
            label: symbol
                .as_ref()
                .map(|r| r.label.clone())
                .unwrap_or_else(|| "File".into()),
            file_path,
            start_line: symbol
                .as_ref()
                .and_then(|r| r.start_line)
                .unwrap_or(scan.first_line),
            score: scan.raw_count as f32,
            matches: scan.lines,
        });
    }

    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    hits.truncate(limit);
    let mut directories: Vec<DirectoryCount> = dirs
        .into_iter()
        .map(|(dir, count)| DirectoryCount { dir, count })
        .collect();
    directories.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.dir.cmp(&b.dir)));
    Ok(CodeSearchResult { hits, directories })
}

struct CodeLineScan {
    lines: Vec<CodeLineMatch>,
    raw_count: usize,
    first_line: u32,
}

fn read_source_lines(repo_root: &Path, rel: &str) -> Result<Vec<String>> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        bail!("invalid path (must be repo-relative): {rel}");
    }
    let root = repo_root
        .canonicalize()
        .with_context(|| format!("repo path not found: {}", repo_root.display()))?;
    let abs = root
        .join(rel_path)
        .canonicalize()
        .with_context(|| format!("file not found in repo: {rel}"))?;
    if !abs.starts_with(&root) || !abs.is_file() {
        bail!("invalid path: {rel}");
    }
    let bytes = std::fs::read(&abs).with_context(|| format!("read {}", abs.display()))?;
    if bytes.contains(&0) {
        bail!("invalid file (binary content): {rel}");
    }
    let text = std::str::from_utf8(&bytes).with_context(|| format!("invalid UTF-8: {rel}"))?;
    Ok(text.lines().map(str::to_string).collect())
}

fn find_code_line_matches(
    lines: &[String],
    needle: &str,
    re: Option<&regex::Regex>,
    context: usize,
) -> CodeLineScan {
    let mut out: Vec<CodeLineMatch> = Vec::new();
    let mut raw_count = 0;
    let mut first_line = 1;
    for (idx, line) in lines.iter().enumerate() {
        let matched = if let Some(re) = re {
            re.is_match(line)
        } else {
            line_matches_literal_or_terms(line, needle)
        };
        if !matched {
            continue;
        }
        raw_count += 1;
        if raw_count == 1 {
            first_line = (idx + 1) as u32;
        }
        let from = idx.saturating_sub(context);
        let to = (idx + context + 1).min(lines.len());
        for (ctx_idx, text) in lines.iter().enumerate().take(to).skip(from) {
            let line_no = (ctx_idx + 1) as u32;
            if let Some(existing) = out.iter_mut().find(|m| m.line == line_no) {
                existing.matched |= line_no == (idx + 1) as u32;
            } else {
                out.push(CodeLineMatch {
                    line: line_no,
                    text: text.clone(),
                    matched: line_no == (idx + 1) as u32,
                });
            }
        }
    }
    CodeLineScan {
        lines: out,
        raw_count,
        first_line,
    }
}

fn line_matches_literal_or_terms(line: &str, needle: &str) -> bool {
    let line_lower = line.to_ascii_lowercase();
    if line_lower.contains(needle) {
        return true;
    }
    let query_terms = code_search_terms(needle);
    if query_terms.is_empty() {
        return false;
    }
    let line_terms = code_search_terms(line);
    query_terms
        .iter()
        .all(|term| line_terms.iter().any(|line_term| line_term == term))
}

fn code_search_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut prev: Option<char> = None;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            if let Some(prev) = prev {
                if should_split_identifier(prev, ch) && !current.is_empty() {
                    terms.push(current.to_ascii_lowercase());
                    current.clear();
                }
            }
            current.push(ch);
            prev = Some(ch);
        } else {
            if !current.is_empty() {
                terms.push(current.to_ascii_lowercase());
                current.clear();
            }
            prev = None;
        }
    }
    if !current.is_empty() {
        terms.push(current.to_ascii_lowercase());
    }
    terms.sort();
    terms.dedup();
    terms
}

fn should_split_identifier(prev: char, ch: char) -> bool {
    (prev.is_ascii_lowercase() && ch.is_ascii_uppercase())
        || (prev.is_ascii_alphabetic() && ch.is_ascii_digit())
        || (prev.is_ascii_digit() && ch.is_ascii_alphabetic())
}

fn run_git_diff(repo: &Path, scope: &str, base_ref: Option<&str>) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo).arg("diff").arg("--unified=0");
    hide_child_console(&mut cmd);
    match scope {
        "unstaged" => {}
        "staged" | "cached" => {
            cmd.arg("--cached");
        }
        "all" => {
            cmd.arg("HEAD");
        }
        "compare" => {
            let base = base_ref.context("base_ref is required when scope = compare")?;
            cmd.arg(format!("{base}...HEAD"));
        }
        other => {
            bail!("invalid change scope {other:?}; expected unstaged, staged, all, or compare")
        }
    }
    let output = cmd
        .output()
        .with_context(|| format!("run git diff in {}", repo.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_changed_ranges(diff: &str) -> Vec<ChangedRange> {
    let mut file: Option<String> = None;
    let mut ranges = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            file = parse_diff_file(rest);
            continue;
        }
        if !line.starts_with("@@ ") {
            continue;
        }
        let Some(path) = file.clone() else {
            continue;
        };
        if path == "/dev/null" {
            continue;
        }
        if let Some((start, count)) = parse_hunk_new_range(line) {
            let end = if count == 0 {
                start
            } else {
                start.saturating_add(count).saturating_sub(1)
            };
            ranges.push(ChangedRange {
                file_path: path,
                start_line: start.max(1),
                end_line: end.max(start.max(1)),
            });
        }
    }
    merge_changed_ranges(ranges)
}

fn parse_diff_file(raw: &str) -> Option<String> {
    let path = raw.split('\t').next().unwrap_or(raw);
    if path == "/dev/null" {
        return Some(path.to_string());
    }
    let path = path.strip_prefix("b/").unwrap_or(path);
    Some(path.to_string())
}

fn parse_hunk_new_range(line: &str) -> Option<(u32, u32)> {
    let plus = line.split_whitespace().find(|part| part.starts_with('+'))?;
    let body = plus.strip_prefix('+')?;
    let (start, count) = body.split_once(',').unwrap_or((body, "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}

fn merge_changed_ranges(mut ranges: Vec<ChangedRange>) -> Vec<ChangedRange> {
    ranges.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.end_line.cmp(&b.end_line))
    });
    let mut merged: Vec<ChangedRange> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut() {
            if last.file_path == range.file_path && range.start_line <= last.end_line + 1 {
                last.end_line = last.end_line.max(range.end_line);
                continue;
            }
        }
        merged.push(range);
    }
    merged
}

fn changed_symbols_in_handle(
    handle: &RepoHandle,
    ranges: &[ChangedRange],
) -> Result<Vec<ChangedSymbol>> {
    let mut by_id: HashMap<String, ChangedSymbol> = HashMap::new();
    let store = handle.store.lock().expect("store lock");
    for range in ranges {
        let rows = store.nodes_in_file(&range.file_path)?;
        for row in rows {
            let Some(start) = row.start_line else {
                continue;
            };
            if !DEFINITION_LABELS.contains(&row.label.as_str()) {
                continue;
            }
            let end = row.end_line.or(row.start_line).unwrap_or(start);
            if end < range.start_line || start > range.end_line {
                continue;
            }
            let entry = by_id
                .entry(row.id.clone())
                .or_insert_with(|| ChangedSymbol {
                    node_id: row.id.clone(),
                    name: row.name.clone().unwrap_or_default(),
                    label: row.label.clone(),
                    file_path: row.file_path.clone().unwrap_or_default(),
                    start_line: start,
                    end_line: end,
                    ranges: Vec::new(),
                });
            entry.ranges.push(range.clone());
        }
    }
    let mut out: Vec<ChangedSymbol> = by_id.into_values().collect();
    for symbol in &mut out {
        symbol.ranges = merge_changed_ranges(std::mem::take(&mut symbol.ranges));
    }
    out.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    Ok(out)
}

fn nearest_symbol_for_match(
    handle: &RepoHandle,
    file_path: &str,
    line: u32,
) -> Result<Option<NodeRow>> {
    let rows = {
        let store = handle.store.lock().expect("store lock");
        store.nodes_in_file(file_path)?
    };
    let mut best_before: Option<NodeRow> = None;
    let mut first_symbol: Option<NodeRow> = None;
    for row in rows {
        if row.start_line.is_none() {
            continue;
        }
        if first_symbol.is_none() {
            first_symbol = Some(row.clone());
        }
        if row.start_line.unwrap_or(0) <= line {
            best_before = Some(row);
        } else {
            break;
        }
    }
    Ok(best_before.or(first_symbol))
}

fn top_level_dir(path: &str) -> String {
    path.split('/')
        .find(|s| !s.is_empty())
        .unwrap_or("(root)")
        .to_string()
}

/// analyze 落注册表后补 name / source 字段（register() 只继承已有条目，
/// 新导入的 git/zip 仓库要在这里盖上来源）。
fn finalize_entry(repo_path: &Path, name: &str, kind: &str, url: Option<String>) -> Result<()> {
    let repo_path = user_facing_path(repo_path);
    let mut registry = Registry::load()?;
    if let Some(entry) = registry.repos.iter_mut().find(|r| r.repo_path == repo_path) {
        entry.name = name.to_string();
        entry.source_kind = kind.to_string();
        entry.source_url = url;
        registry.save()?;
    }
    Ok(())
}

fn resolve_registered_repo_for_update(registry: &Registry, key: &str) -> Result<Option<RepoEntry>> {
    if let Some(entry) = registry.find_by_name(key).cloned() {
        return Ok(Some(entry));
    }
    if !looks_like_local_path(key) {
        return Ok(None);
    }
    let requested = PathBuf::from(key);
    let Some(path) = discover_workspace_root(&requested)?
        .or_else(|| requested.canonicalize().ok())
        .map(|path| user_facing_path(&path))
    else {
        return Ok(None);
    };
    Ok(registry.find(&path).cloned())
}

impl AkaBackend {
    pub fn new() -> Self {
        Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
            jobs: Arc::new(Mutex::new(HashMap::new())),
            completed_jobs: Arc::new(Mutex::new(HashMap::new())),
            auto_indexer: Arc::new(AutoIndexer::new()),
            engine_dir: None,
            auto_discover_workspace: false,
            job_event_sink: None,
        }
    }

    pub fn clear_runtime_data(&self) -> Result<()> {
        {
            let jobs = self.jobs.lock().expect("jobs lock");
            if jobs.values().any(|j| j.status == "indexing") {
                bail!("cannot clear app data while indexing is running");
            }
        }
        self.handles.lock().expect("handles lock").clear();
        self.jobs.lock().expect("jobs lock").clear();
        let home = aka_home();
        if home.exists() {
            std::fs::remove_dir_all(&home)
                .with_context(|| format!("clear aka data dir {}", home.display()))?;
        }
        std::fs::create_dir_all(&home)
            .with_context(|| format!("recreate aka data dir {}", home.display()))?;
        Ok(())
    }

    pub fn with_engine_dir(engine_dir: PathBuf) -> Self {
        Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
            jobs: Arc::new(Mutex::new(HashMap::new())),
            completed_jobs: Arc::new(Mutex::new(HashMap::new())),
            auto_indexer: Arc::new(AutoIndexer::new()),
            engine_dir: Some(engine_dir),
            auto_discover_workspace: false,
            job_event_sink: None,
        }
    }

    pub fn with_workspace_auto_index(mut self) -> Self {
        self.auto_discover_workspace = true;
        self
    }

    pub fn with_job_event_sink(mut self, sink: impl Fn(String) + Send + Sync + 'static) -> Self {
        self.job_event_sink = Some(Arc::new(sink));
        self
    }

    fn engine_dir(&self) -> Option<PathBuf> {
        self.engine_dir.clone()
    }

    pub fn has_running_jobs(&self) -> bool {
        self.jobs
            .lock()
            .expect("jobs lock")
            .values()
            .any(|job| job.status == "indexing")
    }

    pub fn clear_cached_handles(&self) {
        self.handles.lock().expect("handles lock").clear();
    }

    pub fn start_auto_indexer(&self) {
        let auto = Arc::clone(&self.auto_indexer);
        if auto.running.swap(true, Ordering::Relaxed) {
            return;
        }
        auto.stop.store(false, Ordering::Relaxed);
        let jobs = Arc::clone(&self.jobs);
        let completed = Arc::clone(&self.completed_jobs);
        let handles = Arc::clone(&self.handles);
        let engine_dir = self.engine_dir();
        let job_event_sink = self.job_event_sink.clone();
        std::thread::spawn(move || {
            run_auto_indexer(auto, jobs, completed, handles, engine_dir, job_event_sink);
        });
    }

    pub fn auto_index_current_workspace(&self) -> Result<Option<String>> {
        let cwd = std::env::current_dir().context("read current directory")?;
        let Some(repo) = discover_workspace_root(&cwd)? else {
            return Ok(None);
        };
        self.auto_index_workspace(repo)
    }

    fn ensure_current_workspace_queued(&self) -> Result<Option<String>> {
        if !self.auto_discover_workspace {
            return Ok(None);
        }
        self.auto_index_current_workspace()
    }

    fn auto_index_workspace(&self, repo: PathBuf) -> Result<Option<String>> {
        let repo = repo
            .canonicalize()
            .with_context(|| format!("resolve workspace repo {}", repo.display()))?;
        let repo = user_facing_path(&repo);
        let registry = Registry::load()?;
        if registry.find(&repo).is_some() {
            return Ok(None);
        }
        if self
            .jobs
            .lock()
            .expect("jobs lock")
            .values()
            .any(|job| job.path == repo)
        {
            return Ok(None);
        }
        let name = unique_local_name(&registry, &repo);
        validate_name(&name)?;
        let job_name = name.clone();
        let job_path = repo.clone();
        let engine_dir = self.engine_dir();
        self.spawn_job(name.clone(), "local", None, repo, move |jobs, name| {
            update_job(&jobs, &name, |job| {
                job.set_stage(
                    "queued",
                    "Current workspace detected by MCP; indexing automatically",
                    2.0,
                );
            });
            run_analyze_job(&jobs, &name, job_path.clone(), engine_dir)?;
            update_job(&jobs, &name, |job| {
                job.set_stage("register", "Saving repository metadata", 98.0);
            });
            finalize_entry(&job_path, &job_name, "local", None)?;
            Ok(job_path)
        });
        Ok(Some(name))
    }

    fn resolve_explicit_repo_key(&self, key: &str, registry: &Registry) -> Result<Vec<RepoEntry>> {
        if let Some(entry) = registry
            .repos
            .iter()
            .find(|r| r.name == key || r.repo_path.to_string_lossy() == key)
            .cloned()
        {
            return Ok(vec![entry]);
        }
        if !looks_like_local_path(key) {
            bail!("未注册的仓库: {key}（aka repos 查看）");
        }
        let requested = PathBuf::from(key);
        let repo = match discover_workspace_root(&requested) {
            Ok(Some(repo)) => repo,
            Ok(None) => bail!("未注册的仓库: {key}（aka repos 查看）"),
            Err(err) => bail!("未注册的仓库: {key}（{err:#}）"),
        };
        if let Some(entry) = registry.find(&repo).cloned() {
            return Ok(vec![entry]);
        }
        if let Some(name) = self.auto_index_workspace(repo)? {
            bail!("仓库正在自动索引中，请稍后重试: {name}");
        }
        bail!("仓库仍在索引中，请稍后完成后重试: {key}")
    }

    /// 名字可用性守卫：已注册或有进行中任务 → 拒绝。
    fn guard_name_free(&self, name: &str) -> Result<()> {
        if Registry::load()?.find_by_name(name).is_some() {
            bail!("repo already registered: {name}");
        }
        let jobs = self.jobs.lock().expect("jobs lock");
        if jobs.get(name).is_some_and(|j| j.status == "indexing") {
            bail!("a job is already running for repo: {name}");
        }
        Ok(())
    }

    /// 受管 checkout 目录：守卫名字后清掉失败导入的残留目录。
    fn prepare_checkout(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        self.guard_name_free(name)?;
        let dir = checkouts_dir().join(name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("clear stale checkout {}", dir.display()))?;
        }
        std::fs::create_dir_all(checkouts_dir())?;
        Ok(dir)
    }

    /// 登记后台任务并 spawn 线程执行。`work` 成功返回仓库路径（旧句柄作废），
    /// 失败则任务态置 failed + 原因，等下一次导入/更新覆盖。
    fn spawn_job(
        &self,
        name: String,
        kind: &str,
        url: Option<String>,
        path: PathBuf,
        work: impl FnOnce(Arc<Mutex<HashMap<String, JobInfo>>>, String) -> Result<PathBuf>
            + Send
            + 'static,
    ) {
        let path = user_facing_path(&path);
        let jobs = Arc::clone(&self.jobs);
        let completed = Arc::clone(&self.completed_jobs);
        let handles = Arc::clone(&self.handles);
        let job_event_sink = self.job_event_sink.clone();
        emit_job_event(
            &job_event_sink,
            format!(
                "index job queued name={name} kind={kind} path={}",
                path.display()
            ),
        );
        jobs.lock()
            .expect("jobs lock")
            .insert(name.clone(), JobInfo::new(kind, url, path));
        std::thread::spawn(move || {
            emit_job_event(&job_event_sink, format!("index job started name={name}"));
            match work(Arc::clone(&jobs), name.clone()) {
                Ok(repo_path) => {
                    handles.lock().expect("handles lock").remove(&repo_path);
                    mark_job_done(&jobs, &completed, &name);
                    jobs.lock().expect("jobs lock").remove(&name);
                    emit_job_event(
                        &job_event_sink,
                        format!(
                            "index job completed name={name} path={}",
                            repo_path.display()
                        ),
                    );
                }
                Err(e) => {
                    let detail = format!("{e:#}");
                    if let Some(job) = jobs.lock().expect("jobs lock").get_mut(&name) {
                        job.status = "failed".into();
                        job.detail = Some(detail.clone());
                        job.progress.stage = "failed".into();
                        job.progress.message = "Indexing failed".into();
                        job.push_log(format!("failed: {detail}"));
                    }
                    emit_job_event(
                        &job_event_sink,
                        format!("index job failed name={name}: {detail}"),
                    );
                }
            }
        });
    }

    /// 解析 repo 参数（名字或路径；None = 全部已索引仓库）。
    fn targets(&self, repo: Option<&str>) -> Result<Vec<Arc<RepoHandle>>> {
        if repo.is_none() {
            let _ = self.ensure_current_workspace_queued();
        }
        let registry = Registry::load()?;
        let jobs = self.jobs.lock().expect("jobs lock").clone();
        let entries: Vec<RepoEntry> = match repo {
            Some(key) => {
                if let Some((_, job)) = jobs
                    .iter()
                    .find(|(name, job)| job_matches_key(name, job, key))
                {
                    match job.status.as_str() {
                        "indexing" => bail!("仓库仍在索引中，请等待完成: {key}"),
                        "failed" => bail!("仓库索引失败: {}", job.detail.as_deref().unwrap_or(key)),
                        _ => {}
                    }
                }
                self.resolve_explicit_repo_key(key, &registry)?
            }
            None => registry.repos.clone(),
        };
        if entries.is_empty() {
            let pending = jobs.values().any(|job| job.status == "indexing");
            if pending {
                bail!("当前工作区正在自动索引中，请稍后重试或先调用 list_repos 查看进度");
            }
            bail!("没有已注册的仓库——MCP 会自动尝试索引当前工作区；也可显式调用 analyze 传入仓库绝对路径");
        }

        let mut cache = self.handles.lock().expect("handles lock");
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            if let Some(job) = jobs.get(&entry.name) {
                match job.status.as_str() {
                    "indexing" => continue,
                    "failed" => continue,
                    _ => {}
                }
            }
            let key = entry.repo_path.clone();
            if let Some(h) = cache.get(&key) {
                out.push(Arc::clone(h));
                continue;
            }
            let handle = Arc::new(RepoHandle::open(entry)?);
            cache.insert(key, Arc::clone(&handle));
            out.push(handle);
        }
        if out.is_empty() {
            bail!("没有已就绪的仓库——请等待索引完成，或调用 list_repos 查看进度");
        }
        Ok(out)
    }

    fn traverse(
        &self,
        repo: Option<&str>,
        symbol: &str,
        edge_label: &str,
        f: impl Fn(&RepoHandle, u32) -> Vec<(u32, u32)>,
    ) -> Result<Vec<SymbolRef>> {
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            let Some(node) = handle.resolve_symbol(symbol)? else {
                continue;
            };
            for (n, depth) in f(&handle, node) {
                if let Some(row) = handle.node_row(n)? {
                    out.push(SymbolRef {
                        node_id: row.id.clone(),
                        name: row.name.clone().unwrap_or_default(),
                        label: row.label.clone(),
                        file_path: row.file_path.clone().unwrap_or_default(),
                        start_line: row.start_line.unwrap_or(0),
                        edge_type: edge_label.to_string(),
                        depth,
                    });
                }
            }
        }
        Ok(out)
    }

    fn traverse_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        edge_label: &str,
        f: impl Fn(&RepoHandle, u32) -> Vec<(u32, u32)>,
    ) -> Result<Vec<SymbolRef>> {
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            for node in handle.resolve_selector(selector)? {
                for (n, depth) in f(&handle, node) {
                    if let Some(row) = handle.node_row(n)? {
                        out.push(SymbolRef {
                            node_id: row.id.clone(),
                            name: row.name.clone().unwrap_or_default(),
                            label: row.label.clone(),
                            file_path: row.file_path.clone().unwrap_or_default(),
                            start_line: row.start_line.unwrap_or(0),
                            edge_type: edge_label.to_string(),
                            depth,
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.start_line.cmp(&b.start_line))
                .then_with(|| a.node_id.cmp(&b.node_id))
        });
        dedup_symbol_refs(&mut out);
        Ok(out)
    }
}

impl Default for AkaBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for AkaBackend {
    fn list_repos(&self) -> Result<Vec<RepoInfo>> {
        let _ = self.ensure_current_workspace_queued();
        let registry = Registry::load()?;
        let jobs = {
            let mut guard = self.jobs.lock().expect("jobs lock");
            for job in guard.values_mut() {
                job.maybe_heartbeat();
            }
            guard.clone()
        };
        let completed = self
            .completed_jobs
            .lock()
            .expect("completed jobs lock")
            .clone();
        let mut out: Vec<RepoInfo> = registry
            .repos
            .iter()
            .map(|r| {
                let (status, detail) = jobs
                    .get(&r.name)
                    .map(|j| (j.status.clone(), j.detail.clone()))
                    .unwrap_or_else(|| ("ready".into(), None));
                RepoInfo {
                    name: r.name.clone(),
                    path: r.repo_path.to_string_lossy().to_string(),
                    nodes: r.stats.nodes,
                    edges: r.stats.edges,
                    indexed_at: r.indexed_at,
                    embeddings_enabled: r.embeddings_enabled,
                    status,
                    source_kind: r.source_kind.clone(),
                    source_url: r.source_url.clone(),
                    detail,
                    render_max_nodes: r.render_max_nodes,
                    progress: jobs
                        .get(&r.name)
                        .map(|j| j.progress.clone())
                        .or_else(|| completed.get(&r.name).cloned()),
                }
            })
            .collect();
        // 进行中 / 失败的导入还没进注册表 → 合成条目供前端轮询（名字序保证确定性）。
        let mut pending: Vec<(&String, &JobInfo)> = jobs
            .iter()
            .filter(|(name, _)| registry.find_by_name(name).is_none())
            .collect();
        pending.sort_by_key(|(name, _)| name.as_str());
        for (name, job) in pending {
            out.push(RepoInfo {
                name: name.clone(),
                path: job.path.to_string_lossy().to_string(),
                nodes: 0,
                edges: 0,
                indexed_at: None,
                embeddings_enabled: false,
                status: job.status.clone(),
                source_kind: job.kind.clone(),
                source_url: job.url.clone(),
                detail: job.detail.clone(),
                render_max_nodes: None,
                progress: Some(job.progress.clone()),
            });
        }
        Ok(out)
    }

    fn search(&self, repo: Option<&str>, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let mut hits: Vec<SearchHit> = Vec::new();
        for handle in self.targets(repo)? {
            for h in handle.search.search(query, limit)? {
                hits.push(SearchHit {
                    node_id: h.node_id,
                    name: h.name,
                    label: h.label,
                    kind: h.kind,
                    file_path: h.file_path,
                    start_line: h.start_line,
                    score: h.score,
                    snippet: h.snippet,
                });
            }
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(limit);
        Ok(hits)
    }

    fn search_code(
        &self,
        repo: Option<&str>,
        query: &str,
        limit: usize,
        context: usize,
        regex: bool,
        path_filter: Option<&str>,
    ) -> Result<CodeSearchResult> {
        let mut hits = Vec::new();
        let mut dirs: HashMap<String, usize> = HashMap::new();
        for handle in self.targets(repo)? {
            let result = code_search_in_handle(&handle, query, limit, context, regex, path_filter)?;
            hits.extend(result.hits);
            for d in result.directories {
                *dirs.entry(d.dir).or_default() += d.count;
            }
        }
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.start_line.cmp(&b.start_line))
        });
        hits.truncate(limit);
        let mut directories: Vec<DirectoryCount> = dirs
            .into_iter()
            .map(|(dir, count)| DirectoryCount { dir, count })
            .collect();
        directories.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.dir.cmp(&b.dir)));
        Ok(CodeSearchResult { hits, directories })
    }

    fn find_definition(&self, repo: Option<&str>, symbol: &str) -> Result<Vec<SearchHit>> {
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            let rows = {
                let store = handle.store.lock().expect("store lock");
                store.nodes_by_name(symbol, 20)?
            };
            let exact: Vec<&NodeRow> = rows
                .iter()
                .filter(|r| {
                    r.name.as_deref() == Some(symbol)
                        && DEFINITION_LABELS.contains(&r.label.as_str())
                })
                .collect();
            let chosen: Vec<&NodeRow> = if exact.is_empty() {
                rows.iter()
                    .filter(|r| r.name.as_deref() == Some(symbol))
                    .collect()
            } else {
                exact
            };
            out.extend(chosen.into_iter().map(|r| row_to_hit(r, 1.0)));
        }
        Ok(out)
    }

    fn find_definition_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
    ) -> Result<Vec<SearchHit>> {
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            out.extend(
                handle
                    .definition_rows_by_selector(selector)?
                    .into_iter()
                    .map(|r| row_to_hit(&r, 1.0)),
            );
        }
        out.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.start_line.cmp(&b.start_line))
                .then_with(|| a.node_id.cmp(&b.node_id))
        });
        Ok(out)
    }

    fn references(&self, repo: Option<&str>, symbol: &str, limit: usize) -> Result<Vec<SymbolRef>> {
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            let Some(node) = handle.resolve_symbol(symbol)? else {
                continue;
            };
            for nb in handle.adj.neighbors(node) {
                if nb.outgoing {
                    continue; // 引用 = 入边
                }
                if let Some(row) = handle.node_row(nb.node)? {
                    out.push(SymbolRef {
                        node_id: row.id.clone(),
                        name: row.name.clone().unwrap_or_default(),
                        label: row.label.clone(),
                        file_path: row.file_path.clone().unwrap_or_default(),
                        start_line: row.start_line.unwrap_or(0),
                        edge_type: nb.edge_type.to_string(),
                        depth: 1,
                    });
                }
                if out.len() >= limit {
                    break;
                }
            }
        }
        Ok(out)
    }

    fn references_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        limit: usize,
    ) -> Result<Vec<SymbolRef>> {
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            for node in handle.resolve_selector(selector)? {
                for nb in handle.adj.neighbors(node) {
                    if nb.outgoing {
                        continue;
                    }
                    if let Some(row) = handle.node_row(nb.node)? {
                        out.push(SymbolRef {
                            node_id: row.id.clone(),
                            name: row.name.clone().unwrap_or_default(),
                            label: row.label.clone(),
                            file_path: row.file_path.clone().unwrap_or_default(),
                            start_line: row.start_line.unwrap_or(0),
                            edge_type: nb.edge_type.to_string(),
                            depth: 1,
                        });
                    }
                }
            }
        }
        out.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.start_line.cmp(&b.start_line))
                .then_with(|| a.node_id.cmp(&b.node_id))
        });
        dedup_symbol_refs(&mut out);
        out.truncate(limit);
        Ok(out)
    }

    fn callers(&self, repo: Option<&str>, symbol: &str, depth: u32) -> Result<Vec<SymbolRef>> {
        self.traverse(repo, symbol, "CALLS", |h, n| {
            h.adj.callers(n, depth.max(1), 200)
        })
    }

    fn callers_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        depth: u32,
    ) -> Result<Vec<SymbolRef>> {
        self.traverse_selector(repo, selector, "CALLS", |h, n| {
            h.adj.callers(n, depth.max(1), 200)
        })
    }

    fn callees(&self, repo: Option<&str>, symbol: &str, depth: u32) -> Result<Vec<SymbolRef>> {
        self.traverse(repo, symbol, "CALLS", |h, n| {
            h.adj.callees(n, depth.max(1), 200)
        })
    }

    fn callees_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        depth: u32,
    ) -> Result<Vec<SymbolRef>> {
        self.traverse_selector(repo, selector, "CALLS", |h, n| {
            h.adj.callees(n, depth.max(1), 200)
        })
    }

    fn impact(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
        limit: usize,
    ) -> Result<Vec<SymbolRef>> {
        self.traverse(repo, symbol, "IMPACT", |h, n| {
            h.adj.impact(n, depth.max(1), limit)
        })
    }

    fn impact_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        direction: ImpactDirection,
        depth: u32,
        limit: usize,
    ) -> Result<Vec<SymbolRef>> {
        let mut out = match direction {
            ImpactDirection::Upstream => {
                self.traverse_selector(repo, selector, "IMPACT", |h, n| {
                    h.adj.impact(n, depth.max(1), limit)
                })?
            }
            ImpactDirection::Downstream => {
                self.traverse_selector(repo, selector, "CALLS", |h, n| {
                    h.adj.callees(n, depth.max(1), limit)
                })?
            }
            ImpactDirection::Both => {
                let mut refs = self.impact_by_selector(
                    repo,
                    selector,
                    ImpactDirection::Upstream,
                    depth,
                    limit,
                )?;
                refs.extend(self.impact_by_selector(
                    repo,
                    selector,
                    ImpactDirection::Downstream,
                    depth,
                    limit,
                )?);
                refs
            }
        };
        out.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.start_line.cmp(&b.start_line))
                .then_with(|| a.node_id.cmp(&b.node_id))
        });
        dedup_symbol_refs(&mut out);
        out.truncate(limit);
        Ok(out)
    }

    fn rename_symbol(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        replacement: &str,
        dry_run: bool,
    ) -> Result<RenamePlan> {
        rename::validate_identifier_name(replacement)?;
        if !dry_run && repo.is_none() {
            bail!("rename dry_run=false requires an explicit repo or repo_path");
        }
        let defs = self.find_definition_by_selector(repo, selector)?;
        if should_disambiguate_for_backend(selector, &defs) {
            return Ok(RenamePlan {
                status: "ambiguous".into(),
                target: selector.label().to_string(),
                replacement: replacement.to_string(),
                dry_run,
                edits: Vec::new(),
                changed_files: 0,
                applied: false,
                message: Some("Multiple definitions match; pass uid/file_path/kind.".into()),
                candidates: defs,
            });
        }
        let Some(target) = selector
            .symbol
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| defs.first().map(|h| h.name.as_str()))
        else {
            bail!("rename requires a symbol name or a uid resolving to a named symbol");
        };
        if target == replacement {
            return Ok(RenamePlan {
                status: "ok".into(),
                target: target.to_string(),
                replacement: replacement.to_string(),
                dry_run,
                edits: Vec::new(),
                changed_files: 0,
                applied: false,
                message: Some("Replacement is identical to target.".into()),
                candidates: Vec::new(),
            });
        }

        let mut edits = Vec::new();
        let mut changed_files = 0usize;
        for handle in self.targets(repo)? {
            let handle_defs = handle
                .definition_rows_by_selector(selector)?
                .into_iter()
                .map(|r| row_to_hit(&r, 1.0))
                .collect::<Vec<_>>();
            let handle_refs = if handle_defs.is_empty() {
                Vec::new()
            } else {
                let mut out = Vec::new();
                for node in handle.resolve_selector(selector)? {
                    for nb in handle.adj.neighbors(node) {
                        if nb.outgoing {
                            continue;
                        }
                        if let Some(row) = handle.node_row(nb.node)? {
                            out.push(SymbolRef {
                                node_id: row.id.clone(),
                                name: row.name.clone().unwrap_or_default(),
                                label: row.label.clone(),
                                file_path: row.file_path.clone().unwrap_or_default(),
                                start_line: row.start_line.unwrap_or(0),
                                edge_type: nb.edge_type.to_string(),
                                depth: 1,
                            });
                        }
                        if out.len() >= RENAME_REFERENCE_LIMIT {
                            break;
                        }
                    }
                    if out.len() >= RENAME_REFERENCE_LIMIT {
                        break;
                    }
                }
                out
            };
            let ranges_by_file = rename::collect_file_ranges(&handle_defs, &handle_refs);
            for (file_path, ranges) in &ranges_by_file {
                if let Some(mut file_edits) = rename::apply_file_plan(
                    &handle.entry.repo_path,
                    file_path,
                    ranges,
                    target,
                    replacement,
                    dry_run,
                )? {
                    changed_files += 1;
                    edits.append(&mut file_edits);
                }
            }
        }
        edits.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.column.cmp(&b.column))
        });
        Ok(RenamePlan {
            status: "ok".into(),
            target: target.to_string(),
            replacement: replacement.to_string(),
            dry_run,
            changed_files,
            applied: !dry_run && !edits.is_empty(),
            message: (edits.is_empty()).then(|| {
                "No identifier occurrences found in indexed definition/reference windows.".into()
            }),
            edits,
            candidates: Vec::new(),
        })
    }

    fn processes_of(&self, repo: Option<&str>, node_id: &str) -> Result<Vec<ProcessHit>> {
        // node_id 是图谱节点 id：先解析 rowid 再查 STEP_IN_PROCESS 归属。
        // 跨仓库查询（repo = None）时节点只会落在其中一个库里，查不到跳过即可。
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            let store = handle.store.lock().expect("store lock");
            let Some(row) = store.node_by_id(node_id)? else {
                continue;
            };
            for m in store.processes_of_node(row.rowid)? {
                out.push(ProcessHit {
                    process_id: m.process_id,
                    name: m.name,
                    process_type: m.process_type,
                    step: m.step,
                    step_count: m.step_count,
                });
            }
        }
        Ok(out)
    }

    fn query_enrichment(
        &self,
        repo: Option<&str>,
        node_ids: &[String],
        include_content: bool,
    ) -> Result<HashMap<String, QueryEnrichment>> {
        let mut out = HashMap::new();
        if node_ids.is_empty() {
            return Ok(out);
        }
        for handle in self.targets(repo)? {
            let (processes_by_node, community_by_node, rows_by_id) = {
                let store = handle.store.lock().expect("store lock");
                let processes = store.processes_of_node_ids(node_ids)?;
                let communities = store.community_of_node_ids(node_ids)?;
                let mut rows = HashMap::new();
                if include_content {
                    for id in node_ids {
                        if let Some(row) = store.node_by_id(id)? {
                            rows.insert(id.clone(), row);
                        }
                    }
                }
                (processes, communities, rows)
            };

            for id in node_ids {
                let processes = processes_by_node
                    .get(id)
                    .map(|rows| {
                        rows.iter()
                            .map(|m| ProcessHit {
                                process_id: m.process_id.clone(),
                                name: m.name.clone(),
                                process_type: m.process_type.clone(),
                                step: m.step,
                                step_count: m.step_count,
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let community = community_by_node.get(id);
                let content = if include_content {
                    rows_by_id
                        .get(id)
                        .and_then(|row| node_content(&handle.entry.repo_path, row).ok().flatten())
                } else {
                    None
                };
                if !processes.is_empty() || community.is_some() || content.is_some() {
                    out.insert(
                        id.clone(),
                        QueryEnrichment {
                            processes,
                            module: community
                                .and_then(|c| (!c.module.is_empty()).then(|| c.module.clone())),
                            cohesion: community.map(|c| c.cohesion).unwrap_or(0.0),
                            content,
                        },
                    );
                }
            }
        }
        Ok(out)
    }

    fn graph_lod(&self, repo: &str, max_nodes: Option<usize>) -> Result<serde_json::Value> {
        // 缺省 → per-repo render_max_nodes 设置 → 默认 50_000；一律 clamp 到硬上限。
        let requested = match max_nodes {
            Some(n) => u32::try_from(n).unwrap_or(u32::MAX),
            None => Registry::load()?
                .repos
                .iter()
                .find(|r| r.name == repo || r.repo_path.to_string_lossy() == repo)
                .and_then(|r| r.render_max_nodes)
                .unwrap_or(DEFAULT_RENDER_MAX_NODES),
        };
        let budget = clamp_render_nodes(requested) as usize;
        let handles = self.targets(Some(repo))?;
        let handle = handles.first().context("repo handle")?;
        let store = handle.store.lock().expect("store lock");
        let lod = store.lod_snapshot(budget)?;
        Ok(serde_json::to_value(lod)?)
    }

    fn graph_clusters(&self, repo: &str) -> Result<serde_json::Value> {
        let handles = self.targets(Some(repo))?;
        let handle = handles.first().context("repo handle")?;
        let store = handle.store.lock().expect("store lock");
        let clusters = store.cluster_lod_snapshot()?;
        Ok(serde_json::to_value(clusters)?)
    }

    fn analyze(&self, repo_path: &str) -> Result<String> {
        let requested = PathBuf::from(repo_path);
        let path = discover_workspace_root(&requested)?
            .or_else(|| requested.canonicalize().ok())
            .with_context(|| format!("resolve repository path {repo_path}"))?;
        let path = user_facing_path(&path);
        if !path.is_dir() {
            bail!("repository path is not a directory: {repo_path}");
        }
        if let Some(entry) = Registry::load()?.find(&path).cloned() {
            self.update_repo(&entry.name)
        } else {
            match self.auto_index_workspace(path)? {
                Some(name) => Ok(format!(
                    "indexing scheduled: {name} (local import + analyze)"
                )),
                None => Ok("indexing already queued for this repository".into()),
            }
        }
    }

    fn queue_workspaces(&self, roots: &[PathBuf]) -> Result<Vec<String>> {
        let mut queued = Vec::new();
        for root in roots {
            let Ok(Some(repo)) = discover_workspace_root(root) else {
                continue;
            };
            if let Some(name) = self.auto_index_workspace(repo)? {
                queued.push(name);
            }
        }
        Ok(queued)
    }

    fn detect_changes(
        &self,
        repo: Option<&str>,
        scope: &str,
        base_ref: Option<&str>,
    ) -> Result<ChangeDetection> {
        let handles = self.targets(repo)?;
        if repo.is_none() && handles.len() != 1 {
            bail!(
                "detect_changes requires repo when multiple repositories are indexed ({} ready)",
                handles.len()
            );
        }
        let handle = handles.first().context("repo handle")?;
        let scope = scope.to_ascii_lowercase();
        let diff = run_git_diff(&handle.entry.repo_path, &scope, base_ref)?;
        let ranges = parse_changed_ranges(&diff);
        let symbols = changed_symbols_in_handle(handle, &ranges)?;
        Ok(ChangeDetection {
            repo: handle.entry.name.clone(),
            scope,
            base_ref: base_ref.map(str::to_string),
            ranges,
            symbols,
        })
    }

    fn route_map(&self, repo: Option<&str>, route: Option<&str>) -> Result<Vec<RouteMapEntry>> {
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            let store = handle.store.lock().expect("store lock");
            for route_row in store.nodes_by_label("Route", route, 500)? {
                let route_name = route_row
                    .name
                    .clone()
                    .or_else(|| string_prop(&route_row.props, "name"))
                    .unwrap_or_else(|| route_row.id.clone());
                let handlers = store.incoming_linked_nodes(route_row.rowid, &["HANDLES_ROUTE"])?;
                let handler = handlers
                    .first()
                    .and_then(|h| h.node.file_path.clone().or(h.node.name.clone()))
                    .or_else(|| route_row.file_path.clone())
                    .or_else(|| string_prop(&route_row.props, "filePath"))
                    .unwrap_or_default();
                let consumers = store
                    .incoming_linked_nodes(route_row.rowid, &["FETCHES", "HTTP_CALLS"])?
                    .into_iter()
                    .map(|linked| {
                        let (accessed_keys, fetch_count) = parse_fetch_reason(&linked.reason);
                        RouteConsumer {
                            name: linked
                                .node
                                .name
                                .clone()
                                .unwrap_or_else(|| linked.node.id.clone()),
                            file_path: linked.node.file_path.unwrap_or_default(),
                            accessed_keys,
                            fetch_count,
                        }
                    })
                    .collect::<Vec<_>>();
                let mut flows = Vec::new();
                for p in store.entry_processes_of_node(route_row.rowid)? {
                    flows.push(p.name);
                }
                if flows.is_empty() {
                    for p in store.processes_of_node(route_row.rowid)? {
                        flows.push(p.name);
                    }
                }
                flows.sort();
                flows.dedup();
                out.push(RouteMapEntry {
                    id: route_row.id.clone(),
                    route: route_name,
                    handler,
                    middleware: string_array_prop(&route_row.props, "middleware"),
                    response_keys: string_array_prop(&route_row.props, "responseKeys"),
                    error_keys: string_array_prop(&route_row.props, "errorKeys"),
                    consumers,
                    flows,
                    properties: Some(route_row.props.clone()),
                });
            }
        }
        out.sort_by(|a, b| a.route.cmp(&b.route).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    fn tool_map(&self, repo: Option<&str>, tool: Option<&str>) -> Result<Vec<ToolMapEntry>> {
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            let store = handle.store.lock().expect("store lock");
            for tool_row in store.nodes_by_label("Tool", tool, 500)? {
                let handlers = store
                    .incoming_linked_nodes(tool_row.rowid, &["HANDLES_TOOL"])?
                    .into_iter()
                    .map(|linked| row_to_hit(&linked.node, 1.0))
                    .collect::<Vec<_>>();
                let mut flows = Vec::new();
                for p in store.entry_processes_of_node(tool_row.rowid)? {
                    flows.push(p.name);
                }
                if flows.is_empty() {
                    for p in store.processes_of_node(tool_row.rowid)? {
                        flows.push(p.name);
                    }
                }
                flows.sort();
                flows.dedup();
                out.push(ToolMapEntry {
                    id: tool_row.id.clone(),
                    name: tool_row
                        .name
                        .clone()
                        .or_else(|| string_prop(&tool_row.props, "name"))
                        .unwrap_or_else(|| tool_row.id.clone()),
                    file_path: tool_row
                        .file_path
                        .clone()
                        .or_else(|| string_prop(&tool_row.props, "filePath"))
                        .unwrap_or_default(),
                    description: string_prop(&tool_row.props, "description")
                        .unwrap_or_default()
                        .chars()
                        .take(200)
                        .collect(),
                    handlers,
                    flows,
                    properties: Some(tool_row.props.clone()),
                });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(out)
    }

    fn graphql_map(
        &self,
        repo: Option<&str>,
        operation: Option<&str>,
    ) -> Result<Vec<GraphqlMapEntry>> {
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            let store = handle.store.lock().expect("store lock");
            for row in store.nodes_by_label("GraphQL", operation, 500)? {
                let handlers = store
                    .incoming_linked_nodes(row.rowid, &["HANDLES_GRAPHQL"])?
                    .into_iter()
                    .map(|linked| row_to_hit(&linked.node, 1.0))
                    .collect::<Vec<_>>();
                let mut flows = Vec::new();
                for p in store.entry_processes_of_node(row.rowid)? {
                    flows.push(p.name);
                }
                if flows.is_empty() {
                    for p in store.processes_of_node(row.rowid)? {
                        flows.push(p.name);
                    }
                }
                flows.sort();
                flows.dedup();
                out.push(GraphqlMapEntry {
                    id: row.id.clone(),
                    name: row
                        .name
                        .clone()
                        .or_else(|| string_prop(&row.props, "operationName"))
                        .or_else(|| string_prop(&row.props, "name"))
                        .unwrap_or_else(|| row.id.clone()),
                    operation_type: string_prop(&row.props, "operationType")
                        .unwrap_or_else(|| "query".into()),
                    file_path: row
                        .file_path
                        .clone()
                        .or_else(|| string_prop(&row.props, "filePath"))
                        .unwrap_or_default(),
                    handlers,
                    flows,
                    properties: Some(row.props.clone()),
                });
            }
        }
        out.sort_by(|a, b| {
            a.operation_type
                .cmp(&b.operation_type)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    fn topic_map(
        &self,
        repo: Option<&str>,
        topic: Option<&str>,
        broker: Option<&str>,
    ) -> Result<Vec<TopicMapEntry>> {
        let broker_filter = broker.map(|value| value.to_ascii_lowercase());
        let mut out = Vec::new();
        for handle in self.targets(repo)? {
            let store = handle.store.lock().expect("store lock");
            for row in store.nodes_by_label("Topic", topic, 500)? {
                let topic_broker =
                    string_prop(&row.props, "broker").unwrap_or_else(|| "unknown".into());
                if broker_filter
                    .as_deref()
                    .is_some_and(|wanted| topic_broker.to_ascii_lowercase() != wanted)
                {
                    continue;
                }
                let producers =
                    topic_endpoints_for(&store, row.rowid, &["PUBLISHES_TOPIC", "EMITS"])?;
                let consumers =
                    topic_endpoints_for(&store, row.rowid, &["CONSUMES_TOPIC", "LISTENS_ON"])?;
                let mut flows = producers
                    .iter()
                    .chain(consumers.iter())
                    .flat_map(|endpoint| endpoint.flows.iter().cloned())
                    .collect::<Vec<_>>();
                flows.sort();
                flows.dedup();
                out.push(TopicMapEntry {
                    id: row.id.clone(),
                    name: row
                        .name
                        .clone()
                        .or_else(|| string_prop(&row.props, "name"))
                        .unwrap_or_else(|| row.id.clone()),
                    broker: topic_broker,
                    source: string_prop(&row.props, "topicSource")
                        .or_else(|| string_prop(&row.props, "source"))
                        .unwrap_or_default(),
                    consumer_groups: string_array_prop(&row.props, "consumerGroups"),
                    producers,
                    consumers,
                    flows,
                    properties: Some(row.props.clone()),
                });
            }
        }
        out.sort_by(|a, b| {
            a.broker
                .cmp(&b.broker)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    // ── 仓库管理 ────────────────────────────────────────────────

    fn import_repo(&self, kind: &str, src: &str, name: Option<&str>) -> Result<String> {
        match kind {
            "git" => {
                let name = match name {
                    Some(n) => n.trim().to_string(),
                    None => derive_git_name(src)?,
                };
                let dest = self.prepare_checkout(&name)?;
                let url = src.to_string();
                let job_name = name.clone();
                let job_url = url.clone();
                let engine_dir = self.engine_dir();
                self.spawn_job(
                    name.clone(),
                    "git",
                    Some(url),
                    dest.clone(),
                    move |jobs, name| {
                        update_job(&jobs, &name, |job| {
                            job.set_stage(
                                "checkout",
                                format!("Cloning git repository into {}", dest.display()),
                                6.0,
                            );
                        });
                        clone_git_repo(&job_url, &dest)?;
                        update_job(&jobs, &name, |job| {
                            job.push_log(format!("checkout: clone complete {}", dest.display()));
                        });
                        let repo = user_facing_path(&dest.canonicalize()?);
                        run_analyze_job(&jobs, &name, repo.clone(), engine_dir)?;
                        update_job(&jobs, &name, |job| {
                            job.set_stage("register", "Saving repository metadata", 98.0);
                        });
                        finalize_entry(&repo, &job_name, "git", Some(job_url))?;
                        Ok(repo)
                    },
                );
                Ok(name)
            }
            "local" => {
                let requested = PathBuf::from(src)
                    .canonicalize()
                    .map_err(|e| anyhow!("invalid local path {src}: {e}"))?;
                let path =
                    user_facing_path(&discover_workspace_root(&requested)?.unwrap_or(requested));
                if !path.is_dir() {
                    bail!("invalid local path (not a directory): {src}");
                }
                let name = match name {
                    Some(n) => n.trim().to_string(),
                    None => path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "repo".into()),
                };
                validate_name(&name)?;
                self.guard_name_free(&name)?;
                if Registry::load()?.find(&path).is_some() {
                    bail!("path already registered: {}", path.display());
                }
                let job_name = name.clone();
                let job_path = path.clone();
                let engine_dir = self.engine_dir();
                self.spawn_job(name.clone(), "local", None, path, move |jobs, name| {
                    run_analyze_job(&jobs, &name, job_path.clone(), engine_dir)?;
                    update_job(&jobs, &name, |job| {
                        job.set_stage("register", "Saving repository metadata", 98.0);
                    });
                    finalize_entry(&job_path, &job_name, "local", None)?;
                    Ok(job_path)
                });
                Ok(name)
            }
            other => bail!("invalid import kind: {other:?} (expect \"git\" or \"local\")"),
        }
    }

    fn import_repo_zip(&self, name: &str, zip_path: &Path) -> Result<String> {
        let name = name.trim().to_string();
        let dest = self.prepare_checkout(&name)?;
        let zip = zip_path.to_path_buf();
        let job_name = name.clone();
        let engine_dir = self.engine_dir();
        self.spawn_job(
            name.clone(),
            "zip",
            None,
            dest.clone(),
            move |jobs, name| {
                let result = (|| {
                    update_job(&jobs, &name, |job| {
                        job.set_stage(
                            "extract",
                            format!("Extracting zip archive into {}", dest.display()),
                            8.0,
                        );
                    });
                    extract_zip(&zip, &dest)?;
                    update_job(&jobs, &name, |job| {
                        job.push_log(format!("extract: zip complete {}", dest.display()));
                    });
                    let repo = user_facing_path(&dest.canonicalize()?);
                    run_analyze_job(&jobs, &name, repo.clone(), engine_dir)?;
                    update_job(&jobs, &name, |job| {
                        job.set_stage("register", "Saving repository metadata", 98.0);
                    });
                    finalize_entry(&repo, &job_name, "zip", None)?;
                    Ok(repo)
                })();
                let _ = std::fs::remove_file(&zip); // 上传临时件用完即焚（含失败路径）
                result
            },
        );
        Ok(name)
    }

    fn update_repo(&self, name: &str) -> Result<String> {
        let registry = Registry::load()?;
        let Some(entry) = resolve_registered_repo_for_update(&registry, name)? else {
            bail!("repo not registered: {name}");
        };
        let name = entry.name.clone();
        {
            let jobs = self.jobs.lock().expect("jobs lock");
            if jobs.get(&name).is_some_and(|j| j.status == "indexing") {
                bail!("a job is already running for repo: {name}");
            }
        }
        match entry.source_kind.as_str() {
            "git" => {
                let dir = entry.repo_path.clone();
                let job_dir = dir.clone();
                let engine_dir = self.engine_dir();
                self.spawn_job(
                    name.clone(),
                    "git",
                    entry.source_url.clone(),
                    dir,
                    move |jobs, name| {
                        update_job(&jobs, &name, |job| {
                            job.set_stage(
                                "checkout",
                                format!("Fetching git updates in {}", job_dir.display()),
                                6.0,
                            );
                        });
                        fast_forward_git_repo(&job_dir)?;
                        update_job(&jobs, &name, |job| {
                            job.push_log(format!(
                                "checkout: git update complete {}",
                                job_dir.display()
                            ));
                        });
                        // register() 继承旧条目的 name/source/embeddings，无须再 finalize。
                        run_analyze_job(&jobs, &name, job_dir.clone(), engine_dir)?;
                        Ok(job_dir)
                    },
                );
                Ok(format!("update scheduled: {name} (git pull + analyze)"))
            }
            "zip" => bail!(
                "repo {name} was imported from a zip archive: upload a new zip via update-zip"
            ),
            _ => {
                let dir = entry.repo_path.clone();
                let job_dir = dir.clone();
                let engine_dir = self.engine_dir();
                self.spawn_job(name.clone(), "local", None, dir, move |jobs, name| {
                    run_analyze_job(&jobs, &name, job_dir.clone(), engine_dir)?;
                    update_job(&jobs, &name, |job| {
                        job.push_log(format!(
                            "runtime: local re-analyze complete {}",
                            job_dir.display()
                        ));
                    });
                    Ok(job_dir)
                });
                Ok(format!("update scheduled: {name} (re-analyze)"))
            }
        }
    }

    fn update_repo_zip(&self, name: &str, zip_path: &Path) -> Result<String> {
        let registry = Registry::load()?;
        let Some(entry) = registry.find_by_name(name).cloned() else {
            bail!("repo not registered: {name}");
        };
        if entry.source_kind != "zip" {
            bail!(
                "repo {name} source is {:?}: update-zip only applies to zip-imported repos",
                entry.source_kind
            );
        }
        {
            let jobs = self.jobs.lock().expect("jobs lock");
            if jobs.get(name).is_some_and(|j| j.status == "indexing") {
                bail!("a job is already running for repo: {name}");
            }
        }
        // 只清受管 checkout，绝不动用户路径。
        let checkouts = checkouts_dir();
        let checkouts = checkouts.canonicalize().unwrap_or(checkouts);
        if !entry.repo_path.starts_with(&checkouts) {
            bail!(
                "invalid state: zip repo path {} is outside managed checkouts",
                entry.repo_path.display()
            );
        }
        let dest = entry.repo_path.clone();
        let zip = zip_path.to_path_buf();
        let job_name = name.to_string();
        let engine_dir = self.engine_dir();
        self.spawn_job(
            name.to_string(),
            "zip",
            None,
            dest.clone(),
            move |jobs, name| {
                let result = (|| {
                    update_job(&jobs, &name, |job| {
                        job.set_stage(
                            "extract",
                            format!("Replacing zip checkout at {}", dest.display()),
                            6.0,
                        );
                    });
                    if dest.exists() {
                        update_job(&jobs, &name, |job| {
                            job.push_log(format!(
                                "extract: removing old checkout {}",
                                dest.display()
                            ));
                        });
                        std::fs::remove_dir_all(&dest)?;
                    }
                    extract_zip(&zip, &dest)?;
                    update_job(&jobs, &name, |job| {
                        job.push_log(format!(
                            "extract: zip replacement complete {}",
                            dest.display()
                        ));
                    });
                    let repo = user_facing_path(&dest.canonicalize()?);
                    run_analyze_job(&jobs, &name, repo.clone(), engine_dir)?;
                    update_job(&jobs, &name, |job| {
                        job.set_stage("register", "Saving repository metadata", 98.0);
                    });
                    finalize_entry(&repo, &job_name, "zip", None)?;
                    Ok(repo)
                })();
                let _ = std::fs::remove_file(&zip);
                result
            },
        );
        Ok(name.to_string())
    }

    fn remove_repo(&self, name: &str) -> Result<()> {
        let mut registry = Registry::load()?;
        let entry = registry.find_by_name(name).cloned();
        let checkouts = checkouts_dir();
        let checkouts = checkouts.canonicalize().unwrap_or(checkouts);

        let Some(entry) = entry else {
            // 没注册但有失败任务记录（导入失败的残留）→ 清任务态 + 残留 checkout。
            let removed = self.jobs.lock().expect("jobs lock").remove(name);
            if let Some(job) = removed {
                if job.status == "indexing" {
                    // 放回去，进行中的任务不能删。
                    self.jobs
                        .lock()
                        .expect("jobs lock")
                        .insert(name.to_string(), job);
                    bail!("invalid: repo {name} is still indexing, wait for it to finish");
                }
                let stale = checkouts.join(name);
                if stale.exists() {
                    let _ = std::fs::remove_dir_all(&stale);
                }
                return Ok(());
            }
            bail!("repo not registered: {name}");
        };

        if self
            .jobs
            .lock()
            .expect("jobs lock")
            .get(name)
            .is_some_and(|j| j.status == "indexing")
        {
            bail!("invalid: repo {name} is still indexing, wait for it to finish");
        }

        registry.remove_by_name(name);
        registry.save()?;
        let _ = std::fs::remove_dir_all(&entry.data_dir);
        // checkout 目录仅当真在受管目录下才删（绝不删用户自己的本地路径）。
        if entry.repo_path.starts_with(&checkouts) {
            let _ = std::fs::remove_dir_all(&entry.repo_path);
        }
        self.handles
            .lock()
            .expect("handles lock")
            .remove(&entry.repo_path);
        self.jobs.lock().expect("jobs lock").remove(name);
        Ok(())
    }

    fn set_repo_settings(&self, name: &str, settings: RepoSettingsUpdate) -> Result<()> {
        let mut registry = Registry::load()?;
        let Some(entry) = registry.find_by_name_mut(name) else {
            bail!("repo not registered: {name}");
        };
        entry.embeddings_enabled = settings.embeddings_enabled;
        // None = 恢复默认；Some 一律 clamp 到硬上限（HTTP 面已 clamp，这里兜底）。
        entry.render_max_nodes = settings.render_max_nodes.map(clamp_render_nodes);
        registry.save()?;
        Ok(())
    }

    fn read_source(
        &self,
        repo: &str,
        path: &str,
        start: Option<u32>,
        end: Option<u32>,
    ) -> Result<serde_json::Value> {
        let registry = Registry::load()?;
        let Some(entry) = registry
            .repos
            .iter()
            .find(|r| r.name == repo || r.repo_path.to_string_lossy() == repo)
        else {
            bail!("repo not registered: {repo}");
        };
        read_source_slice(&entry.repo_path, path, start, end)
    }

    fn node_detail(&self, repo: &str, id: &str) -> Result<serde_json::Value> {
        let handles = self.targets(Some(repo))?;
        let handle = handles.first().context("repo handle")?;
        let row = {
            let store = handle.store.lock().expect("store lock");
            store.node_by_id(id)?
        };
        let Some(row) = row else {
            bail!("node not found: {id}");
        };
        // 度数概要：callers/callees = 一跳 CALLS 反/正，refs = 全部入边。
        let (callers, callees, refs) = match handle.adj.index_of_id(id) {
            Some(node) => {
                let calls = handle.adj.type_id(aka_graph::CALLS_TYPE);
                let callers = handle
                    .adj
                    .in_edges(node)
                    .filter(|(_, ty)| Some(*ty) == calls)
                    .count();
                let callees = handle
                    .adj
                    .out_edges(node)
                    .filter(|(_, ty)| Some(*ty) == calls)
                    .count();
                let refs = handle.adj.in_edges(node).count();
                (callers, callees, refs)
            }
            None => (0, 0, 0),
        };
        let properties = if row.props.is_object() {
            row.props.clone()
        } else {
            serde_json::json!({})
        };
        let mut detail = serde_json::json!({
            "id": row.id,
            "name": row.name.clone().unwrap_or_default(),
            "label": row.label,
            "file": row.file_path.clone().unwrap_or_default(),
            "line": row.start_line.unwrap_or(0),
            "end_line": row.end_line.unwrap_or(0),
            "properties": properties,
            "degree": { "callers": callers, "callees": callees, "refs": refs },
        });
        // 流程视角：Process 合成节点展开成完整的 "process" 对象（端点 + 步骤），
        // 其他节点给 "processes" 归属数组——空数组也要给，前端据此判断渲染。
        let store = handle.store.lock().expect("store lock");
        if row.label == "Process" {
            // entry/terminal 从 props 的 entryPointId/terminalId 解析；
            // id 失配（工件不完整 / 节点被裁掉）→ null 而非报错。
            let endpoint = |key: &str| -> Result<serde_json::Value> {
                let Some(id) = row.props.get(key).and_then(|v| v.as_str()) else {
                    return Ok(serde_json::Value::Null);
                };
                Ok(match store.node_by_id(id)? {
                    Some(n) => serde_json::json!({
                        "id": n.id,
                        "name": n.name.unwrap_or_default(),
                        "label": n.label,
                        "file": n.file_path.unwrap_or_default(),
                        "line": n.start_line.unwrap_or(0),
                    }),
                    None => serde_json::Value::Null,
                })
            };
            let entry = endpoint("entryPointId")?;
            let terminal = endpoint("terminalId")?;
            // 步骤按 step 升序（process_steps 已排好），缺步号的排最后。
            let steps: Vec<serde_json::Value> = store
                .process_steps(row.rowid)?
                .into_iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "name": s.name,
                        "label": s.label,
                        "file": s.file_path.unwrap_or_default(),
                        "line": s.start_line.unwrap_or(0),
                        "step": s.step,
                    })
                })
                .collect();
            detail["process"] = serde_json::json!({
                "process_type": row.props.get("processType").and_then(|v| v.as_str()).unwrap_or(""),
                // stepCount 以 props 为准（裁剪后 steps 可能不全），缺失回退实际步数。
                "step_count": row.props.get("stepCount").and_then(|v| v.as_u64())
                    .unwrap_or(steps.len() as u64),
                "entry": entry,
                "terminal": terminal,
                "steps": steps,
            });
        } else {
            let processes: Vec<ProcessHit> = store
                .processes_of_node(row.rowid)?
                .into_iter()
                .map(|m| ProcessHit {
                    process_id: m.process_id,
                    name: m.name,
                    process_type: m.process_type,
                    step: m.step,
                    step_count: m.step_count,
                })
                .collect();
            detail["processes"] = serde_json::to_value(processes)?;
        }
        Ok(detail)
    }

    fn file_symbols(&self, repo: &str, path: &str) -> Result<serde_json::Value> {
        let handles = self.targets(Some(repo))?;
        let handle = handles.first().context("repo handle")?;
        let rows = {
            let store = handle.store.lock().expect("store lock");
            store.nodes_in_file(path)?
        };
        Ok(file_symbols_json(path, &rows))
    }

    fn list_files(&self, repo: &str) -> Result<Vec<aka_mcp::ops::FileEntry>> {
        let handles = self.targets(Some(repo))?;
        let handle = handles.first().context("repo handle")?;
        let rows = {
            let store = handle.store.lock().expect("store lock");
            store.file_list()?
        };
        Ok(rows
            .into_iter()
            .map(|(path, symbols)| aka_mcp::ops::FileEntry { path, symbols })
            .collect())
    }

    fn ego_graph(
        &self,
        repo: &str,
        id: &str,
        depth: u32,
        max_nodes: usize,
    ) -> Result<serde_json::Value> {
        let handles = self.targets(Some(repo))?;
        let handle = handles.first().context("repo handle")?;
        let store = handle.store.lock().expect("store lock");
        let ego = store.ego_graph(&handle.adj, id, depth, max_nodes)?;
        Ok(serde_json::to_value(ego)?)
    }

    fn repo_runtime_status(&self) -> HashMap<String, (String, Option<String>)> {
        self.jobs
            .lock()
            .expect("jobs lock")
            .iter()
            .map(|(name, job)| (name.clone(), (job.status.clone(), job.detail.clone())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex as TestMutex, OnceLock};

    struct EnvRestore {
        cwd: PathBuf,
        aka_home: Option<OsString>,
    }

    impl EnvRestore {
        fn capture() -> Self {
            Self {
                cwd: std::env::current_dir().expect("current dir"),
                aka_home: std::env::var_os("AKA_HOME"),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.cwd);
            if let Some(value) = &self.aka_home {
                std::env::set_var("AKA_HOME", value);
            } else {
                std::env::remove_var("AKA_HOME");
            }
        }
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<TestMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| TestMutex::new(()))
            .lock()
            .expect("test env lock")
    }

    /// 每个测试用独立临时仓库目录，互不串扰。
    fn temp_repo(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aka-source-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn node_row(id: &str, label: &str, name: &str, line: Option<u32>, end: Option<u32>) -> NodeRow {
        NodeRow {
            rowid: 0,
            id: id.to_string(),
            label: label.to_string(),
            name: Some(name.to_string()),
            file_path: Some("src/x.ts".to_string()),
            start_line: line,
            end_line: end,
            props: serde_json::json!({}),
        }
    }

    fn assert_job_visible_status(status: &str) {
        assert!(
            matches!(status, "indexing" | "failed"),
            "expected queued job to be visible as indexing or failed when the test host has no engine, got {status:?}"
        );
    }

    #[test]
    fn adapter_progress_tracks_synthesis_before_artifact_writes() {
        let synth_nodes = adapter_percent("aka-engine:export-artifacts:synthesize:nodes", 0, 0);
        let deps_start = adapter_percent(
            "aka-engine:export-artifacts:synthesize:dependency-edges",
            0,
            570,
        );
        let deps_mid = adapter_percent(
            "aka-engine:export-artifacts:synthesize:dependency-edges",
            285,
            570,
        );
        let processes = adapter_percent("aka-engine:export-artifacts:synthesize:processes", 0, 0);
        let write_nodes = adapter_percent("aka-engine:export-artifacts:nodes", 0, 12_687);

        assert!(deps_start > synth_nodes);
        assert!(deps_mid > deps_start);
        assert!(processes > deps_mid);
        assert!(write_nodes > processes);
    }

    #[test]
    fn completed_jobs_report_ready_at_one_hundred_percent() {
        let mut job = JobInfo::new("local", None, PathBuf::from("/tmp/repo"));
        job.set_stage("adapter", "Writing artifacts", 91.0);

        job.mark_done();

        assert_eq!(job.status, "ready");
        assert_eq!(job.progress.stage, "done");
        assert_eq!(job.progress.message, "Index ready");
        assert_eq!(job.progress.percent, 100.0);
        assert!(job.progress.current.is_none());
        assert!(job.progress.total.is_none());
    }

    #[test]
    fn index_progress_events_update_stage_counts_and_logs() {
        let mut job = JobInfo::new("local", None, PathBuf::from("/tmp/repo"));

        job.apply_engine_event(&EngineEvent::Phase {
            phase: "index:graph:edges".into(),
            current: 4,
            total: 8,
        });
        job.apply_engine_event(&EngineEvent::Log {
            stream: "index".into(),
            line: "graph:edges: ingesting 8 artifact edges".into(),
        });

        assert_eq!(job.progress.stage, "graph:edges");
        assert_eq!(job.progress.current, Some(4));
        assert_eq!(job.progress.total, Some(8));
        assert!(job.progress.percent >= 89.0);
        assert!(job
            .progress
            .logs
            .iter()
            .any(|line| line.contains("index: graph:edges (4/8)")));
        assert!(job
            .progress
            .logs
            .iter()
            .any(|line| line.contains("[index] graph:edges: ingesting 8 artifact edges")));
    }

    #[test]
    fn indexing_jobs_emit_heartbeat_when_events_go_quiet() {
        let mut job = JobInfo::new("local", None, PathBuf::from("/tmp/repo"));
        job.set_stage("engine", "Starting AST parser", 14.0);
        job.last_event_at = Instant::now() - JOB_HEARTBEAT_INTERVAL - Duration::from_millis(1);

        job.maybe_heartbeat();

        assert!(job
            .progress
            .logs
            .iter()
            .any(|line| line.contains("still working after")));
        assert!(job
            .progress
            .logs
            .iter()
            .any(|line| line.contains("engine: Starting AST parser")));
    }

    #[test]
    fn auto_index_debounces_until_quick_state_is_stable() {
        let base = RepoQuickState::default();
        let changed = RepoQuickState {
            files: BTreeMap::from([(
                "src/lib.rs".into(),
                QuickFingerprint {
                    size: 42,
                    modified: 100,
                },
            )]),
        };
        let mut state = AutoIndexState {
            quick: base,
            first_seen_dirty: None,
            last_dirty_quick: None,
        };
        let t0 = Instant::now();

        assert!(!auto_index_should_analyze(&mut state, changed.clone(), t0));
        assert!(!auto_index_should_analyze(
            &mut state,
            changed.clone(),
            t0 + Duration::from_secs(1)
        ));
        assert!(auto_index_should_analyze(
            &mut state,
            changed.clone(),
            t0 + AUTO_INDEX_DEBOUNCE + Duration::from_millis(1)
        ));
        assert_eq!(state.quick, changed);
        assert!(state.first_seen_dirty.is_none());
        assert!(state.last_dirty_quick.is_none());
    }

    #[test]
    fn quick_state_skips_generated_and_vcs_directories() {
        let repo = temp_repo("auto-skip");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir_all(repo.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(repo.join("target/debug")).unwrap();
        std::fs::create_dir_all(repo.join(".git/objects")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "pub fn keep() {}\n").unwrap();
        std::fs::write(repo.join("node_modules/pkg/index.js"), "ignored").unwrap();
        std::fs::write(repo.join("target/debug/app"), "ignored").unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ignored").unwrap();

        let state = RepoQuickState::compute(&repo).unwrap();

        assert!(state.files.contains_key("src/lib.rs"));
        assert!(!state.files.contains_key("node_modules/pkg/index.js"));
        assert!(!state.files.contains_key("target/debug/app"));
        assert!(!state.files.contains_key(".git/HEAD"));
    }

    #[test]
    fn discover_workspace_root_uses_git_toplevel() {
        let repo = temp_repo("workspace-git");
        std::fs::create_dir_all(repo.join("src/nested")).unwrap();
        std::fs::write(repo.join("src/nested/lib.rs"), "pub fn keep() {}\n").unwrap();
        let status = Command::new("git")
            .arg("init")
            .arg(&repo)
            .status()
            .expect("git init");
        assert!(status.success());

        let root = discover_workspace_root(&repo.join("src/nested"))
            .unwrap()
            .unwrap();

        assert_eq!(root, repo.canonicalize().unwrap());
    }

    #[test]
    fn discover_workspace_root_ignores_empty_non_git_dirs() {
        let repo = temp_repo("workspace-empty");

        assert!(discover_workspace_root(&repo).unwrap().is_none());
    }

    #[test]
    fn discover_workspace_root_accepts_marked_non_git_dirs() {
        let repo = temp_repo("workspace-marked");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(repo.join("src/app.py"), "def main(): pass\n").unwrap();

        let root = discover_workspace_root(&repo).unwrap().unwrap();

        assert_eq!(root, repo.canonicalize().unwrap());
    }

    #[test]
    fn discover_workspace_root_walks_up_to_non_git_marker() {
        let repo = temp_repo("workspace-marked-nested");
        std::fs::create_dir_all(repo.join("src/app/api")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(
            repo.join("src/app/api/views.py"),
            "def list_orders(): pass\n",
        )
        .unwrap();

        let root = discover_workspace_root(&repo.join("src/app/api"))
            .unwrap()
            .unwrap();

        assert_eq!(root, repo.canonicalize().unwrap());
    }

    #[test]
    fn unique_local_name_uses_path_hash_on_name_collision() {
        let repo = temp_repo("workspace-name");
        let other = temp_repo("other").join("workspace-name");
        std::fs::create_dir_all(&other).unwrap();
        let existing_name = derive_local_name(&repo);
        let mut registry = Registry::default();
        registry.upsert(RepoEntry {
            name: existing_name.clone(),
            repo_path: other,
            data_dir: "/tmp/data".into(),
            indexed_at: None,
            engine_sha: None,
            stats: ArtifactStats::default(),
            embeddings_enabled: false,
            source_kind: "local".into(),
            source_url: None,
            render_max_nodes: None,
        });

        let repo = repo.canonicalize().unwrap();
        let name = unique_local_name(&registry, &repo);

        assert_ne!(name, existing_name);
        assert_eq!(name, repo_dir_name(&repo));
    }

    #[test]
    fn list_repos_auto_queues_workspace_only_when_enabled() {
        let _guard = env_lock();
        let _restore = EnvRestore::capture();
        let repo = temp_repo("workspace-auto-list");
        let home = temp_repo("workspace-auto-home");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(repo.join("src/app.py"), "def main(): pass\n").unwrap();
        std::env::set_var("AKA_HOME", &home);
        std::env::set_current_dir(&repo).unwrap();

        let plain = AkaBackend::new();
        assert!(plain.list_repos().unwrap().is_empty());

        let auto = AkaBackend::new().with_workspace_auto_index();
        let repos = auto.list_repos().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, derive_local_name(&repo));
        assert_job_visible_status(&repos[0].status);
        assert!(repos[0].progress.is_some());
    }

    #[test]
    fn engine_dir_backend_can_auto_queue_current_workspace() {
        let _guard = env_lock();
        let _restore = EnvRestore::capture();
        let repo = temp_repo("workspace-auto-engine-dir");
        let home = temp_repo("workspace-auto-engine-dir-home");
        let engine_dir = temp_repo("workspace-auto-engine-dir-bin");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(repo.join("src/app.py"), "def main(): pass\n").unwrap();
        std::env::set_var("AKA_HOME", &home);
        std::env::set_current_dir(&repo).unwrap();

        let backend = AkaBackend::with_engine_dir(engine_dir).with_workspace_auto_index();
        let repos = backend.list_repos().unwrap();

        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, derive_local_name(&repo));
        assert_job_visible_status(&repos[0].status);
        assert!(repos[0].progress.is_some());
    }

    #[test]
    fn analyze_queues_unregistered_local_repo_for_mcp_visibility() {
        let _guard = env_lock();
        let _restore = EnvRestore::capture();
        let repo = temp_repo("workspace-analyze-local");
        let home = temp_repo("workspace-analyze-home");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(repo.join("src/app.py"), "def main(): pass\n").unwrap();
        std::env::set_var("AKA_HOME", &home);

        let backend = AkaBackend::new();
        let summary = backend.analyze(repo.to_str().unwrap()).unwrap();

        assert!(summary.starts_with("indexing scheduled: "));
        let repos = backend.list_repos().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, derive_local_name(&repo));
        assert_job_visible_status(&repos[0].status);
    }

    #[test]
    fn analyze_lifts_nested_path_to_workspace_root() {
        let _guard = env_lock();
        let _restore = EnvRestore::capture();
        let repo = temp_repo("workspace-analyze-nested");
        let home = temp_repo("workspace-analyze-nested-home");
        std::fs::create_dir_all(repo.join("src/app/api")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(
            repo.join("src/app/api/views.py"),
            "def list_orders(): pass\n",
        )
        .unwrap();
        std::env::set_var("AKA_HOME", &home);

        let backend = AkaBackend::new();
        let summary = backend
            .analyze(repo.join("src/app/api").to_str().unwrap())
            .unwrap();

        assert!(summary.starts_with("indexing scheduled: "));
        let repos = backend.list_repos().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, derive_local_name(&repo));
        assert_eq!(
            repos[0].path,
            repo.canonicalize().unwrap().to_string_lossy()
        );
        assert_job_visible_status(&repos[0].status);
    }

    #[test]
    fn import_local_lifts_nested_path_to_workspace_root() {
        let _guard = env_lock();
        let _restore = EnvRestore::capture();
        let repo = temp_repo("workspace-import-local-nested");
        let home = temp_repo("workspace-import-local-nested-home");
        std::fs::create_dir_all(repo.join("src/app/api")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(
            repo.join("src/app/api/views.py"),
            "def list_orders(): pass\n",
        )
        .unwrap();
        std::env::set_var("AKA_HOME", &home);

        let backend = AkaBackend::new();
        let name = backend
            .import_repo("local", repo.join("src/app/api").to_str().unwrap(), None)
            .unwrap();

        assert_eq!(name, derive_local_name(&repo));
        let repos = backend.list_repos().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, derive_local_name(&repo));
        assert_eq!(
            repos[0].path,
            repo.canonicalize().unwrap().to_string_lossy()
        );
        assert_job_visible_status(&repos[0].status);
    }

    #[test]
    fn update_repo_accepts_registered_workspace_path() {
        let _guard = env_lock();
        let _restore = EnvRestore::capture();
        let repo = temp_repo("workspace-update-path");
        let home = temp_repo("workspace-update-path-home");
        std::fs::create_dir_all(repo.join("src/app/api")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(
            repo.join("src/app/api/views.py"),
            "def list_orders(): pass\n",
        )
        .unwrap();
        std::env::set_var("AKA_HOME", &home);
        let path = repo.canonicalize().unwrap();
        let name = derive_local_name(&repo);
        let mut registry = Registry::load().unwrap();
        registry.upsert(RepoEntry {
            name: name.clone(),
            repo_path: path.clone(),
            data_dir: aka_home().join("repos").join("workspace-update-path"),
            indexed_at: Some(1),
            engine_sha: None,
            stats: ArtifactStats::default(),
            embeddings_enabled: false,
            source_kind: "local".into(),
            source_url: None,
            render_max_nodes: None,
        });
        registry.save().unwrap();

        let backend = AkaBackend::new();
        let summary = backend
            .update_repo(repo.join("src/app/api").to_str().unwrap())
            .unwrap();

        assert_eq!(summary, format!("update scheduled: {name} (re-analyze)"));
        let repos = backend.list_repos().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, name);
        assert_job_visible_status(&repos[0].status);
    }

    #[test]
    fn explicit_repo_path_queries_auto_queue_workspace_root() {
        let _guard = env_lock();
        let _restore = EnvRestore::capture();
        let repo = temp_repo("workspace-query-path");
        let home = temp_repo("workspace-query-path-home");
        std::fs::create_dir_all(repo.join("src/app/api")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(
            repo.join("src/app/api/views.py"),
            "def list_orders(): pass\n",
        )
        .unwrap();
        std::env::set_var("AKA_HOME", &home);

        let backend = AkaBackend::new();
        let nested = repo.join("src/app/api");
        let err = backend
            .search(Some(nested.to_str().unwrap()), "orders", 5)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("自动索引中") || err.contains("仍在索引中") || err.contains("索引失败"),
            "{err}"
        );
        let repos = backend.list_repos().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, derive_local_name(&repo));
        assert_eq!(
            repos[0].path,
            repo.canonicalize().unwrap().to_string_lossy()
        );
        assert_job_visible_status(&repos[0].status);
    }

    #[test]
    fn explicit_unknown_repo_name_stays_a_registry_error() {
        let _guard = env_lock();
        let _restore = EnvRestore::capture();
        let home = temp_repo("workspace-query-name-home");
        std::env::set_var("AKA_HOME", &home);

        let backend = AkaBackend::new();
        let err = backend
            .search(Some("missing-service"), "orders", 5)
            .unwrap_err()
            .to_string();

        assert!(err.contains("未注册的仓库: missing-service"), "{err}");
        assert!(
            !err.contains("resolve workspace path"),
            "plain repo names must not be treated as filesystem paths: {err}"
        );
        assert!(backend.list_repos().unwrap().is_empty());
    }

    #[test]
    fn queue_workspaces_skips_stale_roots_and_indexes_valid_ones() {
        let _guard = env_lock();
        let _restore = EnvRestore::capture();
        let repo = temp_repo("workspace-roots-stale-valid");
        let home = temp_repo("workspace-roots-stale-valid-home");
        std::fs::create_dir_all(repo.join("src/app/api")).unwrap();
        std::fs::write(repo.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        std::fs::write(
            repo.join("src/app/api/views.py"),
            "def list_orders(): pass\n",
        )
        .unwrap();
        std::env::set_var("AKA_HOME", &home);

        let backend = AkaBackend::new();
        let stale = home.join("missing-workspace");
        let queued = backend
            .queue_workspaces(&[stale, repo.join("src/app/api")])
            .unwrap();

        assert_eq!(queued, vec![derive_local_name(&repo)]);
        let repos = backend.list_repos().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, derive_local_name(&repo));
        assert_eq!(
            repos[0].path,
            repo.canonicalize().unwrap().to_string_lossy()
        );
        assert_job_visible_status(&repos[0].status);
    }

    #[test]
    fn file_symbols_contract_shape_and_filtering() {
        // nodes_in_file 已按 start_line 升序返回（NULL 排最前）。
        let rows = vec![
            node_row("File:src/x.ts", "File", "x.ts", None, None), // 无行号 → 滤掉
            node_row("Fn:src/x.ts:alpha", "Function", "alpha", Some(3), Some(9)),
            node_row("Class:src/x.ts:Beta", "Class", "Beta", Some(12), None), // end 缺省 → 回退 line
            node_row("Fn:src/x.ts:gamma", "Function", "gamma", Some(50), Some(54)),
        ];
        let v = file_symbols_json("src/x.ts", &rows);
        assert_eq!(v["path"], "src/x.ts");
        let symbols = v["symbols"].as_array().unwrap();
        assert_eq!(symbols.len(), 3, "File 节点（无行号）必须被滤掉");
        // line 升序 + 合同字段一字不差。
        let lines: Vec<u64> = symbols
            .iter()
            .map(|s| s["line"].as_u64().unwrap())
            .collect();
        assert_eq!(lines, [3, 12, 50]);
        assert_eq!(symbols[0]["id"], "Fn:src/x.ts:alpha");
        assert_eq!(symbols[0]["name"], "alpha");
        assert_eq!(symbols[0]["label"], "Function");
        assert_eq!(symbols[0]["file"], "src/x.ts");
        assert_eq!(symbols[0]["end_line"], 9);
        assert_eq!(symbols[1]["end_line"], 12, "end_line 缺失时回退 start_line");

        // 文件没有符号（或只剩无行号节点）→ 空数组而非缺字段。
        let v = file_symbols_json("src/empty.ts", &[]);
        assert_eq!(v["path"], "src/empty.ts");
        assert!(v["symbols"].as_array().unwrap().is_empty());
    }

    #[test]
    fn source_slice_basic_and_clamp() {
        let repo = temp_repo("basic");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
        std::fs::write(repo.join("src/a.ts"), body).unwrap();

        // 整文件（缺省 start/end）。
        let v = read_source_slice(&repo, "src/a.ts", None, None).unwrap();
        assert_eq!(v["path"], "src/a.ts");
        assert_eq!(v["total_lines"], 10);
        assert_eq!(v["start"], 1);
        assert_eq!(v["end"], 10);
        assert_eq!(v["lines"].as_array().unwrap().len(), 10);
        assert_eq!(v["lines"][0], "line1");
        assert_eq!(v["truncated"], false);
        assert!(v["abs_path"].as_str().unwrap().ends_with("src/a.ts"));

        // 1-based 含端切片。
        let v = read_source_slice(&repo, "src/a.ts", Some(3), Some(5)).unwrap();
        assert_eq!(v["start"], 3);
        assert_eq!(v["end"], 5);
        let lines: Vec<&str> = v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap())
            .collect();
        assert_eq!(lines, ["line3", "line4", "line5"]);

        // 越界自动 clamp：start=0 → 1；end=999 → 10；start>total → 最后一行。
        let v = read_source_slice(&repo, "src/a.ts", Some(0), Some(999)).unwrap();
        assert_eq!(v["start"], 1);
        assert_eq!(v["end"], 10);
        let v = read_source_slice(&repo, "src/a.ts", Some(42), None).unwrap();
        assert_eq!(v["start"], 10);
        assert_eq!(v["end"], 10);
        assert_eq!(v["lines"][0], "line10");
    }

    #[test]
    fn code_search_matches_identifier_terms() {
        assert!(line_matches_literal_or_terms(
            "class OrderService { void reserveInventory() {} }",
            "order service"
        ));
        assert!(line_matches_literal_or_terms(
            "def parse_config(path): return load_yaml(path)",
            "parse config"
        ));
        assert!(line_matches_literal_or_terms(
            "class Http2RouteConsumer {}",
            "http 2 route"
        ));
        assert!(!line_matches_literal_or_terms(
            "class OrderRepository {}",
            "order service"
        ));
    }

    #[test]
    fn source_slice_truncates_at_2000_lines() {
        let repo = temp_repo("trunc");
        let body: String = (1..=2300).map(|i| format!("l{i}\n")).collect();
        std::fs::write(repo.join("big.txt"), body).unwrap();

        let v = read_source_slice(&repo, "big.txt", None, None).unwrap();
        assert_eq!(v["total_lines"], 2300);
        assert_eq!(v["start"], 1);
        assert_eq!(v["end"], 2000);
        assert_eq!(v["lines"].as_array().unwrap().len(), 2000);
        assert_eq!(v["truncated"], true);

        // 显式范围 ≤ 2000 行不截断。
        let v = read_source_slice(&repo, "big.txt", Some(100), Some(2099)).unwrap();
        assert_eq!(v["truncated"], false);
        assert_eq!(v["lines"].as_array().unwrap().len(), 2000);
    }

    #[test]
    fn source_slice_rejects_traversal_and_absolute() {
        let repo = temp_repo("traverse");
        std::fs::write(repo.join("ok.txt"), "hi\n").unwrap();
        // 仓库外目标文件（../ 穿越能拿到的位置）。
        std::fs::write(
            repo.parent().unwrap().join("aka-source-test-outside.txt"),
            "secret",
        )
        .unwrap();

        let err = read_source_slice(&repo, "../aka-source-test-outside.txt", None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid path"), "应拒绝 ../ 穿越: {err}");

        let err = read_source_slice(&repo, "/etc/hosts", None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid path"), "应拒绝绝对路径: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn source_slice_rejects_symlink_escape() {
        let repo = temp_repo("symlink");
        let outside = std::env::temp_dir().join(format!(
            "aka-source-test-symlink-target-{}.txt",
            std::process::id()
        ));
        std::fs::write(&outside, "secret\n").unwrap();
        std::os::unix::fs::symlink(&outside, repo.join("sneaky.txt")).unwrap();

        let err = read_source_slice(&repo, "sneaky.txt", None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid path"), "软链接逃逸必须被挡: {err}");
    }

    #[test]
    fn source_slice_missing_file_and_binary() {
        let repo = temp_repo("misc");
        let err = read_source_slice(&repo, "nope.rs", None, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("file not found"),
            "缺文件要 not found 语义: {err}"
        );

        std::fs::write(repo.join("bin.dat"), b"abc\0def").unwrap();
        let err = read_source_slice(&repo, "bin.dat", None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid file"), "二进制要 invalid 语义: {err}");

        std::fs::write(repo.join("bad.txt"), [0xFFu8, 0xFE, 0x41]).unwrap();
        let err = read_source_slice(&repo, "bad.txt", None, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("invalid file"),
            "非 UTF-8 要 invalid 语义: {err}"
        );

        // 空文件：total 0 / start 1 / end 0 / lines []。
        std::fs::write(repo.join("empty.txt"), "").unwrap();
        let v = read_source_slice(&repo, "empty.txt", None, None).unwrap();
        assert_eq!(v["total_lines"], 0);
        assert_eq!(v["start"], 1);
        assert_eq!(v["end"], 0);
        assert!(v["lines"].as_array().unwrap().is_empty());
        assert_eq!(v["truncated"], false);
    }
}
