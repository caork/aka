//! SQLite 持久层 — nodes/edges/edge_types/meta/positions/clusters 表，
//! 单事务批量摄取（prepared statement，悬空边跳过计数）+ 基本查询。
//! 打开时做轻量列迁移（`migrate`，目前只有 edges.step），旧库无需重建。

use std::collections::HashMap;
use std::path::Path;

use aka_core::{EdgeRec, NodeRec};
use rusqlite::{params, Connection, OpenFlags, Row};

use crate::error::{GraphError, Result};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS nodes (
    id         TEXT NOT NULL UNIQUE,
    label      TEXT NOT NULL,
    name       TEXT,
    file_path  TEXT,
    start_line INTEGER,
    end_line   INTEGER,
    props      TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS edges (
    source     INTEGER NOT NULL,
    target     INTEGER NOT NULL,
    type_id    INTEGER NOT NULL,
    confidence REAL NOT NULL DEFAULT 0,
    reason     TEXT NOT NULL DEFAULT '',
    step       INTEGER
);
CREATE TABLE IF NOT EXISTS edge_types (
    type_id INTEGER PRIMARY KEY,
    name    TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS positions (
    node    INTEGER PRIMARY KEY,
    x       REAL NOT NULL,
    y       REAL NOT NULL,
    size    REAL NOT NULL,
    cluster INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS clusters (
    cluster INTEGER PRIMARY KEY,
    label   TEXT NOT NULL,
    cx      REAL NOT NULL,
    cy      REAL NOT NULL,
    count   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_nodes_label ON nodes(label);
CREATE INDEX IF NOT EXISTS idx_nodes_name  ON nodes(name);
CREATE INDEX IF NOT EXISTS idx_nodes_file  ON nodes(file_path);
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source);
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target);
CREATE INDEX IF NOT EXISTS idx_edges_type   ON edges(type_id);
";

const NODE_COLS: &str = "rowid, id, label, name, file_path, start_line, end_line, props";

/// 图持久层。一个 `GraphStore` 对应一个 SQLite 文件（或内存库）。
pub struct GraphStore {
    conn: Connection,
}

/// 一次摄取的统计。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestStats {
    /// 新插入节点数。
    pub nodes: u64,
    /// 因 id 重复被跳过的节点数。
    pub duplicate_nodes: u64,
    /// 成功插入边数。
    pub edges: u64,
    /// 端点缺失（悬空）被跳过的边数。
    pub dangling_edges: u64,
}

/// File-scoped deletion statistics for incremental index replacement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteFileStats {
    /// Nodes removed from `nodes`.
    pub nodes: u64,
    /// Incident edges removed from `edges`.
    pub edges: u64,
    /// Layout rows removed from `positions` before the full layout tables were cleared.
    pub positions: u64,
}

/// positions 表一行（compute_layout 的产物）。
#[derive(Debug, Clone, PartialEq)]
pub struct PositionRow {
    /// nodes.rowid。
    pub node: i64,
    pub x: f64,
    pub y: f64,
    /// log(1+度数)。
    pub size: f64,
    pub cluster: u32,
}

/// nodes 表一行的物化结果。
#[derive(Debug, Clone)]
pub struct NodeRow {
    pub rowid: i64,
    pub id: String,
    pub label: String,
    pub name: Option<String>,
    pub file_path: Option<String>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    /// properties 完整 JSON。
    pub props: serde_json::Value,
}

impl NodeRow {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let props_text: String = row.get(7)?;
        Ok(Self {
            rowid: row.get(0)?,
            id: row.get(1)?,
            label: row.get(2)?,
            name: row.get(3)?,
            file_path: row.get(4)?,
            start_line: row.get(5)?,
            end_line: row.get(6)?,
            props: serde_json::from_str(&props_text).unwrap_or(serde_json::Value::Null),
        })
    }
}

