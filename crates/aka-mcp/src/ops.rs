//! 工具输出的线上格式（紧凑、token 友好：数组 + 短字段名）与聚合逻辑。
//!
//! MCP 工具和 aka-server 的 HTTP API 共用这里的 DTO / 聚合函数，
//! 保证两个面输出一致。

use std::collections::{BTreeMap, HashSet};

use serde::Serialize;

use crate::backend::{Backend, ProcessHit, RepoInfo, SearchHit, SymbolRef};

/// 检索命中（短字段名版）。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct HitOut {
    pub id: String,
    pub name: String,
    pub label: String,
    /// 切块类型（ast-function / char …），命中来自 chunk 时携带。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub file: String,
    pub line: u32,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snip: Option<String>,
    /// 命中符号所属的流程名（最多 [`MAX_HIT_PROCESS_NAMES`] 个）；
    /// 没有流程归属时整个字段省略（token 友好）。只有 query 填它。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processes: Option<Vec<String>>,
}

impl From<SearchHit> for HitOut {
    fn from(h: SearchHit) -> Self {
        Self {
            id: h.node_id,
            name: h.name,
            label: h.label,
            kind: h.kind,
            file: h.file_path,
            line: h.start_line,
            score: (h.score * 1000.0).round() / 1000.0,
            snip: h.snippet,
            processes: None,
        }
    }
}

/// 图遍历引用（短字段名版）。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RefOut {
    pub id: String,
    pub name: String,
    pub label: String,
    pub file: String,
    pub line: u32,
    pub edge: String,
    pub depth: u32,
}

impl From<SymbolRef> for RefOut {
    fn from(r: SymbolRef) -> Self {
        Self {
            id: r.node_id,
            name: r.name,
            label: r.label,
            file: r.file_path,
            line: r.start_line,
            edge: r.edge_type,
            depth: r.depth,
        }
    }
}

/// 仓库来源（合同：`{"kind":"local|git|zip","url":string|null}`）。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SourceOut {
    pub kind: String,
    /// 合同要求显式 null，不省略。
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RepoOut {
    pub name: String,
    pub path: String,
    pub nodes: u64,
    pub edges: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_at: Option<u64>,
    pub embeddings: bool,
    /// `ready` / `indexing` / `failed`。
    pub status: String,
    pub source: SourceOut,
    /// 失败原因等补充信息；合同要求显式 null。
    pub detail: Option<String>,
    /// 渲染节点预算；合同要求显式 null（null = 默认 50_000）。
    pub render_max_nodes: Option<u32>,
}

