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

/// GraphJSON 同形的簇级总览：每个 Community/Cluster 是一个可渲染节点。
#[derive(Debug, Clone, Serialize)]
pub struct ClusterLodGraph {
    pub classes: Vec<String>,
    pub nodes: Vec<LodNode>,
    /// 扁平 (s, t) 对，引用 nodes 的 `i`。
    pub edges: Vec<u32>,
    /// 与 `edges` 的 pair 一一对应；权重 = 簇间聚合边条数。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edge_weights: Vec<u32>,
    /// `nodes[].c` 对应的真实簇名，供前端复用 GraphJSON 时显示标签。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cluster_labels: Vec<String>,
    /// 原图节点总数。
    pub total_nodes: u64,
    /// 实际返回的聚合节点数（= clusters 数）。
    pub returned_nodes: u64,
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

    /// GraphJSON 同形的簇级总览：每簇一个节点，边带聚合权重。
    pub fn cluster_lod_snapshot(&self) -> Result<ClusterLodGraph> {
        let cg = self.cluster_graph()?;
        let mut dense_of_cluster: HashMap<u32, u32> = HashMap::new();
        for (i, node) in cg.nodes.iter().enumerate() {
            dense_of_cluster.insert(node.cluster, i as u32);
        }

        let mut incident_weight: HashMap<u32, u32> = HashMap::new();
        for edge in &cg.edges {
            *incident_weight.entry(edge.s).or_insert(0) += edge.w;
            *incident_weight.entry(edge.t).or_insert(0) += edge.w;
        }

        let classes = vec!["Community".to_string()];
        let mut cluster_labels = Vec::with_capacity(cg.nodes.len());
        let nodes = cg
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                cluster_labels.push(node.label.clone());
                LodNode {
                    i: i as u32,
                    id: format!("cluster:{}", node.cluster),
                    x: node.x,
                    y: node.y,
                    s: cluster_overview_size(
                        node.count,
                        incident_weight.get(&node.cluster).copied().unwrap_or(0),
                    ),
                    c: i as u32,
                    l: 0,
                    name: node.label.clone(),
                }
            })
            .collect();

        let mut edges = Vec::with_capacity(cg.edges.len() * 2);
        let mut edge_weights = Vec::with_capacity(cg.edges.len());
        for edge in &cg.edges {
            let (Some(&s), Some(&t)) =
                (dense_of_cluster.get(&edge.s), dense_of_cluster.get(&edge.t))
            else {
                continue;
            };
            edges.push(s);
            edges.push(t);
            edge_weights.push(edge.w);
        }

        Ok(ClusterLodGraph {
            classes,
            nodes,
            edges,
            edge_weights,
            cluster_labels,
            total_nodes: self.node_count()?,
            returned_nodes: cg.nodes.len() as u64,
        })
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

fn cluster_overview_size(count: u32, incident_weight: u32) -> f32 {
    let count_term = (1.0 + f64::from(count)).ln() * 1.35;
    let edge_term = (1.0 + f64::from(incident_weight)).ln() * 0.35;
    (1.0 + count_term + edge_term) as f32
}
