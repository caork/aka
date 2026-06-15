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
    /// 每个社区/簇的可读摘要：代表符号、文件、标签来源和聚类质量。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cluster_summaries: Vec<ClusterSummary>,
    /// 原图节点总数。
    pub total_nodes: u64,
    /// 实际返回的聚合节点数（= clusters 数）。
    pub returned_nodes: u64,
}

/// 簇级摘要，供图 UI 在社区节点上展示 GitNexus-like 的标签与质量解释。
#[derive(Debug, Clone, Serialize)]
pub struct ClusterSummary {
    pub cluster: u32,
    /// 布局阶段写入的原始簇名。
    pub label: String,
    /// 调优后的展示标签。
    pub display_label: String,
    /// 标签调优使用的主要证据。
    pub label_basis: Vec<String>,
    pub top_symbols: Vec<ClusterSymbolSummary>,
    pub top_files: Vec<ClusterFileSummary>,
    pub quality: ClusterQuality,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClusterSymbolSummary {
    pub id: String,
    pub name: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    pub score: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClusterFileSummary {
    pub path: String,
    pub nodes: u32,
    pub symbols: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClusterQuality {
    pub cohesion: f32,
    pub boundary_ratio: f32,
    pub internal_edges: u32,
    pub external_edges: u32,
    pub confidence: f32,
    pub explanation: String,
}

#[derive(Debug, Clone)]
struct ClusterMember {
    id: String,
    label: String,
    name: String,
    file_path: Option<String>,
    start_line: Option<u32>,
    size: f32,
    degree: u32,
}

#[derive(Default)]
struct ClusterEdgeStats {
    internal: u32,
    external: u32,
}

#[derive(Default)]
struct FileAccumulator {
    nodes: u32,
    symbols: u32,
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
        let summaries = self.cluster_summaries(&cg)?;
        let summary_by_cluster: HashMap<u32, ClusterSummary> = summaries
            .into_iter()
            .map(|summary| (summary.cluster, summary))
            .collect();
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
                let display_label = summary_by_cluster
                    .get(&node.cluster)
                    .map(|summary| summary.display_label.as_str())
                    .unwrap_or(&node.label);
                cluster_labels.push(display_label.to_owned());
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
                    name: display_label.to_owned(),
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
            cluster_summaries: cg
                .nodes
                .iter()
                .filter_map(|node| summary_by_cluster.get(&node.cluster).cloned())
                .collect(),
            total_nodes: self.node_count()?,
            returned_nodes: cg.nodes.len() as u64,
        })
    }

    fn cluster_summaries(&self, cg: &ClusterGraph) -> Result<Vec<ClusterSummary>> {
        let conn = self.conn();
        let mut degree_of: HashMap<i64, u32> = HashMap::new();
        {
            let mut stmt = conn.prepare_cached(
                "SELECT node, COUNT(*) FROM ( \
                   SELECT source AS node FROM edges \
                   UNION ALL \
                   SELECT target AS node FROM edges \
                 ) GROUP BY node",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                degree_of.insert(row.get(0)?, row.get::<_, i64>(1)? as u32);
            }
        }

        let mut members_by_cluster: BTreeMap<u32, Vec<ClusterMember>> = BTreeMap::new();
        let mut cluster_of: HashMap<i64, u32> = HashMap::new();
        {
            let mut stmt = conn.prepare_cached(
                "SELECT p.cluster, n.rowid, n.id, n.label, COALESCE(n.name, n.id), \
                        n.file_path, n.start_line, p.size \
                 FROM positions p JOIN nodes n ON n.rowid = p.node \
                 ORDER BY p.cluster ASC, p.size DESC, n.rowid ASC",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let cluster = row.get::<_, i64>(0)? as u32;
                let rowid = row.get(1)?;
                cluster_of.insert(rowid, cluster);
                members_by_cluster
                    .entry(cluster)
                    .or_default()
                    .push(ClusterMember {
                        id: row.get(2)?,
                        label: row.get(3)?,
                        name: row.get(4)?,
                        file_path: row.get(5)?,
                        start_line: row.get(6)?,
                        size: row.get::<_, f64>(7)? as f32,
                        degree: degree_of.get(&rowid).copied().unwrap_or(0),
                    });
            }
        }

        let mut edge_stats: HashMap<u32, ClusterEdgeStats> = HashMap::new();
        {
            let mut stmt = conn.prepare_cached("SELECT source, target FROM edges")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let source: i64 = row.get(0)?;
                let target: i64 = row.get(1)?;
                let (Some(&cs), Some(&ct)) = (cluster_of.get(&source), cluster_of.get(&target))
                else {
                    continue;
                };
                if cs == ct {
                    edge_stats.entry(cs).or_default().internal += 1;
                } else {
                    edge_stats.entry(cs).or_default().external += 1;
                    edge_stats.entry(ct).or_default().external += 1;
                }
            }
        }