impl From<RepoInfo> for RepoOut {
    fn from(r: RepoInfo) -> Self {
        Self {
            name: r.name,
            path: r.path,
            nodes: r.nodes,
            edges: r.edges,
            indexed_at: r.indexed_at,
            embeddings: r.embeddings_enabled,
            status: r.status,
            source: SourceOut {
                kind: r.source_kind,
                url: r.source_url,
            },
            detail: r.detail,
            render_max_nodes: r.render_max_nodes,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ReposOut {
    pub repos: Vec<RepoOut>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct QueryOut {
    pub hits: Vec<HitOut>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DefsOut {
    pub defs: Vec<HitOut>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RefsOut {
    pub refs: Vec<RefOut>,
}

/// 符号所属执行流程（线上形状与 [`ProcessHit`] 一一对应，字段名原样）。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ProcessOut {
    pub process_id: String,
    pub name: String,
    pub process_type: String,
    pub step: Option<u32>,
    pub step_count: Option<u32>,
}

impl From<ProcessHit> for ProcessOut {
    fn from(p: ProcessHit) -> Self {
        Self {
            process_id: p.process_id,
            name: p.name,
            process_type: p.process_type,
            step: p.step,
            step_count: p.step_count,
        }
    }
}

/// impact 的流程视角聚合：哪条执行流会断、断在第几步、波及几个符号。
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct AffectedProcessOut {
    pub process_id: String,
    pub name: String,
    pub process_type: String,
    pub step_count: Option<u32>,
    /// 流程内所有受影响符号步号的最小值——执行流最早断在这一步。
    pub first_affected_step: Option<u32>,
    /// 该流程内受影响符号数（含目标符号自身）。
    pub affected_symbols: usize,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ImpactOut {
    pub impacted: Vec<RefOut>,
    pub count: usize,
    /// 按 affected_symbols 降序；同数按 process_id 升序保证确定性。
    pub affected_processes: Vec<AffectedProcessOut>,
}

/// 一个符号的 360° 上下文。
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ContextOut {
    pub symbol: String,
    pub defs: Vec<HitOut>,
    pub callers: Vec<RefOut>,
    pub callees: Vec<RefOut>,
    pub refs: Vec<RefOut>,
    /// 目标符号的流程归属（多定义聚合，按 process_id 去重）。
    pub processes: Vec<ProcessOut>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AugmentItem {
    pub hit: HitOut,
    pub callers: Vec<RefOut>,
    pub callees: Vec<RefOut>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AugmentOut {
    pub items: Vec<AugmentItem>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct AnalyzeOut {
    pub summary: String,
}

pub const DEFAULT_QUERY_LIMIT: usize = 10;
pub const MAX_QUERY_LIMIT: usize = 100;
pub const DEFAULT_REFS_LIMIT: usize = 25;
pub const DEFAULT_IMPACT_DEPTH: u32 = 2;
pub const DEFAULT_IMPACT_LIMIT: usize = 50;
pub const CONTEXT_NEIGHBOR_DEPTH: u32 = 1;
pub const AUGMENT_TOP_K: usize = 3;
/// query 每条命中最多带几个流程名（再多就靠 context / node 详情看全量）。
pub const MAX_HIT_PROCESS_NAMES: usize = 3;

pub fn list_repos(b: &dyn Backend) -> anyhow::Result<ReposOut> {
    Ok(ReposOut {
        repos: b.list_repos()?.into_iter().map(Into::into).collect(),
    })
}

pub fn query(
    b: &dyn Backend,
    repo: Option<&str>,
    query: &str,
    limit: usize,
) -> anyhow::Result<QueryOut> {
    let mut hits = Vec::new();
    for h in b.search(repo, query, limit)? {
        let procs = b.processes_of(repo, &h.node_id)?;
        let mut hit = HitOut::from(h);
        if !procs.is_empty() {
            hit.processes = Some(
                procs
                    .into_iter()
                    .take(MAX_HIT_PROCESS_NAMES)
                    .map(|p| p.name)
                    .collect(),
            );
        }
        hits.push(hit);
    }
    Ok(QueryOut { hits })
}

pub fn find_definition(
    b: &dyn Backend,
    repo: Option<&str>,
    symbol: &str,
) -> anyhow::Result<DefsOut> {
    Ok(DefsOut {
        defs: b.find_definition(repo, symbol)?.into_iter().map(Into::into).collect(),
    })
}

pub fn references(
    b: &dyn Backend,
    repo: Option<&str>,
    symbol: &str,
    limit: usize,
) -> anyhow::Result<RefsOut> {
    Ok(RefsOut {
        refs: b.references(repo, symbol, limit)?.into_iter().map(Into::into).collect(),
    })
}

pub fn impact(
    b: &dyn Backend,
    repo: Option<&str>,
    symbol: &str,
    depth: u32,
    limit: usize,
) -> anyhow::Result<ImpactOut> {
    let refs = b.impact(repo, symbol, depth, limit)?;
    // 受影响节点集合 = 目标符号自身（所有定义）+ 影响面内全部符号，去重后再
    // 做流程聚合，避免同一符号在某流程里被数两次。
    let mut node_ids: Vec<String> = b
        .find_definition(repo, symbol)?
        .into_iter()
        .map(|h| h.node_id)
        .collect();
    node_ids.extend(refs.iter().map(|r| r.node_id.clone()));
    node_ids.sort();
    node_ids.dedup();

    // BTreeMap 按 process_id 有序，聚合结果与输入顺序无关（确定性输出）。
    let mut agg: BTreeMap<String, AffectedProcessOut> = BTreeMap::new();
    for id in &node_ids {
        for p in b.processes_of(repo, id)? {
            let entry = agg.entry(p.process_id.clone()).or_insert(AffectedProcessOut {
                process_id: p.process_id,
                name: p.name,
                process_type: p.process_type,
                step_count: p.step_count,
                first_affected_step: None,
                affected_symbols: 0,
            });
            entry.affected_symbols += 1;
            // 最早断点 = 所有受影响符号步号的最小值（无步号的符号不参与）。
            entry.first_affected_step = match (entry.first_affected_step, p.step) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
    }
    let mut affected_processes: Vec<AffectedProcessOut> = agg.into_values().collect();
    affected_processes.sort_by(|a, b| {
        b.affected_symbols
            .cmp(&a.affected_symbols)
            .then_with(|| a.process_id.cmp(&b.process_id))
    });

    let impacted: Vec<RefOut> = refs.into_iter().map(Into::into).collect();
    let count = impacted.len();
    Ok(ImpactOut { impacted, count, affected_processes })
}

/// definition + callers + callees + references 拼成一个结构化结果。
pub fn context(b: &dyn Backend, repo: Option<&str>, symbol: &str) -> anyhow::Result<ContextOut> {
    let defs: Vec<HitOut> =
        b.find_definition(repo, symbol)?.into_iter().map(Into::into).collect();
    // 流程归属：符号可能重名多定义，全部聚合后按 process_id 去重。
    let mut seen: HashSet<String> = HashSet::new();
    let mut processes: Vec<ProcessOut> = Vec::new();
    for d in &defs {
        for p in b.processes_of(repo, &d.id)? {
            if seen.insert(p.process_id.clone()) {
                processes.push(p.into());
            }
        }
    }
    Ok(ContextOut {
        symbol: symbol.to_string(),
        defs,
        callers: b
            .callers(repo, symbol, CONTEXT_NEIGHBOR_DEPTH)?
            .into_iter()
            .map(Into::into)
            .collect(),
        callees: b
            .callees(repo, symbol, CONTEXT_NEIGHBOR_DEPTH)?
            .into_iter()
            .map(Into::into)
            .collect(),
        refs: b
            .references(repo, symbol, DEFAULT_REFS_LIMIT)?
            .into_iter()
            .map(Into::into)
            .collect(),
        processes,
    })
}

/// query top-3 + 每个命中的一跳 callers/callees（编辑器 hook 用的轻量版）。
pub fn augment(b: &dyn Backend, repo: Option<&str>, query: &str) -> anyhow::Result<AugmentOut> {
    let mut items = Vec::new();
    for hit in b.search(repo, query, AUGMENT_TOP_K)? {
        let callers = b
            .callers(repo, &hit.name, 1)?
            .into_iter()
            .map(Into::into)
            .collect();
        let callees = b
            .callees(repo, &hit.name, 1)?
            .into_iter()
            .map(Into::into)
            .collect();
        items.push(AugmentItem { hit: hit.into(), callers, callees });
    }
    Ok(AugmentOut { items })
}

pub fn analyze(b: &dyn Backend, repo_path: &str) -> anyhow::Result<AnalyzeOut> {
    Ok(AnalyzeOut { summary: b.analyze(repo_path)? })
}
