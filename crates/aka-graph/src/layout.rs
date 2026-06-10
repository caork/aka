//! 确定性静态布局 — 两级 phyllotaxis（葵花籽螺旋）。
//!
//! 1. 聚类：优先用图里现成的 Community 节点（MEMBER_OF 边）；
//!    不在任何社区的节点按 file_path 顶层目录聚，无路径的归 "(root)"。
//! 2. 簇中心按权重（成员数）降序做 phyllotaxis，簇间距随累计面积增长，
//!    与簇半径成正比避免重叠；簇内成员按度数降序再做局部 phyllotaxis，
//!    度数高的靠簇心。
//!
//! 全程无随机数，输入相同输出逐位一致。结果写回 positions / clusters 表。

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::params;

use crate::adjacency::Adjacency;
use crate::error::Result;
use crate::store::GraphStore;

/// 黄金角（弧度）。
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;
/// 簇内相邻成员的基准间距。
const MEMBER_SPACING: f64 = 10.0;
/// 簇间打包系数：簇心间距 ≥ PACK × 簇半径，防重叠。
const CLUSTER_PACK: f64 = 2.0;

struct Cluster {
    label: String,
    /// 排序稳定性兜底（label 可能撞名）。
    orig_index: usize,
    members: Vec<u32>,
    radius: f64,
    center: (f64, f64),
}

/// 计算全图静态布局并写回 positions / clusters 表（覆盖旧结果）。
pub fn compute_layout(store: &GraphStore, adj: &Adjacency) -> Result<()> {
    let n = adj.len() as usize;
    let conn = store.conn();

    // ── 1. 聚类 ────────────────────────────────────────────────
    // Community 节点（rowid 升序 → 社区簇的确定性编号）。
    let mut communities: Vec<(u32, String)> = Vec::new(); // (稠密下标, 簇标签)
    {
        let mut stmt = conn.prepare(
            "SELECT id, name FROM nodes WHERE label = 'Community' ORDER BY rowid",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let name: Option<String> = row.get(1)?;
            if let Some(dense) = adj.index_of_id(&id) {
                communities.push((dense, name.unwrap_or(id)));
            }
        }
    }
    let comm_index: HashMap<u32, u32> = communities
        .iter()
        .enumerate()
        .map(|(i, &(dense, _))| (dense, i as u32))
        .collect();
    let comm_set: HashSet<u32> = comm_index.keys().copied().collect();

    // 成员归属：member -[MEMBER_OF]-> community；多社区取最小簇号（确定性）。
    let mut assign: Vec<Option<u32>> = vec![None; n];
    for &(dense, _) in &communities {
        assign[dense as usize] = Some(comm_index[&dense]);
    }
    if let Some(member_of) = adj.type_id("MEMBER_OF") {
        for u in 0..n as u32 {
            if comm_set.contains(&u) {
                continue;
            }
            for (v, ty) in adj.out_edges(u) {
                if ty != member_of {
                    continue;
                }
                if let Some(&ci) = comm_index.get(&v) {
                    let slot = &mut assign[u as usize];
                    *slot = Some(slot.map_or(ci, |cur| cur.min(ci)));
                }
            }
        }
    }

    // 落单节点按 file_path 顶层目录聚（BTreeMap 保证目录序确定）。
    let mut file_paths: Vec<Option<String>> = vec![None; n];
    {
        let mut stmt =
            conn.prepare("SELECT id, file_path FROM nodes WHERE file_path IS NOT NULL")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            if let Some(dense) = adj.index_of_id(&id) {
                file_paths[dense as usize] = Some(row.get(1)?);
            }
        }
    }
    let mut dir_groups: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut comm_members: Vec<Vec<u32>> = vec![Vec::new(); communities.len()];
    for u in 0..n as u32 {
        match assign[u as usize] {
            Some(ci) => comm_members[ci as usize].push(u),
            None => {
                let key = file_paths[u as usize]
                    .as_deref()
                    .map(top_dir)
                    .filter(|d| !d.is_empty())
                    .unwrap_or("(root)")
                    .to_owned();
                dir_groups.entry(key).or_default().push(u);
            }
        }
    }

    let mut clusters: Vec<Cluster> = Vec::new();
    for (ci, (_, label)) in communities.iter().enumerate() {
        let mut members = std::mem::take(&mut comm_members[ci]);
        // 社区节点本身固定排第一位（簇心），其余按度数降序。
        let comm_node = communities[ci].0;
        sort_members(&mut members, adj, Some(comm_node));
        clusters.push(Cluster {
            label: label.clone(),
            orig_index: ci,
            members,
            radius: 0.0,
            center: (0.0, 0.0),
        });
    }
    for (label, mut members) in dir_groups {
        sort_members(&mut members, adj, None);
        let orig_index = clusters.len();
        clusters.push(Cluster {
            label,
            orig_index,
            members,
            radius: 0.0,
            center: (0.0, 0.0),
        });
    }

    // ── 2. 簇中心 phyllotaxis ─────────────────────────────────
    // 权重 = 成员数，降序；标签 + 原始序号兜底保证全序确定。
    clusters.sort_by(|a, b| {
        b.members
            .len()
            .cmp(&a.members.len())
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.orig_index.cmp(&b.orig_index))
    });
    let mut acc_area = 0.0f64;
    for (i, cl) in clusters.iter_mut().enumerate() {
        cl.radius = MEMBER_SPACING * (cl.members.len() as f64).sqrt() + MEMBER_SPACING;
        acc_area += (cl.radius * CLUSTER_PACK).powi(2);
        let r = acc_area.sqrt();
        let theta = i as f64 * GOLDEN_ANGLE;
        cl.center = (r * theta.cos(), r * theta.sin());
    }

    // ── 3. 簇内成员 phyllotaxis（rayon 特性开启时并行；按簇独立、收集保序，
    //      两种路径输出逐位一致） ──
    let rows = member_rows(&clusters, adj);

    // ── 4. 写回 ───────────────────────────────────────────────
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM positions", [])?;
    tx.execute("DELETE FROM clusters", [])?;
    {
        let mut ins_pos = tx.prepare_cached(
            "INSERT INTO positions (node, x, y, size, cluster) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for (rowid, x, y, size, cluster) in &rows {
            ins_pos.execute(params![rowid, x, y, size, cluster])?;
        }
        let mut ins_cl = tx.prepare_cached(
            "INSERT INTO clusters (cluster, label, cx, cy, count) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for (ci, cl) in clusters.iter().enumerate() {
            ins_cl.execute(params![
                ci as u32,
                cl.label,
                cl.center.0,
                cl.center.1,
                cl.members.len() as i64
            ])?;
        }
    }
    tx.commit()?;
    store.meta_set("layout_version", "1")?;
    Ok(())
}

