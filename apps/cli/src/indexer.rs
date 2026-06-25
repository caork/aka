//! 索引管线：replayable facts → 图存储（SQLite + 布局） + 搜索索引（tantivy）。

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use aka_core::{
    validate_enrichment_batch_provenance, ChunkRec, EdgeRec, FactBatch, FactSource, IndexDelta,
    IndexState, IndexingDeadline, NodeRec, RepoPaths,
};
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnrichmentMergeSummary {
    pub new_nodes: u64,
    pub duplicate_nodes: u64,
    pub new_edges: u64,
    pub duplicate_edges: u64,
    pub dangling_edges: u64,
    pub chunks: u64,
    pub bad_lines: u64,
}

impl EnrichmentMergeSummary {
    pub fn add_assign(&mut self, other: Self) {
        self.new_nodes += other.new_nodes;
        self.duplicate_nodes += other.duplicate_nodes;
        self.new_edges += other.new_edges;
        self.duplicate_edges += other.duplicate_edges;
        self.dangling_edges += other.dangling_edges;
        self.chunks += other.chunks;
        self.bad_lines += other.bad_lines;
    }
}

pub enum IncrementalIndexOutcome {
    Applied(IndexSummary),
    FullRebuildRequired(String),
}

pub type IndexProgress<'a> = dyn FnMut(IndexProgressEvent) + 'a;

#[derive(Debug, Clone)]
pub struct IndexProgressEvent {
    pub stage: &'static str,
    pub message: String,
    pub current: Option<u64>,
    pub total: Option<u64>,
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

/// Index a replayable fact source into graph and search storage.
///
/// This is the primary seam for the fused pipeline: direct engine producers
/// and future SCIP/stack-graph importers feed this graph/search writer without
/// requiring NDJSON artifacts on disk.
pub fn index_facts(source: &impl FactSource, paths: &RepoPaths) -> Result<IndexSummary> {
    index_facts_with_progress(source, paths, None)
}

pub fn index_facts_with_progress(
    source: &impl FactSource,
    paths: &RepoPaths,
    progress: Option<&mut IndexProgress<'_>>,
) -> Result<IndexSummary> {
    index_facts_inner(source, paths, None, progress)
}

pub fn index_facts_with_deadline_progress(
    source: &impl FactSource,
    paths: &RepoPaths,
    deadline: IndexingDeadline,
    progress: Option<&mut IndexProgress<'_>>,
) -> Result<IndexSummary> {
    index_facts_inner(source, paths, Some(deadline), progress)
}

/// Merge optional analyzer facts into an already-ready graph/search index.
///
/// This is intentionally append-only and conservative:
/// - graph edges rely on provenance edge ids for dedupe;
/// - search documents are added only for newly inserted nodes and their chunks;
/// - existing baseline graph/search stays usable if callers decide to skip this
///   optional pass on error.
pub fn merge_enrichment_facts(
    source: &impl FactSource,
    paths: &RepoPaths,
) -> Result<EnrichmentMergeSummary> {
    merge_enrichment_facts_with_progress(source, paths, None)
}

pub fn merge_enrichment_facts_with_progress(
    source: &impl FactSource,
    paths: &RepoPaths,
    progress: Option<&mut IndexProgress<'_>>,
) -> Result<EnrichmentMergeSummary> {
    merge_enrichment_facts_inner(source, paths, None, progress)
}

pub fn merge_enrichment_facts_with_deadline_progress(
    source: &impl FactSource,
    paths: &RepoPaths,
    deadline: IndexingDeadline,
    progress: Option<&mut IndexProgress<'_>>,
) -> Result<EnrichmentMergeSummary> {
    merge_enrichment_facts_inner(source, paths, Some(deadline), progress)
}

fn merge_enrichment_facts_inner(
    source: &impl FactSource,
    paths: &RepoPaths,
    deadline: Option<IndexingDeadline>,
    progress: Option<&mut IndexProgress<'_>>,
) -> Result<EnrichmentMergeSummary> {
    validate_enrichment_source_provenance(source)?;
    let staged_paths = stage_existing_indexes_for_enrichment(paths)?;
    let summary = merge_enrichment_facts_staged(source, &staged_paths, deadline, progress);
    match summary {
        Ok(summary) => {
            install_staged_enrichment(paths, &staged_paths)?;
            Ok(summary)
        }
        Err(err) => {
            cleanup_staged_enrichment(&staged_paths);
            Err(err)
        }
    }
}

fn validate_enrichment_source_provenance(source: &impl FactSource) -> Result<()> {
    let nodes = source
        .nodes()?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let edges = source
        .edges()?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let batch = FactBatch::new(source.stats().clone(), nodes, edges, Vec::new());
    validate_enrichment_batch_provenance(&batch).context("validate enrichment facts provenance")?;
    Ok(())
}

fn merge_enrichment_facts_staged(
    source: &impl FactSource,
    paths: &RepoPaths,
    deadline: Option<IndexingDeadline>,
    mut progress: Option<&mut IndexProgress<'_>>,
) -> Result<EnrichmentMergeSummary> {
    emit_index_progress(
        progress.as_deref_mut(),
        "enrichment:open",
        "opening staged graph and search indexes".into(),
        None,
        None,
    );
    let mut store = GraphStore::open(&paths.graph_db())
        .with_context(|| format!("open graph index {}", paths.graph_db().display()))?;
    let mut search = SearchIndexWriter::open(&paths.search_dir())
        .with_context(|| format!("open search index {}", paths.search_dir().display()))?;

    emit_index_progress(
        progress.as_deref_mut(),
        "enrichment:slice",
        "filtering enrichment nodes already present in graph".into(),
        Some(0),
        Some(source.stats().nodes),
    );
    let mut summary = EnrichmentMergeSummary::default();
    let mut new_node_ids = BTreeSet::new();
    let mut nodes = Vec::new();
    for node in source.nodes()? {
        let node = match node {
            Ok(node) => node,
            Err(_) => {
                summary.bad_lines += 1;
                continue;
            }
        };
        check_deadline(deadline, "enrichment:slice:nodes")?;
        if store.node_by_id(&node.id)?.is_some() {
            summary.duplicate_nodes += 1;
            continue;
        }
        new_node_ids.insert(node.id.clone());
        nodes.push(node);
    }

    let mut edges = Vec::new();
    for edge in source.edges()? {
        let edge = match edge {
            Ok(edge) => edge,
            Err(_) => {
                summary.bad_lines += 1;
                continue;
            }
        };
        check_deadline(deadline, "enrichment:slice:edges")?;
        edges.push(edge);
    }

    let mut chunks = Vec::new();
    if let Some(chunk_iter) = source.chunks()? {
        for chunk in chunk_iter {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(_) => {
                    summary.bad_lines += 1;
                    continue;
                }
            };
            check_deadline(deadline, "enrichment:slice:chunks")?;
            if new_node_ids.contains(&chunk.node_id) {
                chunks.push(chunk);
            }
        }
    }