        let labels_by_cluster: HashMap<u32, &str> = cg
            .nodes
            .iter()
            .map(|node| (node.cluster, node.label.as_str()))
            .collect();

        let mut summaries = Vec::with_capacity(cg.nodes.len());
        for node in &cg.nodes {
            let members = members_by_cluster
                .get(&node.cluster)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let top_symbols = top_symbols(members);
            let top_files = top_files(members);
            let original_label = labels_by_cluster
                .get(&node.cluster)
                .copied()
                .unwrap_or(node.label.as_str());
            let (display_label, label_basis) =
                tune_cluster_label(original_label, &top_files, &top_symbols);
            let quality = cluster_quality(
                edge_stats.get(&node.cluster),
                members.len() as u32,
                &top_files,
            );
            summaries.push(ClusterSummary {
                cluster: node.cluster,
                label: original_label.to_owned(),
                display_label,
                label_basis,
                top_symbols,
                top_files,
                quality,
            });
        }
        Ok(summaries)
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

fn top_symbols(members: &[ClusterMember]) -> Vec<ClusterSymbolSummary> {
    let mut symbols: Vec<_> = members
        .iter()
        .filter(|member| is_symbol_label(&member.label))
        .map(|member| {
            let score = symbol_score(member);
            ClusterSymbolSummary {
                id: member.id.clone(),
                name: member.name.clone(),
                label: member.label.clone(),
                file_path: member.file_path.clone(),
                start_line: member.start_line,
                score,
            }
        })
        .collect();
    symbols.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.file_path.cmp(&b.file_path))
            .then_with(|| a.id.cmp(&b.id))
    });
    symbols.truncate(5);
    symbols
}

fn top_files(members: &[ClusterMember]) -> Vec<ClusterFileSummary> {
    let mut files: BTreeMap<String, FileAccumulator> = BTreeMap::new();
    for member in members {
        let Some(path) = member
            .file_path
            .as_deref()
            .filter(|path| is_plausible_file_path(path))
        else {
            continue;
        };
        let acc = files.entry(path.to_owned()).or_default();
        acc.nodes += 1;
        if is_symbol_label(&member.label) {
            acc.symbols += 1;
        }
    }
    let mut files: Vec<_> = files
        .into_iter()
        .map(|(path, acc)| ClusterFileSummary {
            path,
            nodes: acc.nodes,
            symbols: acc.symbols,
        })
        .collect();
    files.sort_by(|a, b| {
        b.symbols
            .cmp(&a.symbols)
            .then_with(|| b.nodes.cmp(&a.nodes))
            .then_with(|| a.path.cmp(&b.path))
    });
    if files.iter().any(|file| file.symbols > 0) {
        files.retain(|file| file.symbols > 0);
    }
    files.truncate(5);
    files
}

fn symbol_score(member: &ClusterMember) -> u32 {
    let label_weight = match member.label.as_str() {
        "Process" => 22,
        "Class" | "Interface" | "Struct" | "Enum" | "Trait" | "Type" => 18,
        "Function" | "Method" | "Route" | "Tool" | "Job" => 14,
        _ => 5,
    };
    let source_weight = if member.start_line.is_some() { 4 } else { 0 };
    member.degree.saturating_mul(3)
        + (member.size.max(0.0) * 6.0).round() as u32
        + label_weight
        + source_weight
}

fn is_symbol_label(label: &str) -> bool {
    !matches!(
        label,
        "Community"
            | "File"
            | "Folder"
            | "Project"
            | "Repository"
            | "Directory"
            | "Module"
            | "Package"
    )
}