/// 单个簇内成员的 (rowid, x, y, size, cluster) 行。
fn cluster_member_rows<'a>(
    ci: usize,
    cl: &'a Cluster,
    adj: &'a Adjacency,
) -> impl Iterator<Item = (i64, f64, f64, f64, u32)> + 'a {
    cl.members.iter().enumerate().map(move |(k, &u)| {
        let r = MEMBER_SPACING * (k as f64).sqrt();
        let theta = k as f64 * GOLDEN_ANGLE;
        let x = cl.center.0 + r * theta.cos();
        let y = cl.center.1 + r * theta.sin();
        let size = (1.0 + f64::from(adj.degree(u))).ln();
        (adj.rowid_of(u), x, y, size, ci as u32)
    })
}

#[cfg(feature = "rayon")]
fn member_rows(clusters: &[Cluster], adj: &Adjacency) -> Vec<(i64, f64, f64, f64, u32)> {
    use rayon::prelude::*;
    clusters
        .par_iter()
        .enumerate()
        .flat_map_iter(|(ci, cl)| cluster_member_rows(ci, cl, adj))
        .collect()
}

#[cfg(not(feature = "rayon"))]
fn member_rows(clusters: &[Cluster], adj: &Adjacency) -> Vec<(i64, f64, f64, f64, u32)> {
    clusters
        .iter()
        .enumerate()
        .flat_map(|(ci, cl)| cluster_member_rows(ci, cl, adj))
        .collect()
}

/// 成员排序：可选的固定首位（社区节点），其余 (度数降序, 稠密下标升序)。
fn sort_members(members: &mut [u32], adj: &Adjacency, first: Option<u32>) {
    members.sort_unstable_by_key(|&u| {
        let pinned = first == Some(u);
        (!pinned, std::cmp::Reverse(adj.degree(u)), u)
    });
}

/// file_path 顶层目录；根级文件返回空串（调用方归入 "(root)"）。
fn top_dir(path: &str) -> &str {
    let p = path
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_start_matches('\\');
    match p.find(['/', '\\']) {
        Some(i) => &p[..i],
        None => "",
    }
}
