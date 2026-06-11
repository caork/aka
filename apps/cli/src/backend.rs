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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};

use aka_core::{
    aka_home, clamp_render_nodes, Registry, RepoEntry, RepoPaths, DEFAULT_RENDER_MAX_NODES,
};
use aka_graph::{Adjacency, GraphStore, NodeRow};
use aka_mcp::{Backend, ProcessHit, RepoInfo, RepoSettingsUpdate, SearchHit, SymbolRef};
use aka_search::SearchIndex;

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
];

pub struct RepoHandle {
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
        let _ = &entry;
        Ok(Self {
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

/// 后台任务状态（import / update）。不在 map 里 = ready。
#[derive(Debug, Clone)]
struct JobInfo {
    /// `indexing` 或 `failed`。
    status: String,
    /// 失败原因（status = failed 时携带）。
    detail: Option<String>,
    /// 来源种类（合成 list 条目用）：local / git / zip。
    kind: String,
    /// git 来源 URL。
    url: Option<String>,
    /// 任务针对的仓库路径（合成 list 条目展示用）。
    path: PathBuf,
}

pub struct AkaBackend {
    handles: Arc<Mutex<HashMap<PathBuf, Arc<RepoHandle>>>>,
    jobs: Arc<Mutex<HashMap<String, JobInfo>>>,
}

/// git / zip 导入的受管 checkout 根目录。
fn checkouts_dir() -> PathBuf {
    aka_home().join("checkouts")
}

/// 仓库名用于拼 checkout 路径，必须是单段目录名。
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
    {
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

/// shell out 系统 git；失败带 stderr 尾部。
fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = cmd
        .output()
        .with_context(|| format!("spawn git {}", args.join(" ")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(8).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        bail!(
            "git {} failed ({}): {}",
            args.first().unwrap_or(&"?"),
            out.status,
            tail.join(" | ")
        );
    }
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
        if rel.components().next().is_some_and(|c| c.as_os_str() == "__MACOSX") {
            continue;
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut f = std::fs::File::create(&out)
                .with_context(|| format!("write {}", out.display()))?;
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
    let lines: Vec<&str> = if total == 0 { Vec::new() } else { all[s - 1..e].to_vec() };

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

/// analyze 落注册表后补 name / source 字段（register() 只继承已有条目，
/// 新导入的 git/zip 仓库要在这里盖上来源）。
fn finalize_entry(repo_path: &Path, name: &str, kind: &str, url: Option<String>) -> Result<()> {
    let mut registry = Registry::load()?;
    if let Some(entry) = registry.repos.iter_mut().find(|r| r.repo_path == repo_path) {
        entry.name = name.to_string();
        entry.source_kind = kind.to_string();
        entry.source_url = url;
        registry.save()?;
    }
    Ok(())
}

impl AkaBackend {
    pub fn new() -> Self {
        Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
            jobs: Arc::new(Mutex::new(HashMap::new())),
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
        work: impl FnOnce() -> Result<PathBuf> + Send + 'static,
    ) {
        let jobs = Arc::clone(&self.jobs);
        let handles = Arc::clone(&self.handles);
        jobs.lock().expect("jobs lock").insert(
            name.clone(),
            JobInfo {
                status: "indexing".into(),
                detail: None,
                kind: kind.to_string(),
                url,
                path,
            },
        );
        std::thread::spawn(move || match work() {
            Ok(repo_path) => {
                handles.lock().expect("handles lock").remove(&repo_path);
                jobs.lock().expect("jobs lock").remove(&name);
            }
            Err(e) => {
                if let Some(job) = jobs.lock().expect("jobs lock").get_mut(&name) {
                    job.status = "failed".into();
                    job.detail = Some(format!("{e:#}"));
                }
            }
        });
    }

    /// 解析 repo 参数（名字或路径；None = 全部已索引仓库）。
    fn targets(&self, repo: Option<&str>) -> Result<Vec<Arc<RepoHandle>>> {
        let registry = Registry::load()?;
        let entries: Vec<RepoEntry> = match repo {
            Some(key) => {
                let found = registry
                    .repos
                    .iter()
                    .find(|r| r.name == key || r.repo_path.to_string_lossy() == key)
                    .cloned();
                match found {
                    Some(e) => vec![e],
                    None => bail!("未注册的仓库: {key}（aka repos 查看）"),
                }
            }
            None => registry.repos.clone(),
        };
        if entries.is_empty() {
            bail!("没有已注册的仓库——先 `aka analyze <path>`");
        }

        let mut cache = self.handles.lock().expect("handles lock");
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let key = entry.repo_path.clone();
            if let Some(h) = cache.get(&key) {
                out.push(Arc::clone(h));
                continue;
            }
            let handle = Arc::new(RepoHandle::open(entry)?);
            cache.insert(key, Arc::clone(&handle));
            out.push(handle);
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
}

impl Default for AkaBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for AkaBackend {
    fn list_repos(&self) -> Result<Vec<RepoInfo>> {
        let registry = Registry::load()?;
        let jobs = self.jobs.lock().expect("jobs lock").clone();
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

    fn callers(&self, repo: Option<&str>, symbol: &str, depth: u32) -> Result<Vec<SymbolRef>> {
        self.traverse(repo, symbol, "CALLS", |h, n| {
            h.adj.callers(n, depth.max(1), 200)
        })
    }

    fn callees(&self, repo: Option<&str>, symbol: &str, depth: u32) -> Result<Vec<SymbolRef>> {
        self.traverse(repo, symbol, "CALLS", |h, n| {
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

    fn analyze(&self, repo_path: &str) -> Result<String> {
        let summary = crate::run_analyze(PathBuf::from(repo_path), None, false)
            .map_err(|e| anyhow!("{e:#}"))?;
        // 旧句柄作废（重新索引后必须重开）。
        self.handles
            .lock()
            .expect("handles lock")
            .remove(&PathBuf::from(repo_path));
        Ok(summary)
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
                self.spawn_job(name.clone(), "git", Some(url), dest.clone(), move || {
                    run_git(
                        &["clone", &job_url, &dest.to_string_lossy()],
                        None,
                    )?;
                    let repo = dest.canonicalize()?;
                    crate::run_analyze(repo.clone(), None, false)
                        .map_err(|e| anyhow!("{e:#}"))?;
                    finalize_entry(&repo, &job_name, "git", Some(job_url))?;
                    Ok(repo)
                });
                Ok(name)
            }
            "local" => {
                let path = PathBuf::from(src)
                    .canonicalize()
                    .map_err(|e| anyhow!("invalid local path {src}: {e}"))?;
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
                self.spawn_job(name.clone(), "local", None, path, move || {
                    crate::run_analyze(job_path.clone(), None, false)
                        .map_err(|e| anyhow!("{e:#}"))?;
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
        self.spawn_job(name.clone(), "zip", None, dest.clone(), move || {
            let result = (|| {
                extract_zip(&zip, &dest)?;
                let repo = dest.canonicalize()?;
                crate::run_analyze(repo.clone(), None, false)
                    .map_err(|e| anyhow!("{e:#}"))?;
                finalize_entry(&repo, &job_name, "zip", None)?;
                Ok(repo)
            })();
            let _ = std::fs::remove_file(&zip); // 上传临时件用完即焚（含失败路径）
            result
        });
        Ok(name)
    }

    fn update_repo(&self, name: &str) -> Result<String> {
        let registry = Registry::load()?;
        let Some(entry) = registry.find_by_name(name).cloned() else {
            bail!("repo not registered: {name}");
        };
        {
            let jobs = self.jobs.lock().expect("jobs lock");
            if jobs.get(name).is_some_and(|j| j.status == "indexing") {
                bail!("a job is already running for repo: {name}");
            }
        }
        match entry.source_kind.as_str() {
            "git" => {
                let dir = entry.repo_path.clone();
                let job_dir = dir.clone();
                self.spawn_job(
                    name.to_string(),
                    "git",
                    entry.source_url.clone(),
                    dir,
                    move || {
                        run_git(&["pull", "--ff-only"], Some(&job_dir))?;
                        // register() 继承旧条目的 name/source/embeddings，无须再 finalize。
                        crate::run_analyze(job_dir.clone(), None, false)
                            .map_err(|e| anyhow!("{e:#}"))?;
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
                self.spawn_job(name.to_string(), "local", None, dir, move || {
                    crate::run_analyze(job_dir.clone(), None, false)
                        .map_err(|e| anyhow!("{e:#}"))?;
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
        self.spawn_job(name.to_string(), "zip", None, dest.clone(), move || {
            let result = (|| {
                if dest.exists() {
                    std::fs::remove_dir_all(&dest)?;
                }
                extract_zip(&zip, &dest)?;
                let repo = dest.canonicalize()?;
                crate::run_analyze(repo.clone(), None, false)
                    .map_err(|e| anyhow!("{e:#}"))?;
                finalize_entry(&repo, &job_name, "zip", None)?;
                Ok(repo)
            })();
            let _ = std::fs::remove_file(&zip);
            result
        });
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
                    self.jobs.lock().expect("jobs lock").insert(name.to_string(), job);
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

    /// 每个测试用独立临时仓库目录，互不串扰。
    fn temp_repo(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aka-source-test-{tag}-{}", std::process::id()));
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
        let lines: Vec<u64> = symbols.iter().map(|s| s["line"].as_u64().unwrap()).collect();
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
        let lines: Vec<&str> = v["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
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
        std::fs::write(repo.parent().unwrap().join("aka-source-test-outside.txt"), "secret").unwrap();

        let err = read_source_slice(&repo, "../aka-source-test-outside.txt", None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid path"), "应拒绝 ../ 穿越: {err}");

        let err = read_source_slice(&repo, "/etc/hosts", None, None).unwrap_err().to_string();
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

        let err = read_source_slice(&repo, "sneaky.txt", None, None).unwrap_err().to_string();
        assert!(err.contains("invalid path"), "软链接逃逸必须被挡: {err}");
    }

    #[test]
    fn source_slice_missing_file_and_binary() {
        let repo = temp_repo("misc");
        let err = read_source_slice(&repo, "nope.rs", None, None).unwrap_err().to_string();
        assert!(err.contains("file not found"), "缺文件要 not found 语义: {err}");

        std::fs::write(repo.join("bin.dat"), b"abc\0def").unwrap();
        let err = read_source_slice(&repo, "bin.dat", None, None).unwrap_err().to_string();
        assert!(err.contains("invalid file"), "二进制要 invalid 语义: {err}");

        std::fs::write(repo.join("bad.txt"), [0xFFu8, 0xFE, 0x41]).unwrap();
        let err = read_source_slice(&repo, "bad.txt", None, None).unwrap_err().to_string();
        assert!(err.contains("invalid file"), "非 UTF-8 要 invalid 语义: {err}");

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