/// 沿某条边抵达的节点（带边类型和 reason）。用于服务层做 GitNexus-like
/// route/tool map，而不暴露 GraphStore 内部 SQLite 连接。
#[derive(Debug, Clone)]
pub struct LinkedNodeRow {
    pub node: NodeRow,
    pub edge_type: String,
    pub reason: String,
    pub step: Option<u32>,
}

impl GraphStore {
    /// 新建（或打开已存在的）图库文件并确保 schema 就绪。
    pub fn create(db: &Path) -> Result<Self> {
        let conn = Connection::open(db)?;
        Self::init(conn)
    }

    /// 打开已存在的图库文件；文件不存在时报 `DbNotFound`。
    pub fn open(db: &Path) -> Result<Self> {
        if !db.exists() {
            return Err(GraphError::DbNotFound(db.to_path_buf()));
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI;
        let conn = Connection::open_with_flags(db, flags)?;
        Self::init(conn)
    }

    /// 内存图库（测试 / 临时分析用）。
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // WAL + NORMAL：批量摄取吞吐关键；内存库会自动回落 journal_mode=memory。
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL;")?;
        conn.execute_batch(SCHEMA)?;
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// 轻量迁移。`CREATE TABLE IF NOT EXISTS` 不会改动已存在的表，
    /// 旧库要在这里补列；create/open 都走可写连接，ALTER 安全。
    fn migrate(conn: &Connection) -> Result<()> {
        // edges.step：流程步号（`符号 -[STEP_IN_PROCESS]-> Process` 的 1-based 序）。
        let has_step = {
            let mut stmt = conn.prepare("PRAGMA table_info(edges)")?;
            let mut rows = stmt.query([])?;
            let mut found = false;
            while let Some(row) = rows.next()? {
                if row.get::<_, String>(1)? == "step" {
                    found = true;
                    break;
                }
            }
            found
        };
        if !has_step {
            conn.execute_batch("ALTER TABLE edges ADD COLUMN step INTEGER")?;
        }
        Ok(())
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// 单事务批量摄取。节点 id 重复跳过；悬空边（任一端点不存在）跳过并计数。
    pub fn ingest(
        &mut self,
        nodes: impl Iterator<Item = NodeRec>,
        edges: impl Iterator<Item = EdgeRec>,
    ) -> Result<IngestStats> {
        self.ingest_with_cancel(nodes, edges, || false)
    }

    pub fn ingest_with_cancel(
        &mut self,
        nodes: impl Iterator<Item = NodeRec>,
        edges: impl Iterator<Item = EdgeRec>,
        mut is_cancelled: impl FnMut() -> bool,
    ) -> Result<IngestStats> {
        let mut stats = IngestStats::default();
        let tx = self.conn.transaction()?;
        // 本批次 id -> rowid 映射；增量摄取时旧节点查库回填。
        let mut id_map: HashMap<String, i64> = HashMap::new();
        {
            let mut ins_node = tx.prepare_cached(
                "INSERT OR IGNORE INTO nodes (id, label, name, file_path, start_line, end_line, props) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut sel_node = tx.prepare_cached("SELECT rowid FROM nodes WHERE id = ?1")?;

            for n in nodes {
                if is_cancelled() {
                    return Err(GraphError::Cancelled("graph node ingest".into()));
                }
                let props = serde_json::to_string(&n.properties)?;
                let changed = ins_node.execute(params![
                    n.id,
                    n.label,
                    n.name(),
                    n.file_path(),
                    /* 行号统一存 1-based（工件是 tree-sitter 0-based row） */
                    n.start_line_1based(),
                    n.end_line_1based(),
                    props
                ])?;
                if changed == 1 {
                    id_map.insert(n.id, tx.last_insert_rowid());
                    stats.nodes += 1;
                } else {
                    let rowid: i64 = sel_node.query_row([&n.id], |r| r.get(0))?;
                    id_map.insert(n.id, rowid);
                    stats.duplicate_nodes += 1;
                }
            }

            // 边类型字典：先载入已有，再按需追加。
            let mut type_ids: HashMap<String, i64> = HashMap::new();
            let mut next_type_id: i64 = 0;
            {
                let mut stmt = tx.prepare("SELECT type_id, name FROM edge_types")?;
                let mut rows = stmt.query([])?;
                while let Some(row) = rows.next()? {
                    let tid: i64 = row.get(0)?;
                    let name: String = row.get(1)?;
                    next_type_id = next_type_id.max(tid + 1);
                    type_ids.insert(name, tid);
                }
            }
            let mut ins_type =
                tx.prepare_cached("INSERT INTO edge_types (type_id, name) VALUES (?1, ?2)")?;
            let mut ins_edge = tx.prepare_cached(
                "INSERT INTO edges (source, target, type_id, confidence, reason, step) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;

            let mut resolve = |id: &str, map: &mut HashMap<String, i64>| -> Result<Option<i64>> {
                if let Some(&rowid) = map.get(id) {
                    return Ok(Some(rowid));
                }
                let found: Option<i64> =
                    sel_node
                        .query_row([id], |r| r.get(0))
                        .map(Some)
                        .or_else(|e| match e {
                            rusqlite::Error::QueryReturnedNoRows => Ok(None),
                            other => Err(other),
                        })?;
                if let Some(rowid) = found {
                    map.insert(id.to_owned(), rowid);
                }
                Ok(found)
            };

            for e in edges {
                if is_cancelled() {
                    return Err(GraphError::Cancelled("graph edge ingest".into()));
                }
                let src = resolve(&e.source_id, &mut id_map)?;
                let dst = resolve(&e.target_id, &mut id_map)?;
                let (Some(src), Some(dst)) = (src, dst) else {
                    stats.dangling_edges += 1;
                    continue;
                };
                let tid = match type_ids.get(&e.edge_type) {
                    Some(&tid) => tid,
                    None => {
                        let tid = next_type_id;
                        next_type_id += 1;
                        ins_type.execute(params![tid, e.edge_type])?;
                        type_ids.insert(e.edge_type.clone(), tid);
                        tid
                    }
                };
                ins_edge.execute(params![src, dst, tid, e.confidence, e.reason, e.step])?;
                stats.edges += 1;
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    /// Delete all graph rows owned by a single source file.
    ///
    /// Incremental replacement is intentionally file-scoped: collect matching
    /// `nodes.rowid`s first, delete every incident edge, delete layout rows for
    /// those nodes, then remove the nodes. Since layout clusters depend on the
    /// full graph, the remaining `positions` and `clusters` tables are cleared
    /// and callers should run `compute_layout` after re-ingesting replacements.
    pub fn delete_file(&mut self, file_path: &str) -> Result<DeleteFileStats> {
        let tx = self.conn.transaction()?;
        let rowids = {
            let mut stmt = tx.prepare("SELECT rowid FROM nodes WHERE file_path = ?1")?;
            let rows = stmt.query_map([file_path], |row| row.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rowids.is_empty() {
            tx.execute("DELETE FROM positions", [])?;
            tx.execute("DELETE FROM clusters", [])?;
            tx.commit()?;
            return Ok(DeleteFileStats::default());
        }

        let mut stats = DeleteFileStats {
            nodes: rowids.len() as u64,
            ..DeleteFileStats::default()
        };
        {
            let mut del_edges =
                tx.prepare_cached("DELETE FROM edges WHERE source = ?1 OR target = ?1")?;
            let mut del_positions = tx.prepare_cached("DELETE FROM positions WHERE node = ?1")?;
            for rowid in &rowids {
                stats.edges += del_edges.execute([rowid])? as u64;
                stats.positions += del_positions.execute([rowid])? as u64;
            }
        }
        {
            let mut del_nodes = tx.prepare_cached("DELETE FROM nodes WHERE rowid = ?1")?;
            for rowid in &rowids {
                del_nodes.execute([rowid])?;
            }
        }

        // Remaining coordinates/clusters are stale after node/edge removal.
        tx.execute("DELETE FROM positions", [])?;
        tx.execute("DELETE FROM clusters", [])?;
        tx.commit()?;
        Ok(stats)
    }

    // ── 基本查询 ────────────────────────────────────────────────

    pub fn node_by_id(&self, id: &str) -> Result<Option<NodeRow>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("SELECT {NODE_COLS} FROM nodes WHERE id = ?1"))?;
        let mut rows = stmt.query_map([id], NodeRow::from_row)?;
        Ok(rows.next().transpose()?)
    }

    /// 名字精确 + 前缀匹配；精确命中排最前，其余按名字长度/字典序。
    pub fn nodes_by_name(&self, name: &str, limit: usize) -> Result<Vec<NodeRow>> {
        let pattern = format!("{}%", escape_like(name));
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {NODE_COLS} FROM nodes \
             WHERE name LIKE ?1 ESCAPE '\\' \
             ORDER BY (name = ?2) DESC, length(name) ASC, name ASC, rowid ASC \
             LIMIT ?3"
        ))?;
        let rows = stmt.query_map(params![pattern, name, limit as i64], NodeRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 按 label 列出节点；`name_contains` 为大小写敏感子串过滤，保持与
    /// GitNexus `CONTAINS` 语义接近。用于 Route/Tool 这类应用语义节点。
    pub fn nodes_by_label(
        &self,
        label: &str,
        name_contains: Option<&str>,
        limit: usize,
    ) -> Result<Vec<NodeRow>> {
        let pattern = name_contains.map(|s| format!("%{}%", escape_like(s)));
        let sql = match pattern.as_ref() {
            Some(_) => format!(
                "SELECT {NODE_COLS} FROM nodes \
                 WHERE label = ?1 AND name LIKE ?2 ESCAPE '\\' \
                 ORDER BY name ASC, rowid ASC LIMIT ?3"
            ),
            None => format!(
                "SELECT {NODE_COLS} FROM nodes \
                 WHERE label = ?1 ORDER BY name ASC, rowid ASC LIMIT ?2"
            ),
        };
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = if let Some(pattern) = pattern {
            stmt.query_map(params![label, pattern, limit as i64], NodeRow::from_row)?
        } else {
            stmt.query_map(params![label, limit as i64], NodeRow::from_row)?
        };
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn incoming_linked_nodes(
        &self,
        target_rowid: i64,
        edge_types: &[&str],
    ) -> Result<Vec<LinkedNodeRow>> {
        self.linked_nodes(target_rowid, edge_types, false)
    }

    pub fn outgoing_linked_nodes(
        &self,
        source_rowid: i64,
        edge_types: &[&str],
    ) -> Result<Vec<LinkedNodeRow>> {
        self.linked_nodes(source_rowid, edge_types, true)
    }

    fn linked_nodes(
        &self,
        rowid: i64,
        edge_types: &[&str],
        outgoing: bool,
    ) -> Result<Vec<LinkedNodeRow>> {
        if edge_types.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = std::iter::repeat_n("?", edge_types.len())
            .collect::<Vec<_>>()
            .join(", ");
        let (edge_anchor, node_side) = if outgoing {
            ("e.source", "e.target")
        } else {
            ("e.target", "e.source")
        };
        let linked_node_cols = "nodes.rowid, nodes.id, nodes.label, nodes.name, nodes.file_path, \
             nodes.start_line, nodes.end_line, nodes.props";
        let sql = format!(
            "SELECT {linked_node_cols}, t.name, e.reason, e.step \
             FROM edges e \
             JOIN edge_types t ON t.type_id = e.type_id \
             JOIN nodes ON nodes.rowid = {node_side} \
             WHERE {edge_anchor} = ?1 AND t.name IN ({placeholders}) \
             ORDER BY t.name ASC, nodes.file_path ASC, nodes.start_line ASC, nodes.rowid ASC"
        );
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(edge_types.len() + 1);
        params_vec.push(&rowid);
        for ty in edge_types {
            params_vec.push(ty);
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_vec.as_slice(), |row| {
            Ok(LinkedNodeRow {
                node: NodeRow::from_row(row)?,
                edge_type: row.get(8)?,
                reason: row.get(9)?,
                step: row.get(10)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 某文件内全部节点，按起始行排序。
    pub fn nodes_in_file(&self, path: &str) -> Result<Vec<NodeRow>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {NODE_COLS} FROM nodes WHERE file_path = ?1 \
             ORDER BY start_line ASC, rowid ASC"
        ))?;
        let rows = stmt.query_map([path], NodeRow::from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 源文件清单：每个含真实定义（start_line 非空）的文件 → 该文件的符号数。
    /// file_path 为 NULL 的聚合节点排除；按 file_path 字典序升序，确定性输出。
    pub fn file_list(&self) -> Result<Vec<(String, u32)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT file_path, COUNT(*) FROM nodes \
             WHERE file_path IS NOT NULL AND start_line IS NOT NULL \
             GROUP BY file_path ORDER BY file_path ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 可源码搜索的文件清单：所有落到图里的文件路径 → 该路径节点数。
    ///
    /// 与 [`Self::file_list`] 不同，这里不要求 `start_line IS NOT NULL`，用于
    /// grep-like search_code 覆盖只有 File/Resource 节点、没有符号定义的源码/配置文件。
    pub fn searchable_file_list(&self) -> Result<Vec<(String, u32)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT file_path, COUNT(*) FROM nodes \
             WHERE file_path IS NOT NULL AND file_path != '' \
             GROUP BY file_path ORDER BY file_path ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u32))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn node_count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    pub fn edge_count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// 边类型字典（type_id 升序）。
    pub fn edge_types(&self) -> Result<Vec<(u16, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT type_id, name FROM edge_types ORDER BY type_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u16, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 全量布局坐标（rowid 升序）。compute_layout 之前为空。
    pub fn positions(&self) -> Result<Vec<PositionRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT node, x, y, size, cluster FROM positions ORDER BY node")?;
        let rows = stmt.query_map([], |r| {
            Ok(PositionRow {
                node: r.get(0)?,
                x: r.get(1)?,
                y: r.get(2)?,
                size: r.get(3)?,
                cluster: r.get::<_, i64>(4)? as u32,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT value FROM meta WHERE key = ?1")?;
        let mut rows = stmt.query_map([key], |r| r.get(0))?;
        Ok(rows.next().transpose()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

/// 转义 LIKE 通配符（`%`、`_`、`\`），转义符约定 `\`。
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("aka-graph-store-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let base = format!("{name}-{}", std::process::id());
        for suffix in ["db", "db-wal", "db-shm"] {
            let _ = std::fs::remove_file(dir.join(format!("{base}.{suffix}")));
        }
        dir.join(format!("{base}.db"))
    }

    /// 旧库（edges 建表早于 step 列）打开时自动 ALTER 补列，且补列后可正常摄取步号。
    #[test]
    fn open_migrates_legacy_edges_table() {
        let path = temp_db("legacy-schema");
        {
            // 手造旧 schema：edges 无 step 列。
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE nodes (
                     id TEXT NOT NULL UNIQUE, label TEXT NOT NULL, name TEXT,
                     file_path TEXT, start_line INTEGER, end_line INTEGER,
                     props TEXT NOT NULL
                 );
                 CREATE TABLE edges (
                     source INTEGER NOT NULL, target INTEGER NOT NULL,
                     type_id INTEGER NOT NULL,
                     confidence REAL NOT NULL DEFAULT 0,
                     reason TEXT NOT NULL DEFAULT ''
                 );",
            )
            .unwrap();
        }

        let mut store = GraphStore::open(&path).unwrap();
        let has_step = {
            let mut stmt = store.conn().prepare("PRAGMA table_info(edges)").unwrap();
            let cols: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            cols.contains(&"step".to_string())
        };
        assert!(has_step, "open legacy db must add edges.step");

        // 迁移后的库能摄取带步号的边，并通过 process 查询读回。
        let nodes = vec![
            NodeRec {
                id: "fn1".into(),
                label: "Function".into(),
                properties: json!({"name": "one"}).as_object().unwrap().clone(),
            },
            NodeRec {
                id: "p1".into(),
                label: "Process".into(),
                properties: json!({"name": "proc", "processType": "call-chain", "stepCount": 1})
                    .as_object()
                    .unwrap()
                    .clone(),
            },
        ];
        let edges = vec![EdgeRec {
            id: "e1".into(),
            source_id: "fn1".into(),
            target_id: "p1".into(),
            edge_type: "STEP_IN_PROCESS".into(),
            confidence: 1.0,
            reason: String::new(),
            step: Some(1),
            evidence: None,
        }];
        let stats = store.ingest(nodes.into_iter(), edges.into_iter()).unwrap();
        assert_eq!(stats.edges, 1);

        let p1 = store.node_by_id("p1").unwrap().unwrap();
        let steps = store.process_steps(p1.rowid).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].id, "fn1");
        assert_eq!(steps[0].step, Some(1));

        // 再次打开：迁移幂等，不会重复 ALTER 报错。
        drop(store);
        let store = GraphStore::open(&path).unwrap();
        assert_eq!(store.edge_count().unwrap(), 1);
    }

    #[test]
    fn delete_file_removes_nodes_incident_edges_and_stale_layout() {
        let mut store = GraphStore::open_in_memory().unwrap();
        let nodes = vec![
            NodeRec {
                id: "a".into(),
                label: "Function".into(),
                properties: json!({"name": "a", "filePath": "src/a.rs", "startLine": 0})
                    .as_object()
                    .unwrap()
                    .clone(),
            },
            NodeRec {
                id: "b".into(),
                label: "Function".into(),
                properties: json!({"name": "b", "filePath": "src/b.rs", "startLine": 0})
                    .as_object()
                    .unwrap()
                    .clone(),
            },
            NodeRec {
                id: "c".into(),
                label: "Function".into(),
                properties: json!({"name": "c", "filePath": "src/b.rs", "startLine": 1})
                    .as_object()
                    .unwrap()
                    .clone(),
            },
        ];
        let edges = vec![
            EdgeRec {
                id: "a-b".into(),
                source_id: "a".into(),
                target_id: "b".into(),
                edge_type: "CALLS".into(),
                confidence: 1.0,
                reason: String::new(),
                step: None,
                evidence: None,
            },
            EdgeRec {
                id: "b-c".into(),
                source_id: "b".into(),
                target_id: "c".into(),
                edge_type: "CALLS".into(),
                confidence: 1.0,
                reason: String::new(),
                step: None,
                evidence: None,
            },
        ];
        store.ingest(nodes.into_iter(), edges.into_iter()).unwrap();
        let adj = crate::Adjacency::build(&store).unwrap();
        crate::compute_layout(&store, &adj).unwrap();
        assert_eq!(store.positions().unwrap().len(), 3);

        let stats = store.delete_file("src/b.rs").unwrap();

        assert_eq!(stats.nodes, 2);
        assert_eq!(stats.edges, 2);
        assert_eq!(store.node_count().unwrap(), 1);
        assert_eq!(store.edge_count().unwrap(), 0);
        assert!(store.node_by_id("a").unwrap().is_some());
        assert!(store.node_by_id("b").unwrap().is_none());
        assert!(store.node_by_id("c").unwrap().is_none());
        assert!(
            store.positions().unwrap().is_empty(),
            "layout tables must be cleared for recompute"
        );
    }
}