    emit_index_progress(
        progress.as_deref_mut(),
        "enrichment:graph",
        format!("appending {} nodes and {} edges", nodes.len(), edges.len()),
        Some(0),
        Some((nodes.len() + edges.len()) as u64),
    );
    let graph_stats =
        store.ingest_with_cancel(nodes.clone().into_iter(), edges.into_iter(), || {
            deadline_expired(deadline)
        })?;
    summary.new_nodes = graph_stats.nodes;
    summary.duplicate_nodes += graph_stats.duplicate_nodes;
    summary.new_edges = graph_stats.edges;
    summary.duplicate_edges = graph_stats.duplicate_edges;
    summary.dangling_edges = graph_stats.dangling_edges;
    check_deadline(deadline, "enrichment:graph")?;

    emit_index_progress(
        progress.as_deref_mut(),
        "enrichment:layout",
        "rebuilding adjacency and layout after enrichment".into(),
        None,
        None,
    );
    let adj = Adjacency::build(&store)?;
    check_deadline(deadline, "enrichment:layout")?;
    compute_layout(&store, &adj)?;

    emit_index_progress(
        progress.as_deref_mut(),
        "enrichment:search",
        format!(
            "adding {} new enrichment nodes and {} chunks to search",
            nodes.len(),
            chunks.len()
        ),
        Some(0),
        Some((nodes.len() + chunks.len()) as u64),
    );
    search.add_nodes_with_cancel(nodes.into_iter(), || deadline_expired(deadline))?;
    check_deadline(deadline, "enrichment:search:nodes")?;
    let chunk_count = chunks.len() as u64;
    search.add_chunks_with_cancel(chunks.into_iter(), || deadline_expired(deadline))?;
    summary.chunks = chunk_count;
    check_deadline(deadline, "enrichment:search:chunks")?;
    search.commit()?;
    emit_index_progress(
        progress,
        "enrichment:done",
        format!(
            "enrichment merge complete; nodes={} edges={} duplicate_edges={} dangling_edges={}",
            summary.new_nodes, summary.new_edges, summary.duplicate_edges, summary.dangling_edges
        ),
        None,
        None,
    );

    Ok(summary)
}