fn is_plausible_file_path(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.is_empty()
        || trimmed.contains('{')
        || trimmed.contains('}')
        || matches!(
            trimmed,
            "." | "/" | "(root)" | "src" | "lib" | "app" | "apps"
        )
    {
        return false;
    }
    let normalized = trimmed.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or(trimmed);
    name.contains('.')
        || matches!(
            name,
            "BUILD" | "WORKSPACE" | "BUCK" | "Makefile" | "Dockerfile" | "Containerfile"
        )
}

fn tune_cluster_label(
    original: &str,
    top_files: &[ClusterFileSummary],
    top_symbols: &[ClusterSymbolSummary],
) -> (String, Vec<String>) {
    let file_label = derive_file_label(top_files);
    let symbol_label = derive_symbol_label(top_symbols);
    let original_label = polish_label(original);
    let original_is_generic = is_generic_cluster_label(original);

    let mut basis = Vec::new();
    let display = if !original_is_generic {
        basis.push(format!("label:{original}"));
        original_label
    } else if let Some((file_label, file_is_generic)) = file_label {
        if file_is_generic {
            if let Some(symbol_label) = symbol_label {
                basis.push(format!("file:{}", top_files[0].path));
                basis.push(format!("symbol:{}", top_symbols[0].name));
                symbol_label
            } else {
                basis.push(format!("file:{}", top_files[0].path));
                file_label
            }
        } else if let Some(symbol_label) = symbol_label
            .as_deref()
            .filter(|symbol| !labels_overlap(&file_label, symbol))
        {
            basis.push(format!("file:{}", top_files[0].path));
            basis.push(format!("symbol:{}", top_symbols[0].name));
            compact_label(&format!("{file_label} {symbol_label}"))
        } else {
            basis.push(format!("file:{}", top_files[0].path));
            file_label
        }
    } else if let Some(symbol_label) = symbol_label {
        basis.push(format!("symbol:{}", top_symbols[0].name));
        symbol_label
    } else {
        basis.push(format!("label:{original}"));
        original_label
    };

    (display, basis)
}

fn derive_file_label(top_files: &[ClusterFileSummary]) -> Option<(String, bool)> {
    let first = top_files.first()?;
    let (first_component, generic) = best_path_component(&first.path)?;
    let first_component = title_case_words(&split_identifier(&first_component));
    if first_component.is_empty() {
        None
    } else {
        Some((compact_label(&first_component), generic))
    }
}

fn best_path_component(path: &str) -> Option<(String, bool)> {
    let clean = path.replace('\\', "/");
    let mut parts: Vec<&str> = clean.split('/').filter(|part| !part.is_empty()).collect();
    let file = parts.pop()?;
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    if !is_generic_path_part(stem) {
        return Some((stem.to_owned(), false));
    }
    parts
        .into_iter()
        .rev()
        .find(|part| !is_generic_path_part(part))
        .map(|part| (part.to_owned(), false))
        .or_else(|| Some((stem.to_owned(), true)))
}

fn derive_symbol_label(top_symbols: &[ClusterSymbolSummary]) -> Option<String> {
    let mut weights: BTreeMap<String, u32> = BTreeMap::new();
    for symbol in top_symbols.iter().take(5) {
        for token in split_identifier(&symbol.name) {
            if token.len() < 3 || is_generic_token(&token) {
                continue;
            }
            *weights.entry(token).or_default() += symbol.score.max(1);
        }
    }
    let mut tokens: Vec<_> = weights.into_iter().collect();
    tokens.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let words: Vec<_> = tokens.into_iter().take(2).map(|(token, _)| token).collect();
    if words.is_empty() {
        None
    } else {
        Some(compact_label(&title_case_words(&words)))
    }
}

