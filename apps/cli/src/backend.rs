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

use aka_core::{aka_home, Registry, RepoEntry, RepoPaths};
use aka_graph::{Adjacency, GraphStore, NodeRow};
use aka_mcp::{Backend, RepoInfo, SearchHit, SymbolRef};
use aka_search::SearchIndex;

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

    fn graph_lod(&self, repo: &str, max_nodes: usize) -> Result<serde_json::Value> {
        let handles = self.targets(Some(repo))?;
        let handle = handles.first().context("repo handle")?;
        let store = handle.store.lock().expect("store lock");
        let lod = store.lod_snapshot(max_nodes)?;
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

    fn set_repo_settings(&self, name: &str, embeddings_enabled: bool) -> Result<()> {
        let mut registry = Registry::load()?;
        let Some(entry) = registry.find_by_name_mut(name) else {
            bail!("repo not registered: {name}");
        };
        entry.embeddings_enabled = embeddings_enabled;
        registry.save()?;
        Ok(())
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
        Ok(serde_json::json!({
            "id": row.id,
            "name": row.name.clone().unwrap_or_default(),
            "label": row.label,
            "file": row.file_path.clone().unwrap_or_default(),
            "line": row.start_line.unwrap_or(0),
            "end_line": row.end_line.unwrap_or(0),
            "properties": properties,
            "degree": { "callers": callers, "callees": callees, "refs": refs },
        }))
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
