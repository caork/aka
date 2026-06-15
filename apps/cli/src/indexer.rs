//! 索引管线：NDJSON 工件 → 图存储（SQLite + 布局） + 搜索索引（tantivy）。

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Context, Result};

use aka_core::{ArtifactDir, ChunkRec, EdgeRec, IndexDelta, IndexState, NodeRec, RepoPaths};
use aka_graph::{compute_layout, Adjacency, GraphStore};
use aka_search::SearchIndexWriter;

const MAX_INCREMENTAL_CHANGED_FILES: usize = 64;
const INCREMENTAL_RATIO_DIVISOR: usize = 5;

pub struct IndexSummary {
    pub nodes: u64,
    pub edges: u64,
    pub dangling_edges: u64,
    pub chunks: u64,
    pub bad_lines: u64,
    pub incremental: bool,
}

pub enum IncrementalIndexOutcome {
    Applied(IndexSummary),
    FullRebuildRequired(String),
}

struct IncrementalSlice {
    changed_paths: BTreeSet<String>,
    nodes: Vec<NodeRec>,
    edges: Vec<EdgeRec>,
    chunks: Vec<ChunkRec>,
    bad_lines: u64,
}

#[derive(Clone)]
struct NodeInfo {
    label: String,
    file_path: Option<String>,
}

/// 从工件目录全量重建图与搜索索引（幂等：先清旧再建新）。
pub fn index_artifact(artifact: &ArtifactDir, paths: &RepoPaths) -> Result<IndexSummary> {
    let mut bad_lines = 0u64;

    // ── 图存储 ───────────────────────────────────────────────
    let db_path = paths.graph_db();
    remove_if_exists(&db_path)?;
    let mut store = GraphStore::create(&db_path)
        .with_context(|| format!("创建图库失败 {}", db_path.display()))?;

    let nodes = artifact.nodes()?.filter_map(|r| match r {
        Ok(n) => Some(n),
        Err(_) => {
            bad_lines += 1;
            None
        }
    });
    // 借用检查：nodes/edges 两个迭代器都要捕获 bad_lines，分两次摄取。
    store.ingest(nodes, std::iter::empty())?;
    let mut bad_edge_lines = 0u64;
    let edges = artifact.edges()?.filter_map(|r| match r {
        Ok(e) => Some(e),
        Err(_) => {
            bad_edge_lines += 1;
            None
        }
    });
    let stats_edges = store.ingest(std::iter::empty(), edges)?;
    bad_lines += bad_edge_lines;

    // 布局（确定性 phyllotaxis，给可视化用）。
    let adj = Adjacency::build(&store)?;
    compute_layout(&store, &adj)?;

    // ── 搜索索引 ─────────────────────────────────────────────
    let search_dir = paths.search_dir();
    if search_dir.exists() {
        std::fs::remove_dir_all(&search_dir)?;
    }
    std::fs::create_dir_all(&search_dir)?;
    // 写句柄持 tantivy 目录写锁，限定作用域：commit 后立即 drop 释放，
    // 不阻塞其他进程（serve / mcp）的只读查询打开。
    let chunk_count = {
        let mut search = SearchIndexWriter::create(&search_dir)?;
        // 节点先于 chunk 摄取：chunk 文档要携带所属节点的真实 label。
        search.add_nodes(artifact.nodes()?.filter_map(|r| r.ok()))?;
        let mut chunk_count = 0u64;
        if let Some(chunks) = artifact.chunks()? {
            search.add_chunks(chunks.filter_map(|r| r.ok()).inspect(|_| chunk_count += 1))?;
        }
        search.commit()?;
        chunk_count
    };

    Ok(IndexSummary {
        nodes: store.node_count()?,
        edges: store.edge_count()?,
        dangling_edges: stats_edges.dangling_edges,
        chunks: chunk_count,
        bad_lines,
        incremental: false,
    })
}

/// File-scoped incremental replacement over an existing graph/search index.
///
/// The engine still emits a full artifact. This function conservatively slices
/// that artifact down to added/modified files, deletes old rows for those files
/// from the existing indexes, and appends replacement rows. If any condition
/// could make file-scoped replacement unsafe, it reports `FullRebuildRequired`
/// and leaves the existing indexes untouched.
pub fn index_artifact_incremental(
    artifact: &ArtifactDir,
    paths: &RepoPaths,
    delta: &IndexDelta,
    previous_state: &IndexState,
    current_state: &IndexState,
) -> Result<IncrementalIndexOutcome> {
    if let Some(reason) = incremental_preflight(paths, delta, previous_state, current_state) {
        return Ok(IncrementalIndexOutcome::FullRebuildRequired(reason));
    }

    let slice = match build_incremental_slice(artifact, delta)? {
        Ok(slice) => slice,
        Err(reason) => return Ok(IncrementalIndexOutcome::FullRebuildRequired(reason)),
    };

    let mut store = match GraphStore::open(&paths.graph_db()) {
        Ok(store) => store,
        Err(err) => {
            return Ok(IncrementalIndexOutcome::FullRebuildRequired(format!(
                "graph index unavailable for incremental update: {err}"
            )))
        }
    };
    let mut search = match SearchIndexWriter::open(&paths.search_dir()) {
        Ok(search) => search,
        Err(err) => {
            return Ok(IncrementalIndexOutcome::FullRebuildRequired(format!(
                "search index unavailable for incremental update: {err}"
            )))
        }
    };
    if !search.supports_file_deletes() {
        return Ok(IncrementalIndexOutcome::FullRebuildRequired(
            "search index schema lacks exact path field".into(),
        ));
    }

    for file_path in &slice.changed_paths {
        store.delete_file(file_path)?;
    }
    let stats_nodes = store.ingest(slice.nodes.clone().into_iter(), std::iter::empty())?;
    let stats_edges = store.ingest(std::iter::empty(), slice.edges.into_iter())?;
    let adj = Adjacency::build(&store)?;
    compute_layout(&store, &adj)?;

    for file_path in &slice.changed_paths {
        if !search.delete_file(file_path)? {
            return Ok(IncrementalIndexOutcome::FullRebuildRequired(
                "search index schema lacks exact path field".into(),
            ));
        }
    }
    search.add_nodes(slice.nodes.into_iter())?;
    search.add_chunks(slice.chunks.into_iter())?;
    search.commit()?;

    Ok(IncrementalIndexOutcome::Applied(IndexSummary {
        nodes: store.node_count()?,
        edges: store.edge_count()?,
        dangling_edges: stats_nodes.dangling_edges + stats_edges.dangling_edges,
        chunks: artifact.manifest.stats.chunks,
        bad_lines: slice.bad_lines,
        incremental: true,
    }))
}

