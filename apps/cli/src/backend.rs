//! 真实 Backend：aka_mcp::Backend over 注册表 + 图存储 + 搜索索引。
//!
//! 每个仓库的句柄（图库 + 邻接 + 搜索）首次使用时打开并缓存。
//! `GraphStore`（rusqlite）非 Sync，包 Mutex；Adjacency / SearchIndex 只读共享。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};

use aka_core::{Registry, RepoEntry, RepoPaths};
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

pub struct AkaBackend {
    handles: Mutex<HashMap<PathBuf, Arc<RepoHandle>>>,
}

impl AkaBackend {
    pub fn new() -> Self {
        Self {
            handles: Mutex::new(HashMap::new()),
        }
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
        Ok(registry
            .repos
            .iter()
            .map(|r| RepoInfo {
                name: r.name.clone(),
                path: r.repo_path.to_string_lossy().to_string(),
                nodes: r.stats.nodes,
                edges: r.stats.edges,
                indexed_at: r.indexed_at,
                embeddings_enabled: r.embeddings_enabled,
            })
            .collect())
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
}
