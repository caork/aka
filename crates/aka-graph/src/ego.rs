//! Ego 子图 — 以某节点为中心的 BFS 邻域 + 径向确定性布局。
//!
//! 输出与 [`crate::lod::LodGraph`] 完全同形（前端中心化重渲直接复用渲染管线）：
//! - 中心节点固定在原点 (0,0)，`i = 0`；
//! - 邻居按 BFS 深度分环，环半径 = 深度 × [`EGO_RING_STEP`]；
//! - 同环内按度数降序排，均匀角分布（每环加黄金角偏移避免径向对齐）；
//! - `c` 字段复用为 BFS 深度（环号），`i` 从 0 紧凑重编号；
//! - 全程无随机数，输入相同输出逐位一致。

use std::collections::HashSet;

use crate::adjacency::Adjacency;
use crate::error::{GraphError, Result};
use crate::lod::{LodGraph, LodNode};
use crate::store::GraphStore;

/// 相邻 BFS 环之间的半径步长。
pub const EGO_RING_STEP: f32 = 100.0;

/// 黄金角（弧度）——逐环角度偏移用。
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

impl GraphStore {
    /// 以 `center_id` 为中心的 ego 子图：无向 BFS（正反邻接都走）收集到 `depth` 跳，
    /// 每层按度数截断到剩余预算（总节点数 ≤ `max_nodes`），
    /// 子图边 = 两端都入选的边（去重、剔自环）。
    pub fn ego_graph(
        &self,
        adj: &Adjacency,
        center_id: &str,
        depth: u32,
        max_nodes: usize,
    ) -> Result<LodGraph> {
        let Some(center) = adj.index_of_id(center_id) else {
            return Err(GraphError::Invalid(format!(
                "ego center node not found: {center_id}"
            )));
        };
        if max_nodes == 0 {
            return Err(GraphError::Invalid("max_nodes must be > 0".into()));
        }

        // ── BFS 分环收集（无向）─────────────────────────────────
        // rings[d] = 第 d 环的稠密下标（d=0 只有 center）。
        let mut rings: Vec<Vec<u32>> = vec![vec![center]];
        let mut selected: HashSet<u32> = HashSet::from([center]);
        let mut budget = max_nodes.saturating_sub(1);

        for _ in 0..depth {
            if budget == 0 {
                break;
            }
            let frontier = rings.last().expect("rings non-empty");
            // 收集本层候选（去重，发现顺序无关紧要——随后全排序）。
            let mut layer_set: HashSet<u32> = HashSet::new();
            for &u in frontier {
                for (v, _) in adj.out_edges(u) {
                    if !selected.contains(&v) {
                        layer_set.insert(v);
                    }
                }
                for (v, _) in adj.in_edges(u) {
                    if !selected.contains(&v) {
                        layer_set.insert(v);
                    }
                }
            }
            if layer_set.is_empty() {
                break;
            }
            // 度数降序、稠密下标升序兜底 → 确定性；截断到剩余预算。
            let mut layer: Vec<u32> = layer_set.into_iter().collect();
            layer.sort_unstable_by_key(|&v| (std::cmp::Reverse(adj.degree(v)), v));
            layer.truncate(budget);
            budget -= layer.len();
            selected.extend(layer.iter().copied());
            rings.push(layer);
        }

        // ── 节点元数据 + 径向布局 ───────────────────────────────
        let conn = self.conn();
        let mut meta_stmt =
            conn.prepare_cached("SELECT label, name FROM nodes WHERE rowid = ?1")?;

        let mut classes: Vec<String> = Vec::new();
        let mut class_index: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        let mut nodes: Vec<LodNode> = Vec::new();
        // 稠密下标 → 紧凑 i。
        let mut compact: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();

        for (d, ring) in rings.iter().enumerate() {
            let radius = d as f32 * EGO_RING_STEP;
            let count = ring.len();
            for (k, &u) in ring.iter().enumerate() {
                let rowid = adj.rowid_of(u);
                let (label, name): (String, Option<String>) =
                    meta_stmt.query_row([rowid], |r| Ok((r.get(0)?, r.get(1)?)))?;
                let l = *class_index.entry(label.clone()).or_insert_with(|| {
                    classes.push(label);
                    classes.len() as u32 - 1
                });
                let (x, y) = if d == 0 {
                    (0.0, 0.0)
                } else {
                    // 均匀角分布 + 逐环黄金角偏移（避免环间径向排成直线）。
                    let theta = k as f64 * std::f64::consts::TAU / count as f64
                        + d as f64 * GOLDEN_ANGLE;
                    (
                        radius * theta.cos() as f32,
                        radius * theta.sin() as f32,
                    )
                };
                let i = nodes.len() as u32;
                compact.insert(u, i);
                let id = adj.id_of(u).to_string();
                nodes.push(LodNode {
                    i,
                    name: name.unwrap_or_else(|| id.clone()),
                    id,
                    x,
                    y,
                    s: (1.0 + f64::from(adj.degree(u))).ln() as f32,
                    c: d as u32,
                    l,
                });
            }
        }

        // ── 子图边：两端都入选；(s,t) 去重、剔自环，按 i 序扫描保证确定性 ──
        let mut edges: Vec<u32> = Vec::new();
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        let mut order: Vec<u32> = rings.iter().flatten().copied().collect();
        order.sort_unstable_by_key(|u| compact[u]);
        for &u in &order {
            let s = compact[&u];
            for (v, _) in adj.out_edges(u) {
                let Some(&t) = compact.get(&v) else { continue };
                if s != t && seen.insert((s, t)) {
                    edges.push(s);
                    edges.push(t);
                }
            }
        }

        Ok(LodGraph {
            classes,
            nodes,
            edges,
        })
    }
}