fn incremental_preflight(
    paths: &RepoPaths,
    delta: &IndexDelta,
    previous_state: &IndexState,
    current_state: &IndexState,
) -> Option<String> {
    if delta.is_empty() {
        return Some("no file changes to apply incrementally".into());
    }
    if previous_state.version != current_state.version
        || previous_state.contract_version != current_state.contract_version
        || previous_state.engine_sha != current_state.engine_sha
        || previous_state.no_chunks != current_state.no_chunks
    {
        return Some("index state metadata changed".into());
    }
    if !delta.deleted.is_empty() {
        return Some("deleted files require full graph/search rebuild".into());
    }
    let changed = delta.changed_count();
    if changed > MAX_INCREMENTAL_CHANGED_FILES {
        return Some(format!(
            "too many changed files for incremental update: {changed} > {MAX_INCREMENTAL_CHANGED_FILES}"
        ));
    }
    let total_files = current_state
        .files
        .len()
        .max(previous_state.files.len())
        .max(1);
    let ratio_limit = (total_files / INCREMENTAL_RATIO_DIVISOR).max(1);
    if changed > ratio_limit {
        return Some(format!(
            "changed file ratio too large for incremental update: {changed}/{total_files}"
        ));
    }
    if !paths.graph_db().is_file() {
        return Some("graph index missing".into());
    }
    if !paths.search_dir().is_dir() {
        return Some("search index missing".into());
    }
    None
}

fn build_incremental_slice(
    artifact: &ArtifactDir,
    delta: &IndexDelta,
) -> Result<std::result::Result<IncrementalSlice, String>> {
    let changed_paths: BTreeSet<String> = delta
        .added
        .iter()
        .chain(delta.modified.iter())
        .cloned()
        .collect();
    if changed_paths.is_empty() {
        return Ok(Err("no added or modified files to replace".into()));
    }

    let mut bad_lines = 0u64;
    let mut nodes = Vec::new();
    let mut changed_node_ids = BTreeSet::new();
    let mut node_info: HashMap<String, NodeInfo> = HashMap::new();

    for node in artifact.nodes()? {
        let node = match node {
            Ok(node) => node,
            Err(_) => {
                bad_lines += 1;
                continue;
            }
        };
        let file_path = node
            .file_path()
            .filter(|path| !path.is_empty())
            .map(str::to_owned);
        let is_changed_file = file_path
            .as_deref()
            .is_some_and(|path| changed_paths.contains(path));
        if is_changed_file {
            if is_global_or_derived_label(&node.label) {
                return Ok(Err(format!(
                    "changed file contains derived/global node label {}",
                    node.label
                )));
            }
            changed_node_ids.insert(node.id.clone());
            nodes.push(node.clone());
        }
        node_info.insert(
            node.id.clone(),
            NodeInfo {
                label: node.label,
                file_path,
            },
        );
    }

    let mut edges = Vec::new();
    for edge in artifact.edges()? {
        let edge = match edge {
            Ok(edge) => edge,
            Err(_) => {
                bad_lines += 1;
                continue;
            }
        };
        let source_changed = changed_node_ids.contains(&edge.source_id);
        let target_changed = changed_node_ids.contains(&edge.target_id);
        if !source_changed && !target_changed {
            continue;
        }
        if source_changed ^ target_changed {
            let other_id = if source_changed {
                &edge.target_id
            } else {
                &edge.source_id
            };
            if let Some(other) = node_info.get(other_id) {
                if !is_safe_cross_file_endpoint(other) {
                    return Ok(Err(format!(
                        "changed file edge touches global or derived node {other_id}"
                    )));
                }
            }
        }
        edges.push(edge);
    }

    let mut chunks = Vec::new();
    if let Some(chunk_iter) = artifact.chunks()? {
        for chunk in chunk_iter {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(_) => {
                    bad_lines += 1;
                    continue;
                }
            };
            if changed_paths.contains(&chunk.file_path) {
                chunks.push(chunk);
            }
        }
    }

    Ok(Ok(IncrementalSlice {
        changed_paths,
        nodes,
        edges,
        chunks,
        bad_lines,
    }))
}

fn is_safe_cross_file_endpoint(info: &NodeInfo) -> bool {
    info.file_path
        .as_deref()
        .is_some_and(|path| !path.is_empty())
        && !is_global_or_derived_label(&info.label)
}

fn is_global_or_derived_label(label: &str) -> bool {
    matches!(
        label,
        "Community"
            | "Process"
            | "Route"
            | "GraphQL"
            | "Tool"
            | "Command"
            | "Migration"
            | "Transaction"
    )
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