fn cluster_quality(
    stats: Option<&ClusterEdgeStats>,
    member_count: u32,
    top_files: &[ClusterFileSummary],
) -> ClusterQuality {
    let internal_edges = stats.map_or(0, |s| s.internal);
    let external_edges = stats.map_or(0, |s| s.external);
    let edge_total = internal_edges + external_edges;
    let cohesion = if edge_total == 0 {
        1.0
    } else {
        ratio(internal_edges, edge_total)
    };
    let boundary_ratio = ratio(external_edges, edge_total);
    let top_file_ratio = top_files
        .first()
        .map(|file| ratio(file.nodes, member_count.max(1)))
        .unwrap_or(0.0);
    let scale = (member_count as f32 / 20.0).min(1.0);
    let confidence = (cohesion * 0.45 + top_file_ratio * 0.35 + scale * 0.20).clamp(0.0, 1.0);
    let explanation = if edge_total == 0 {
        if let Some(file) = top_files.first() {
            format!(
                "No graph edges inside this community yet; {:.0}% of nodes come from {}.",
                top_file_ratio * 100.0,
                file.path
            )
        } else {
            "No graph edges or source-file concentration available yet.".to_string()
        }
    } else if let Some(file) = top_files.first() {
        format!(
            "{:.0}% internal edges, {:.0}% boundary edges; strongest file signal is {}.",
            cohesion * 100.0,
            boundary_ratio * 100.0,
            file.path
        )
    } else {
        format!(
            "{:.0}% internal edges and {:.0}% boundary edges across this community.",
            cohesion * 100.0,
            boundary_ratio * 100.0
        )
    };

    ClusterQuality {
        cohesion,
        boundary_ratio,
        internal_edges,
        external_edges,
        confidence,
        explanation,
    }
}

fn ratio(numerator: u32, denominator: u32) -> f32 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f32 / denominator as f32
    }
}

fn is_generic_cluster_label(label: &str) -> bool {
    let lower = label.trim().to_ascii_lowercase();
    lower.is_empty()
        || matches!(
            lower.as_str(),
            "(root)"
                | "root"
                | "src"
                | "source"
                | "app"
                | "apps"
                | "lib"
                | "libs"
                | "core"
                | "module"
                | "modules"
                | "package"
                | "packages"
                | "community"
                | "cluster"
                | "group"
        )
        || lower.strip_prefix("community").is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .chars()
                    .all(|ch| ch == '-' || ch == '_' || ch.is_ascii_alphanumeric())
        })
        || lower.strip_prefix("cluster").is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .chars()
                    .all(|ch| ch == '-' || ch == '_' || ch.is_ascii_alphanumeric())
        })
}

fn is_generic_path_part(part: &str) -> bool {
    let lower = part.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "" | "."
            | "src"
            | "source"
            | "app"
            | "apps"
            | "lib"
            | "libs"
            | "components"
            | "component"
            | "pages"
            | "page"
            | "routes"
            | "route"
            | "models"
            | "model"
            | "utils"
            | "util"
            | "helpers"
            | "helper"
            | "index"
            | "mod"
            | "main"
            | "types"
            | "type"
    )
}

fn is_generic_token(token: &str) -> bool {
    matches!(
        token,
        "get"
            | "set"
            | "new"
            | "run"
            | "main"
            | "init"
            | "load"
            | "save"
            | "read"
            | "write"
            | "build"
            | "create"
            | "update"
            | "delete"
            | "remove"
            | "handle"
            | "process"
            | "data"
            | "node"
            | "nodes"
            | "edge"
            | "edges"
            | "file"
            | "files"
            | "type"
            | "types"
            | "item"
            | "items"
            | "value"
            | "values"
    )
}

fn labels_overlap(left: &str, right: &str) -> bool {
    let left_tokens: HashSet<_> = split_identifier(left).into_iter().collect();
    split_identifier(right)
        .into_iter()
        .any(|token| left_tokens.contains(token.as_str()))
}

fn polish_label(label: &str) -> String {
    let words = split_identifier(label);
    if words.is_empty() {
        "Community".to_string()
    } else {
        compact_label(&title_case_words(&words))
    }
}

fn compact_label(label: &str) -> String {
    const MAX_CHARS: usize = 34;
    let mut out = label.trim().to_string();
    if out.chars().count() <= MAX_CHARS {
        return out;
    }
    out = out.chars().take(MAX_CHARS - 3).collect();
    out.push_str("...");
    out
}

fn split_identifier(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut prev_lower_or_digit = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            let is_upper = ch.is_ascii_uppercase();
            if is_upper && prev_lower_or_digit && !current.is_empty() {
                words.push(current.to_ascii_lowercase());
                current.clear();
            }
            current.push(ch);
            prev_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            if !current.is_empty() {
                words.push(current.to_ascii_lowercase());
                current.clear();
            }
            prev_lower_or_digit = false;
        }
    }
    if !current.is_empty() {
        words.push(current.to_ascii_lowercase());
    }
    words
}

fn title_case_words(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    let mut out = String::new();
                    out.extend(first.to_uppercase());
                    out.push_str(chars.as_str());
                    out
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
