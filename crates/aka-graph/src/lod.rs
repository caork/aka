//! LOD 快照与簇级聚合视图 — 可视化的数据侧。
//!
//! 前端（WebGL）只渲染这里产出的静态坐标，不做力导。
//! JSON 形状（与前端约定）：
//! `{"classes":["Function",...],"nodes":[{"i":0,"id":"...","x":..,"y":..,"s":..,"c":簇,"l":类下标,"name":"..."}],"edges":[s,t,s,t,...]}`

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;

use crate::error::{GraphError, Result};
use crate::store::GraphStore;

/// LOD 快照里的一个节点。
#[derive(Debug, Clone, Serialize)]
pub struct LodNode {
    /// 快照内下标（edges 数组引用它）。
    pub i: u32,
    /// 原始节点 id。
    pub id: String,
    pub x: f32,
    pub y: f32,
    /// 渲染尺寸 = log(1+度数)。
    pub s: f32,
    /// 簇号。
    pub c: u32,
    /// label 在 classes 数组中的下标。
    pub l: u32,
    pub name: String,
}

/// 截断 LOD 快照（可直接 serde 成前端约定 JSON）。
#[derive(Debug, Clone, Serialize)]
pub struct LodGraph {
    /// 出现过的节点 label，按首次出现顺序。
    pub classes: Vec<String>,
    pub nodes: Vec<LodNode>,
    /// 扁平 (s, t) 对，引用 nodes 的 `i`。
    pub edges: Vec<u32>,
    /// 图中总节点数（截断前），UI 据此显示「已渲染 X / 共 Y」。
    pub total_nodes: u64,
    /// 实际返回的节点数（= `nodes.len()`，方便前端不数数组）。
    pub returned_nodes: u64,
}

/// 二进制传输形态：meta JSON + xy 交错坐标 + 扁平边。
#[derive(Debug, Clone)]
pub struct LodBinary {
    /// `{"classes":[...],"count":n,"nodes":[{"i","id","s","c","l","name"},...]}`
    pub meta_json: String,
    /// `[x0, y0, x1, y1, ...]`，与 meta 中 nodes 顺序一致。
    pub positions: Vec<f32>,
    /// 扁平 (s, t) 对。
    pub edges: Vec<u32>,
}

#[derive(Serialize)]
struct LodMetaNode<'a> {
    i: u32,
    id: &'a str,
    s: f32,
    c: u32,
    l: u32,
    name: &'a str,
}

#[derive(Serialize)]
struct LodMeta<'a> {
    classes: &'a [String],
    count: usize,
    nodes: Vec<LodMetaNode<'a>>,
}

/// 簇级聚合视图的超节点。
#[derive(Debug, Clone, Serialize)]
pub struct ClusterNode {
    pub cluster: u32,
    pub label: String,
    /// 簇心坐标。
    pub x: f32,
    pub y: f32,
    /// log(1+成员数)。
    pub s: f32,
    pub count: u32,
}

/// 簇间聚合边（无向，s < t），权重 = 跨簇边条数。
#[derive(Debug, Clone, Serialize)]
pub struct ClusterEdge {
    pub s: u32,
    pub t: u32,
    pub w: u32,
}

/// 簇级聚合视图 — 默认渲染层（数千节点天花板）。
#[derive(Debug, Clone, Serialize)]
pub struct ClusterGraph {
    pub nodes: Vec<ClusterNode>,
    pub edges: Vec<ClusterEdge>,
}

impl GraphStore {
    /// 截断 LOD 快照：节点超过 `max_nodes` 时按度数（size 降序）取 top，
    /// 只保留两端都入选的边。需先 `compute_layout`。
    pub fn lod_snapshot(&self, max_nodes: usize) -> Result<LodGraph> {
        self.ensure_layout()?;
        let conn = self.conn();

        let mut classes: Vec<String> = Vec::new();
        let mut class_index: HashMap<String, u32> = HashMap::new();
        let mut nodes: Vec<LodNode> = Vec::new();
        let mut dense_of_rowid: HashMap<i64, u32> = HashMap::new();
        {
            let mut stmt = conn.prepare_cached(
                "SELECT p.node, n.id, n.label, n.name, p.x, p.y, p.size, p.cluster \
                 FROM positions p JOIN nodes n ON n.rowid = p.node \
                 ORDER BY p.size DESC, p.node ASC LIMIT ?1",
            )?;
            let mut rows = stmt.query([max_nodes as i64])?;
            while let Some(row) = rows.next()? {
                let rowid: i64 = row.get(0)?;
                let id: String = row.get(1)?;
                let label: String = row.get(2)?;
                let name: Option<String> = row.get(3)?;
                let l = *class_index.entry(label.clone()).or_insert_with(|| {
                    classes.push(label);
                    classes.len() as u32 - 1
                });
                let i = nodes.len() as u32;
                dense_of_rowid.insert(rowid, i);
                nodes.push(LodNode {
                    i,
                    name: name.unwrap_or_else(|| id.clone()),
                    id,
                    x: row.get::<_, f64>(4)? as f32,
                    y: row.get::<_, f64>(5)? as f32,
                    s: row.get::<_, f64>(6)? as f32,
                    c: row.get::<_, i64>(7)? as u32,
                    l,
                });
            }
        }

        // 两端都入选的边；(s,t) 去重压渲染量，扫描序保证确定性。
        let mut edges: Vec<u32> = Vec::new();
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        {
            let mut stmt = conn.prepare_cached("SELECT source, target FROM edges")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let (Some(&s), Some(&t)) = (
                    dense_of_rowid.get(&row.get::<_, i64>(0)?),
                    dense_of_rowid.get(&row.get::<_, i64>(1)?),
                ) else {
                    continue;
                };
                if s != t && seen.insert((s, t)) {
                    edges.push(s);
                    edges.push(t);
                }
            }
        }