fn stage_existing_indexes_for_enrichment(paths: &RepoPaths) -> Result<RepoPaths> {
    checkpoint_graph(&paths.graph_db())?;
    let staging_root = unique_staging_root(&paths.root, "enrichment");
    let graph_tmp = staging_root.join("graph.db");
    let search_tmp = staging_root.join("search");
    std::fs::create_dir_all(&staging_root)
        .with_context(|| format!("create enrichment staging dir {}", staging_root.display()))?;
    std::fs::copy(paths.graph_db(), &graph_tmp).with_context(|| {
        format!(
            "copy graph index {} to {}",
            paths.graph_db().display(),
            graph_tmp.display()
        )
    })?;
    copy_dir_all(&paths.search_dir(), &search_tmp).with_context(|| {
        format!(
            "copy search index {} to {}",
            paths.search_dir().display(),
            search_tmp.display()
        )
    })?;
    Ok(RepoPaths { root: staging_root })
}

fn install_staged_enrichment(paths: &RepoPaths, staged_paths: &RepoPaths) -> Result<()> {
    checkpoint_graph(&staged_paths.graph_db())?;
    checkpoint_graph(&paths.graph_db())?;
    let backup_paths = RepoPaths {
        root: unique_staging_root(&paths.root, "enrichment-backup"),
    };
    let install = install_staged_enrichment_inner(paths, staged_paths, &backup_paths);
    if install.is_err() {
        restore_enrichment_backup(paths, &backup_paths);
    }
    install.with_context(|| {
        format!(
            "install staged enrichment indexes from {}",
            staged_paths.root.display()
        )
    })?;
    cleanup_staged_enrichment(&backup_paths);
    cleanup_staged_enrichment(staged_paths);
    Ok(())
}

fn install_staged_enrichment_inner(
    paths: &RepoPaths,
    staged_paths: &RepoPaths,
    backup_paths: &RepoPaths,
) -> Result<()> {
    std::fs::create_dir_all(&backup_paths.root).with_context(|| {
        format!(
            "create enrichment backup dir {}",
            backup_paths.root.display()
        )
    })?;
    remove_sqlite_sidecars(&paths.graph_db())?;
    std::fs::rename(paths.search_dir(), backup_paths.search_dir()).with_context(|| {
        format!(
            "backup search index {} to {}",
            paths.search_dir().display(),
            backup_paths.search_dir().display()
        )
    })?;
    std::fs::rename(paths.graph_db(), backup_paths.graph_db()).with_context(|| {
        format!(
            "backup graph index {} to {}",
            paths.graph_db().display(),
            backup_paths.graph_db().display()
        )
    })?;
    std::fs::rename(staged_paths.graph_db(), paths.graph_db()).with_context(|| {
        format!(
            "install graph index {} to {}",
            staged_paths.graph_db().display(),
            paths.graph_db().display()
        )
    })?;
    std::fs::rename(staged_paths.search_dir(), paths.search_dir()).with_context(|| {
        format!(
            "install search index {} to {}",
            staged_paths.search_dir().display(),
            paths.search_dir().display()
        )
    })?;
    Ok(())
}

fn restore_enrichment_backup(paths: &RepoPaths, backup_paths: &RepoPaths) {
    if backup_paths.graph_db().is_file() {
        let _ = std::fs::remove_file(paths.graph_db());
        let _ = remove_sqlite_sidecars(&paths.graph_db());
        let _ = std::fs::rename(backup_paths.graph_db(), paths.graph_db());
    }
    if backup_paths.search_dir().is_dir() {
        if paths.search_dir().exists() {
            let _ = std::fs::remove_dir_all(paths.search_dir());
        }
        let _ = std::fs::rename(backup_paths.search_dir(), paths.search_dir());
    }
}

fn cleanup_staged_enrichment(staged_paths: &RepoPaths) {
    let _ = std::fs::remove_dir_all(&staged_paths.root);
}

