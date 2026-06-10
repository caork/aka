//! 索引管线：NDJSON 工件 → 图存储（SQLite + 布局） + 搜索索引（tantivy）。

use std::path::Path;

use anyhow::{Context, Result};

use aka_core::{ArtifactDir, RepoPaths};
use aka_graph::{compute_layout, Adjacency, GraphStore};
use aka_search::SearchIndexWriter;

pub struct IndexSummary {
    pub nodes: u64,
    pub edges: u64,
    pub dangling_edges: u64,
    pub chunks: u64,
    pub bad_lines: u64,
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
    let stats_nodes = store.ingest(nodes, std::iter::empty())?;
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
        nodes: stats_nodes.nodes,
        edges: stats_edges.edges,
        dangling_edges: stats_edges.dangling_edges,
        chunks: chunk_count,
        bad_lines,
    })
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