        let returned_nodes = nodes.len() as u64;
        Ok(LodGraph {
            classes,
            nodes,
            edges,
            total_nodes: self.node_count()?,
            returned_nodes,
        })
    }

    /// LOD 快照的二进制形态：meta JSON + `Vec<f32>` xy 交错 + `Vec<u32>` 扁平边。
    pub fn lod_snapshot_binary(&self, max_nodes: usize) -> Result<LodBinary> {
        let g = self.lod_snapshot(max_nodes)?;
        let mut positions = Vec::with_capacity(g.nodes.len() * 2);
        let mut meta_nodes = Vec::with_capacity(g.nodes.len());
        for n in &g.nodes {
            positions.push(n.x);
            positions.push(n.y);
            meta_nodes.push(LodMetaNode {
                i: n.i,
                id: &n.id,
                s: n.s,
                c: n.c,
                l: n.l,
                name: &n.name,
            });
        }
        let meta = LodMeta {
            classes: &g.classes,
            count: g.nodes.len(),
            nodes: meta_nodes,
        };
        Ok(LodBinary {
            meta_json: serde_json::to_string(&meta)?,
            positions,
            edges: g.edges,
        })
    }

    /// 簇级聚合：每簇一个超节点（坐标 = 簇心，s = log(1+成员数)），
    /// 簇间聚合边权重 = 跨簇边条数（无向合并）。
    pub fn cluster_graph(&self) -> Result<ClusterGraph> {
        self.ensure_layout()?;
        let conn = self.conn();

        let mut nodes: Vec<ClusterNode> = Vec::new();
        {
            let mut stmt = conn.prepare_cached(
                "SELECT cluster, label, cx, cy, count FROM clusters ORDER BY cluster",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let count: i64 = row.get(4)?;
                nodes.push(ClusterNode {
                    cluster: row.get::<_, i64>(0)? as u32,
                    label: row.get(1)?,
                    x: row.get::<_, f64>(2)? as f32,
                    y: row.get::<_, f64>(3)? as f32,
                    s: (1.0 + count as f64).ln() as f32,
                    count: count as u32,
                });
            }
        }

        let mut cluster_of: HashMap<i64, u32> = HashMap::new();
        {
            let mut stmt = conn.prepare_cached("SELECT node, cluster FROM positions")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                cluster_of.insert(row.get(0)?, row.get::<_, i64>(1)? as u32);
            }
        }

        let mut weights: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        {
            let mut stmt = conn.prepare_cached("SELECT source, target FROM edges")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let (Some(&cs), Some(&ct)) = (
                    cluster_of.get(&row.get::<_, i64>(0)?),
                    cluster_of.get(&row.get::<_, i64>(1)?),
                ) else {
                    continue;
                };
                if cs != ct {
                    *weights.entry((cs.min(ct), cs.max(ct))).or_insert(0) += 1;
                }
            }
        }
        let edges = weights
            .into_iter()
            .map(|((s, t), w)| ClusterEdge { s, t, w })
            .collect();

        Ok(ClusterGraph { nodes, edges })
    }

    fn ensure_layout(&self) -> Result<()> {
        let positions: i64 = self
            .conn()
            .query_row("SELECT COUNT(*) FROM positions", [], |r| r.get(0))?;
        if positions == 0 && self.node_count()? > 0 {
            return Err(GraphError::Invalid(
                "layout not computed yet — call compute_layout first".to_string(),
            ));
        }
        Ok(())
    }
}