fn index_facts_inner(
    source: &impl FactSource,
    paths: &RepoPaths,
    deadline: Option<IndexingDeadline>,
    mut progress: Option<&mut IndexProgress<'_>>,
) -> Result<IndexSummary> {
    let mut bad_lines = 0u64;
    let stats = source.stats();

    // ── 图存储 ───────────────────────────────────────────────
    let db_path = paths.graph_db();
    emit_index_progress(
        progress.as_deref_mut(),
        "graph",
        format!("creating graph database {}", db_path.display()),
        None,
        None,
    );
    remove_if_exists(&db_path)?;
    let mut store = GraphStore::create(&db_path)
        .with_context(|| format!("创建图库失败 {}", db_path.display()))?;

    emit_index_progress(
        progress.as_deref_mut(),
        "graph:nodes",
        format!("ingesting {} fact nodes", stats.nodes),
        Some(0),
        Some(stats.nodes),
    );
    let nodes = source.nodes()?.filter_map(|r| match r {
        Ok(n) => Some(n),
        Err(_) => {
            bad_lines += 1;
            None
        }
    });
    // 借用检查：nodes/edges 两个迭代器都要捕获 bad_lines，分两次摄取。
    if let Some(deadline) = deadline {
        deadline.check("graph:nodes")?;
    }
    store.ingest_with_cancel(nodes, std::iter::empty(), || {
        deadline.is_some_and(|deadline| deadline.is_expired())
    })?;
    check_deadline(deadline, "graph:nodes")?;
    emit_index_progress(
        progress.as_deref_mut(),
        "graph:nodes",
        "node ingest complete".into(),
        Some(stats.nodes),
        Some(stats.nodes),
    );

    emit_index_progress(
        progress.as_deref_mut(),
        "graph:edges",
        format!("ingesting {} fact edges", stats.edges),
        Some(0),
        Some(stats.edges),
    );
    let mut bad_edge_lines = 0u64;
    let edges = source.edges()?.filter_map(|r| match r {
        Ok(e) => Some(e),
        Err(_) => {
            bad_edge_lines += 1;
            None
        }
    });
    if let Some(deadline) = deadline {
        deadline.check("graph:edges")?;
    }
    let stats_edges = store.ingest_with_cancel(std::iter::empty(), edges, || {
        deadline.is_some_and(|deadline| deadline.is_expired())
    })?;
    check_deadline(deadline, "graph:edges")?;
    bad_lines += bad_edge_lines;
    emit_index_progress(
        progress.as_deref_mut(),
        "graph:edges",
        format!(
            "edge ingest complete; dangling_edges={}",
            stats_edges.dangling_edges
        ),
        Some(stats.edges),
        Some(stats.edges),
    );

    // 布局（确定性 phyllotaxis，给可视化用）。
    emit_index_progress(
        progress.as_deref_mut(),
        "graph:layout",
        "building adjacency and deterministic layout".into(),
        None,
        None,
    );
    if let Some(deadline) = deadline {
        deadline.check("graph:layout")?;
    }
    let adj = Adjacency::build(&store)?;
    if let Some(deadline) = deadline {
        deadline.check("graph:layout")?;
    }
    compute_layout(&store, &adj)?;
    emit_index_progress(
        progress.as_deref_mut(),
        "graph:layout",
        "layout complete".into(),
        None,
        None,
    );

    // ── 搜索索引 ─────────────────────────────────────────────
    let search_dir = paths.search_dir();
    emit_index_progress(
        progress.as_deref_mut(),
        "search",
        format!("creating search index {}", search_dir.display()),
        None,
        None,
    );
    if search_dir.exists() {
        std::fs::remove_dir_all(&search_dir)?;
    }
    std::fs::create_dir_all(&search_dir)?;
    // 写句柄持 tantivy 目录写锁，限定作用域：commit 后立即 drop 释放，
    // 不阻塞其他进程（serve / mcp）的只读查询打开。
    let chunk_count = {
        let mut search = SearchIndexWriter::create(&search_dir)?;
        // 节点先于 chunk 摄取：chunk 文档要携带所属节点的真实 label。
        emit_index_progress(
            progress.as_deref_mut(),
            "search:nodes",
            format!("adding {} nodes to search index", stats.nodes),
            Some(0),
            Some(stats.nodes),
        );
        if let Some(deadline) = deadline {
            deadline.check("search:nodes")?;
        }
        search.add_nodes_with_cancel(source.nodes()?.filter_map(|r| r.ok()), || {
            deadline.is_some_and(|deadline| deadline.is_expired())
        })?;
        check_deadline(deadline, "search:nodes")?;
        emit_index_progress(
            progress.as_deref_mut(),
            "search:nodes",
            "search node add complete".into(),
            Some(stats.nodes),
            Some(stats.nodes),
        );
        let mut chunk_count = 0u64;
        if let Some(chunks) = source.chunks()? {
            emit_index_progress(
                progress.as_deref_mut(),
                "search:chunks",
                format!("adding {} chunks to search index", stats.chunks),
                Some(0),
                Some(stats.chunks),
            );
            if let Some(deadline) = deadline {
                deadline.check("search:chunks")?;
            }
            search.add_chunks_with_cancel(
                chunks.filter_map(|r| r.ok()).inspect(|_| chunk_count += 1),
                || deadline.is_some_and(|deadline| deadline.is_expired()),
            )?;
            check_deadline(deadline, "search:chunks")?;
            emit_index_progress(
                progress.as_deref_mut(),
                "search:chunks",
                "search chunk add complete".into(),
                Some(chunk_count),
                Some(stats.chunks),
            );
        } else {
            emit_index_progress(
                progress.as_deref_mut(),
                "search:chunks",
                "chunks disabled; skipping search chunks".into(),
                Some(0),
                Some(0),
            );
        }
        emit_index_progress(
            progress.as_deref_mut(),
            "search:commit",
            "committing search index".into(),
            None,
            None,
        );
        if let Some(deadline) = deadline {
            deadline.check("search:commit")?;
        }
        search.commit()?;
        emit_index_progress(
            progress,
            "search:commit",
            "search index commit complete".into(),
            None,
            None,
        );
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
/// This function conservatively slices a replayable direct-facts source down to
/// added/modified files, deletes old rows for those files from the existing
/// indexes, and appends replacement rows. If any condition could make
/// file-scoped replacement unsafe, it reports `FullRebuildRequired` and leaves
/// the existing indexes untouched.
pub fn index_facts_incremental(
    source: &impl FactSource,
    paths: &RepoPaths,
    delta: &IndexDelta,
    previous_state: &IndexState,
    current_state: &IndexState,
) -> Result<IncrementalIndexOutcome> {
    index_facts_incremental_with_progress(source, paths, delta, previous_state, current_state, None)
}

pub fn index_facts_incremental_with_progress(
    source: &impl FactSource,
    paths: &RepoPaths,
    delta: &IndexDelta,
    previous_state: &IndexState,
    current_state: &IndexState,
    progress: Option<&mut IndexProgress<'_>>,
) -> Result<IncrementalIndexOutcome> {
    index_facts_incremental_inner(
        source,
        paths,
        delta,
        previous_state,
        current_state,
        None,
        progress,
    )
}

pub fn index_facts_incremental_with_deadline_progress(
    source: &impl FactSource,
    paths: &RepoPaths,
    delta: &IndexDelta,
    previous_state: &IndexState,
    current_state: &IndexState,
    deadline: IndexingDeadline,
    progress: Option<&mut IndexProgress<'_>>,
) -> Result<IncrementalIndexOutcome> {
    index_facts_incremental_inner(
        source,
        paths,
        delta,
        previous_state,
        current_state,
        Some(deadline),
        progress,
    )
}

fn index_facts_incremental_inner(
    source: &impl FactSource,
    paths: &RepoPaths,
    delta: &IndexDelta,
    previous_state: &IndexState,
    current_state: &IndexState,
    deadline: Option<IndexingDeadline>,
    mut progress: Option<&mut IndexProgress<'_>>,
) -> Result<IncrementalIndexOutcome> {
    if let Some(reason) = incremental_preflight(paths, delta, previous_state, current_state) {
        emit_index_progress(
            progress.as_deref_mut(),
            "incremental:preflight",
            format!("full rebuild required: {reason}"),
            None,
            None,
        );
        return Ok(IncrementalIndexOutcome::FullRebuildRequired(reason));
    }

    emit_index_progress(
        progress.as_deref_mut(),
        "incremental:slice",
        format!("building incremental slice ({})", delta.summary()),
        None,
        None,
    );
    let slice = match build_incremental_slice(source, delta)? {
        Ok(slice) => slice,
        Err(reason) => {
            emit_index_progress(
                progress.as_deref_mut(),
                "incremental:slice",
                format!("full rebuild required: {reason}"),
                None,
                None,
            );
            return Ok(IncrementalIndexOutcome::FullRebuildRequired(reason));
        }
    };

    emit_index_progress(
        progress.as_deref_mut(),
        "incremental:open",
        "opening existing graph and search indexes".into(),
        None,
        None,
    );
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

    emit_index_progress(
        progress.as_deref_mut(),
        "incremental:graph",
        format!(
            "replacing {} changed files in graph",
            slice.changed_paths.len()
        ),
        Some(0),
        Some(slice.changed_paths.len() as u64),
    );
    for file_path in &slice.changed_paths {
        check_deadline(deadline, "incremental:graph")?;
        store.delete_file(file_path)?;
    }
    let stats_nodes =
        store.ingest_with_cancel(slice.nodes.clone().into_iter(), std::iter::empty(), || {
            deadline_expired(deadline)
        })?;
    check_deadline(deadline, "incremental:graph")?;
    let stats_edges =
        store.ingest_with_cancel(std::iter::empty(), slice.edges.into_iter(), || {
            deadline_expired(deadline)
        })?;
    check_deadline(deadline, "incremental:graph")?;
    emit_index_progress(
        progress.as_deref_mut(),
        "incremental:layout",
        "rebuilding adjacency and layout".into(),
        None,
        None,
    );
    if let Some(deadline) = deadline {
        deadline.check("incremental:layout")?;
    }
    let adj = Adjacency::build(&store)?;
    compute_layout(&store, &adj)?;

    emit_index_progress(
        progress.as_deref_mut(),
        "incremental:search",
        format!(
            "replacing {} changed files in search index",
            slice.changed_paths.len()
        ),
        Some(0),
        Some(slice.changed_paths.len() as u64),
    );
    for file_path in &slice.changed_paths {
        if let Some(deadline) = deadline {
            deadline.check("incremental:search")?;
        }
        if !search.delete_file(file_path)? {
            return Ok(IncrementalIndexOutcome::FullRebuildRequired(
                "search index schema lacks exact path field".into(),
            ));
        }
    }
    search.add_nodes_with_cancel(slice.nodes.into_iter(), || deadline_expired(deadline))?;
    check_deadline(deadline, "incremental:search")?;
    search.add_chunks_with_cancel(slice.chunks.into_iter(), || deadline_expired(deadline))?;
    check_deadline(deadline, "incremental:search")?;
    emit_index_progress(
        progress.as_deref_mut(),
        "incremental:commit",
        "committing incremental search index".into(),
        None,
        None,
    );
    if let Some(deadline) = deadline {
        deadline.check("incremental:commit")?;
    }
    search.commit()?;
    emit_index_progress(
        progress,
        "incremental:done",
        format!(
            "incremental replacement complete; dangling_edges={}",
            stats_nodes.dangling_edges + stats_edges.dangling_edges
        ),
        None,
        None,
    );

    Ok(IncrementalIndexOutcome::Applied(IndexSummary {
        nodes: store.node_count()?,
        edges: store.edge_count()?,
        dangling_edges: stats_nodes.dangling_edges + stats_edges.dangling_edges,
        chunks: source.stats().chunks,
        bad_lines: slice.bad_lines,
        incremental: true,
    }))
}

fn deadline_expired(deadline: Option<IndexingDeadline>) -> bool {
    deadline.is_some_and(|deadline| deadline.is_expired())
}

fn check_deadline(deadline: Option<IndexingDeadline>, stage: &'static str) -> Result<()> {
    if let Some(deadline) = deadline {
        deadline.check(stage)?;
    }
    Ok(())
}

fn emit_index_progress(
    progress: Option<&mut IndexProgress<'_>>,
    stage: &'static str,
    message: String,
    current: Option<u64>,
    total: Option<u64>,
) {
    if let Some(progress) = progress {
        progress(IndexProgressEvent {
            stage,
            message,
            current,
            total,
        });
    }
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

fn checkpoint_graph(graph_db: &Path) -> Result<()> {
    let store = GraphStore::open(graph_db)
        .with_context(|| format!("open graph index {}", graph_db.display()))?;
    store
        .checkpoint()
        .with_context(|| format!("checkpoint graph index {}", graph_db.display()))?;
    Ok(())
}

fn unique_staging_root(root: &Path, kind: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let id = format!(
        "{}.tmp-{}-{}-{:?}",
        kind,
        std::process::id(),
        nanos,
        std::thread::current().id()
    )
    .replace(['(', ')', ' '], "-");
    root.join(id)
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("create dir {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("read dir {}", src.display()))? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&from, &to)?;
        } else if ty.is_file() {
            std::fs::copy(&from, &to)
                .with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

fn remove_sqlite_sidecars(path: &Path) -> Result<()> {
    remove_if_exists(&sidecar_path(path, "wal"))?;
    remove_if_exists(&sidecar_path(path, "shm"))?;
    Ok(())
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(format!("-{suffix}"));
    PathBuf::from(os)
}

fn build_incremental_slice(
    source: &impl FactSource,
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

    for node in source.nodes()? {
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
    for edge in source.edges()? {
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
    if let Some(chunk_iter) = source.chunks()? {
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
            | "Config"
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

#[cfg(test)]
mod tests {
    use super::*;
    use aka_core::{FactBatch, FactStats};
    use aka_search::SearchIndex;

    #[test]
    fn indexes_replayable_fact_batch_without_ndjson_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "fn load_manifest() {}\n").unwrap();

        let paths = RepoPaths {
            root: tmp.path().join("aka-data").join("repo"),
        };
        std::fs::create_dir_all(&paths.root).unwrap();
        let batch = FactBatch::new(
            FactStats {
                files: 1,
                nodes: 1,
                edges: 0,
                chunks: 1,
            },
            vec![NodeRec {
                id: "sym:load_manifest".into(),
                label: "Function".into(),
                properties: serde_json::json!({
                    "name": "load_manifest",
                    "qualifiedName": "demo::load_manifest",
                    "filePath": "src/lib.rs",
                    "startLine": 0,
                    "endLine": 0
                })
                .as_object()
                .unwrap()
                .clone(),
            }],
            Vec::new(),
            vec![ChunkRec {
                node_id: "sym:load_manifest".into(),
                kind: "ast-function".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "fn load_manifest() {}".into(),
            }],
        );

        let summary = index_facts(&batch, &paths).unwrap();

        assert_eq!(summary.nodes, 1);
        assert_eq!(summary.edges, 0);
        assert_eq!(summary.chunks, 1);
        assert!(paths.graph_db().is_file());
        assert!(paths.search_dir().is_dir());
    }

    #[test]
    fn merge_enrichment_facts_appends_new_nodes_and_dedupes_provenance_edges() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RepoPaths {
            root: tmp.path().join("aka-data").join("repo"),
        };
        std::fs::create_dir_all(&paths.root).unwrap();

        let baseline = FactBatch::new(
            FactStats {
                files: 1,
                nodes: 1,
                edges: 0,
                chunks: 0,
            },
            vec![NodeRec {
                id: "sym:handler".into(),
                label: "Function".into(),
                properties: serde_json::json!({
                    "name": "handler",
                    "filePath": "src/lib.rs",
                    "startLine": 0
                })
                .as_object()
                .unwrap()
                .clone(),
            }],
            Vec::new(),
            Vec::new(),
        );
        index_facts(&baseline, &paths).unwrap();

        let enrichment = FactBatch::new(
            FactStats {
                files: 0,
                nodes: 1,
                edges: 1,
                chunks: 1,
            },
            vec![NodeRec {
                id: "scip:symbol:service".into(),
                label: "Interface".into(),
                properties: serde_json::json!({
                    "name": "Service",
                    "source": "scip",
                    "provenance": {
                        "source": "scip",
                        "analyzerId": "scip",
                        "toolVersion": "1.0",
                        "adapterVersion": "test",
                        "oss": true
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
            }],
            vec![EdgeRec {
                id: "scip:edge:handler-service".into(),
                source_id: "sym:handler".into(),
                target_id: "scip:symbol:service".into(),
                edge_type: "DEPENDS_ON".into(),
                confidence: 1.0,
                reason: "scip relationship".into(),
                step: None,
                evidence: Some(serde_json::json!({
                    "source": "scip",
                    "provenance": {
                        "source": "scip",
                        "analyzerId": "scip",
                        "toolVersion": "1.0",
                        "adapterVersion": "test",
                        "oss": true
                    }
                })),
            }],
            vec![ChunkRec {
                node_id: "scip:symbol:service".into(),
                kind: "scip-symbol".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "trait Service".into(),
            }],
        );

        let first = merge_enrichment_facts(&enrichment, &paths).unwrap();
        let second = merge_enrichment_facts(&enrichment, &paths).unwrap();

        assert_eq!(first.new_nodes, 1);
        assert_eq!(first.new_edges, 1);
        assert_eq!(first.chunks, 1);
        assert_eq!(second.new_nodes, 0);
        assert_eq!(second.duplicate_nodes, 1);
        assert_eq!(second.new_edges, 0);
        assert_eq!(second.duplicate_edges, 1);

        let graph = GraphStore::open(&paths.graph_db()).unwrap();
        assert_eq!(graph.node_count().unwrap(), 2);
        assert_eq!(graph.edge_count().unwrap(), 1);
        let handler = graph.node_by_id("sym:handler").unwrap().unwrap();
        let linked = graph
            .outgoing_linked_nodes(handler.rowid, &["DEPENDS_ON"])
            .unwrap();
        assert_eq!(linked.len(), 1);
        assert_eq!(
            linked[0]
                .evidence
                .as_ref()
                .and_then(|value| value.get("provenance"))
                .and_then(|value| value.get("analyzerId")),
            Some(&serde_json::json!("scip"))
        );

        let search = SearchIndex::open(&paths.search_dir()).unwrap();
        let hits = search.search("Service", 10).unwrap();
        assert_eq!(
            hits.iter()
                .filter(|hit| hit.node_id == "scip:symbol:service")
                .count(),
            1
        );
    }

    #[test]
    fn merge_enrichment_facts_rejects_missing_oss_provenance_before_staging() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RepoPaths {
            root: tmp.path().join("aka-data").join("repo"),
        };
        std::fs::create_dir_all(&paths.root).unwrap();

        let baseline = FactBatch::new(
            FactStats {
                files: 1,
                nodes: 1,
                edges: 0,
                chunks: 1,
            },
            vec![NodeRec {
                id: "sym:handler".into(),
                label: "Function".into(),
                properties: serde_json::json!({
                    "name": "handler",
                    "filePath": "src/lib.rs",
                    "startLine": 0
                })
                .as_object()
                .unwrap()
                .clone(),
            }],
            Vec::new(),
            vec![ChunkRec {
                node_id: "sym:handler".into(),
                kind: "ast-function".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "fn handler() {}".into(),
            }],
        );
        index_facts(&baseline, &paths).unwrap();

        let enrichment = FactBatch::new(
            FactStats {
                files: 0,
                nodes: 1,
                edges: 0,
                chunks: 1,
            },
            vec![NodeRec {
                id: "private:symbol:service".into(),
                label: "Interface".into(),
                properties: serde_json::json!({"name": "Service"})
                    .as_object()
                    .unwrap()
                    .clone(),
            }],
            Vec::new(),
            vec![ChunkRec {
                node_id: "private:symbol:service".into(),
                kind: "private-symbol".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "trait Service".into(),
            }],
        );

        let err = merge_enrichment_facts(&enrichment, &paths).unwrap_err();
        assert!(
            err.to_string()
                .contains("validate enrichment facts provenance"),
            "unexpected enrichment error: {err:#}"
        );

        let graph = GraphStore::open(&paths.graph_db()).unwrap();
        assert_eq!(graph.node_count().unwrap(), 1);
        assert!(graph
            .node_by_id("private:symbol:service")
            .unwrap()
            .is_none());
        let search = SearchIndex::open(&paths.search_dir()).unwrap();
        assert!(search
            .search("handler", 10)
            .unwrap()
            .iter()
            .any(|hit| hit.node_id == "sym:handler"));
        assert!(search
            .search("Service", 10)
            .unwrap()
            .iter()
            .all(|hit| hit.node_id != "private:symbol:service"));
    }

    #[test]
    fn merge_enrichment_failure_leaves_baseline_indexes_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = RepoPaths {
            root: tmp.path().join("aka-data").join("repo"),
        };
        std::fs::create_dir_all(&paths.root).unwrap();

        let baseline = FactBatch::new(
            FactStats {
                files: 1,
                nodes: 1,
                edges: 0,
                chunks: 1,
            },
            vec![NodeRec {
                id: "sym:handler".into(),
                label: "Function".into(),
                properties: serde_json::json!({
                    "name": "handler",
                    "filePath": "src/lib.rs",
                    "startLine": 0
                })
                .as_object()
                .unwrap()
                .clone(),
            }],
            Vec::new(),
            vec![ChunkRec {
                node_id: "sym:handler".into(),
                kind: "ast-function".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "fn handler() {}".into(),
            }],
        );
        index_facts(&baseline, &paths).unwrap();

        let enrichment = FactBatch::new(
            FactStats {
                files: 0,
                nodes: 1,
                edges: 0,
                chunks: 1,
            },
            vec![NodeRec {
                id: "scip:symbol:service".into(),
                label: "Interface".into(),
                properties: serde_json::json!({
                    "name": "Service",
                    "source": "scip",
                    "provenance": {
                        "source": "scip",
                        "analyzerId": "scip",
                        "toolVersion": "1.0",
                        "adapterVersion": "test",
                        "oss": true
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
            }],
            Vec::new(),
            vec![ChunkRec {
                node_id: "scip:symbol:service".into(),
                kind: "scip-symbol".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "trait Service".into(),
            }],
        );

        let err = merge_enrichment_facts_with_deadline_progress(
            &enrichment,
            &paths,
            IndexingDeadline::new(std::time::Duration::ZERO),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("timed out") || err.to_string().contains("cancelled"),
            "unexpected enrichment error: {err:#}"
        );

        let graph = GraphStore::open(&paths.graph_db()).unwrap();
        assert_eq!(graph.node_count().unwrap(), 1);
        assert_eq!(graph.edge_count().unwrap(), 0);
        assert!(graph.node_by_id("scip:symbol:service").unwrap().is_none());
        let search = SearchIndex::open(&paths.search_dir()).unwrap();
        assert!(search
            .search("handler", 10)
            .unwrap()
            .iter()
            .any(|hit| { hit.node_id == "sym:handler" }));
        assert!(search
            .search("Service", 10)
            .unwrap()
            .iter()
            .all(|hit| hit.node_id != "scip:symbol:service"));
    }
}
