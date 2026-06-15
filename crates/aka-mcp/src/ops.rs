//! 工具输出的线上格式（紧凑、token 友好：数组 + 短字段名）与聚合逻辑。
//!
//! MCP 工具和 aka-server 的 HTTP API 共用这里的 DTO / 聚合函数，
//! 保证两个面输出一致。

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;

use crate::backend::{
    Backend, ChangeDetection, ChangedRange, ChangedSymbol, CodeLineMatch, CodeSearchHit,
    DirectoryCount, GraphqlMapEntry, ImpactDirection, ProcessHit, RepoInfo, RepoProgress,
    RouteConsumer, RouteMapEntry, SearchHit, SymbolRef, SymbolSelector, ToolMapEntry,
};

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
pub struct RepoProgressOut {
    pub stage: String,
    pub message: String,
    pub percent: f32,
    pub current: Option<u64>,
    pub total: Option<u64>,
    pub files: u64,
    pub nodes: u64,
    pub edges: u64,
    pub chunks: u64,
    pub logs: Vec<String>,
}

impl From<RepoProgress> for RepoProgressOut {
    fn from(p: RepoProgress) -> Self {
        Self {
            stage: p.stage,
            message: p.message,
            percent: (p.percent * 10.0).round() / 10.0,
            current: p.current,
            total: p.total,
            files: p.files,
            nodes: p.nodes,
            edges: p.edges,
            chunks: p.chunks,
            logs: p.logs,
        }
    }
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
    /// status = indexing / failed 时携带；ready 时为 null。
    pub progress: Option<RepoProgressOut>,
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
            progress: r.progress.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ReposOut {
    pub repos: Vec<RepoOut>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct QueryOut {
    /// 兼容旧客户端的扁平命中列表。
    pub hits: Vec<HitOut>,
    /// GitNexus-like 流程分组：按命中符号累计分数排序。
    pub processes: Vec<QueryProcessOut>,
    /// 命中符号按所属流程展开；一个符号参与多条流程时会出现多次。
    pub process_symbols: Vec<QueryProcessSymbolOut>,
    /// 不属于任何流程的 standalone 定义/文件命中。
    pub definitions: Vec<HitOut>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CodeLineOut {
    pub line: u32,
    pub text: String,
    pub matched: bool,
}

impl From<CodeLineMatch> for CodeLineOut {
    fn from(m: CodeLineMatch) -> Self {
        Self {
            line: m.line,
            text: m.text,
            matched: m.matched,
        }
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CodeHitOut {
    pub id: String,
    pub name: String,
    pub label: String,
    pub file: String,
    pub line: u32,
    pub score: f32,
    pub matches: Vec<CodeLineOut>,
}

impl From<CodeSearchHit> for CodeHitOut {
    fn from(h: CodeSearchHit) -> Self {
        Self {
            id: h.node_id,
            name: h.name,
            label: h.label,
            file: h.file_path,
            line: h.start_line,
            score: (h.score * 1000.0).round() / 1000.0,
            matches: h.matches.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DirectoryOut {
    pub dir: String,
    pub count: usize,
}

impl From<DirectoryCount> for DirectoryOut {
    fn from(d: DirectoryCount) -> Self {
        Self {
            dir: d.dir,
            count: d.count,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct CodeSearchOut {
    pub hits: Vec<CodeHitOut>,
    pub directories: Vec<DirectoryOut>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DefsOut {
    pub defs: Vec<HitOut>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RefsOut {
    pub refs: Vec<RefOut>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct CandidateOut {
    pub id: String,
    pub name: String,
    pub label: String,
    pub file: String,
    pub line: u32,
    pub score: f32,
}

impl From<SearchHit> for CandidateOut {
    fn from(h: SearchHit) -> Self {
        Self {
            id: h.node_id,
            name: h.name,
            label: h.label,
            file: h.file_path,
            line: h.start_line,
            score: (h.score * 1000.0).round() / 1000.0,
        }
    }
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

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct QueryProcessOut {
    pub id: String,
    pub summary: String,
    pub priority: f32,
    pub symbol_count: usize,
    pub process_type: String,
    pub step_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct QueryProcessSymbolOut {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub symbol_type: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(rename = "startLine")]
    pub start_line: u32,
    pub score: f32,
    pub process_id: String,
    pub step_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
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
    pub status: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<CandidateOut>,
    pub target: String,
    pub direction: String,
    pub impacted: Vec<RefOut>,
    pub count: usize,
    pub by_depth: Vec<DepthSummaryOut>,
    /// 按 affected_symbols 降序；同数按 process_id 升序保证确定性。
    pub affected_processes: Vec<AffectedProcessOut>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DepthSummaryOut {
    pub depth: u32,
    pub count: usize,
}

/// 一个符号的 360° 上下文。
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ContextOut {
    pub status: String,
    pub symbol: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<CandidateOut>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    pub status: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ChangedRangeOut {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
}

impl From<ChangedRange> for ChangedRangeOut {
    fn from(r: ChangedRange) -> Self {
        Self {
            file: r.file_path,
            start_line: r.start_line,
            end_line: r.end_line,
        }
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ChangedSymbolOut {
    pub id: String,
    pub name: String,
    pub label: String,
    pub file: String,
    pub line: u32,
    pub end_line: u32,
    pub ranges: Vec<ChangedRangeOut>,
}

impl From<ChangedSymbol> for ChangedSymbolOut {
    fn from(s: ChangedSymbol) -> Self {
        Self {
            id: s.node_id,
            name: s.name,
            label: s.label,
            file: s.file_path,
            line: s.start_line,
            end_line: s.end_line,
            ranges: s.ranges.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct DetectChangesOut {
    pub repo: String,
    pub scope: String,
    pub base_ref: Option<String>,
    pub changed_ranges: Vec<ChangedRangeOut>,
    pub changed_symbols: Vec<ChangedSymbolOut>,
    pub changed_count: usize,
    pub affected_processes: Vec<AffectedProcessOut>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RouteConsumerOut {
    pub name: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(rename = "accessedKeys", skip_serializing_if = "Vec::is_empty")]
    pub accessed_keys: Vec<String>,
    #[serde(rename = "fetchCount", skip_serializing_if = "Option::is_none")]
    pub fetch_count: Option<u32>,
}

impl From<RouteConsumer> for RouteConsumerOut {
    fn from(c: RouteConsumer) -> Self {
        Self {
            name: c.name,
            file_path: c.file_path,
            accessed_keys: c.accessed_keys,
            fetch_count: c.fetch_count,
        }
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RouteOut {
    pub id: String,
    pub route: String,
    pub handler: String,
    pub middleware: Vec<String>,
    #[serde(rename = "responseKeys", skip_serializing_if = "Vec::is_empty")]
    pub response_keys: Vec<String>,
    #[serde(rename = "errorKeys", skip_serializing_if = "Vec::is_empty")]
    pub error_keys: Vec<String>,
    pub consumers: Vec<RouteConsumerOut>,
    pub flows: Vec<String>,
}

impl From<RouteMapEntry> for RouteOut {
    fn from(r: RouteMapEntry) -> Self {
        Self {
            id: r.id,
            route: r.route,
            handler: r.handler,
            middleware: r.middleware,
            response_keys: r.response_keys,
            error_keys: r.error_keys,
            consumers: r.consumers.into_iter().map(Into::into).collect(),
            flows: r.flows,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RouteMapOut {
    pub routes: Vec<RouteOut>,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ToolOut {
    pub id: String,
    pub name: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub description: String,
    pub handlers: Vec<HitOut>,
    pub flows: Vec<String>,
}

impl From<ToolMapEntry> for ToolOut {
    fn from(t: ToolMapEntry) -> Self {
        Self {
            id: t.id,
            name: t.name,
            file_path: t.file_path,
            description: t.description,
            handlers: t.handlers.into_iter().map(Into::into).collect(),
            flows: t.flows,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ToolMapOut {
    pub tools: Vec<ToolOut>,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct GraphqlOut {
    pub id: String,
    pub name: String,
    #[serde(rename = "operationType")]
    pub operation_type: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub handlers: Vec<HitOut>,
    pub flows: Vec<String>,
}

impl From<GraphqlMapEntry> for GraphqlOut {
    fn from(g: GraphqlMapEntry) -> Self {
        Self {
            id: g.id,
            name: g.name,
            operation_type: g.operation_type,
            file_path: g.file_path,
            handlers: g.handlers.into_iter().map(Into::into).collect(),
            flows: g.flows,
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct GraphqlMapOut {
    pub operations: Vec<GraphqlOut>,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ShapeConsumerOut {
    pub name: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(rename = "accessedKeys", skip_serializing_if = "Vec::is_empty")]
    pub accessed_keys: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mismatched: Vec<String>,
    #[serde(rename = "mismatchConfidence", skip_serializing_if = "Option::is_none")]
    pub mismatch_confidence: Option<String>,
    #[serde(rename = "errorPathKeys", skip_serializing_if = "Vec::is_empty")]
    pub error_path_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ShapeRouteOut {
    pub route: String,
    pub handler: String,
    #[serde(rename = "responseKeys", skip_serializing_if = "Vec::is_empty")]
    pub response_keys: Vec<String>,
    #[serde(rename = "errorKeys", skip_serializing_if = "Vec::is_empty")]
    pub error_keys: Vec<String>,
    pub consumers: Vec<ShapeConsumerOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ShapeCheckOut {
    pub routes: Vec<ShapeRouteOut>,
    pub total: usize,
    #[serde(rename = "routesWithShapes")]
    pub routes_with_shapes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mismatches: Option<usize>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ApiConsumerOut {
    pub name: String,
    pub file: String,
    pub accesses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ApiMismatchOut {
    pub consumer: String,
    pub field: String,
    pub reason: String,
    pub confidence: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ResponseShapeOut {
    pub success: Vec<String>,
    pub error: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ApiImpactSummaryOut {
    #[serde(rename = "directConsumers")]
    pub direct_consumers: usize,
    #[serde(rename = "affectedFlows")]
    pub affected_flows: usize,
    #[serde(rename = "riskLevel")]
    pub risk_level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ApiImpactRouteOut {
    pub route: String,
    pub handler: String,
    #[serde(rename = "responseShape")]
    pub response_shape: ResponseShapeOut,
    pub middleware: Vec<String>,
    pub consumers: Vec<ApiConsumerOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mismatches: Vec<ApiMismatchOut>,
    #[serde(rename = "executionFlows")]
    pub execution_flows: Vec<String>,
    #[serde(rename = "impactSummary")]
    pub impact_summary: ApiImpactSummaryOut,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ApiImpactOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<ApiImpactRouteOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<ApiImpactRouteOut>,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 源文件清单的一项：repo 内相对路径（与 nodes 表 file_path 一致）+ 含行号的符号数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct FileEntry {
    pub path: String,
    pub symbols: u32,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct FilesOut {
    pub repo: String,
    pub files: Vec<FileEntry>,
}

pub const DEFAULT_QUERY_LIMIT: usize = 10;
pub const MAX_QUERY_LIMIT: usize = 100;
pub const DEFAULT_CODE_CONTEXT: usize = 1;
pub const MAX_CODE_CONTEXT: usize = 5;
pub const DEFAULT_REFS_LIMIT: usize = 25;
pub const DEFAULT_IMPACT_DEPTH: u32 = 2;
pub const DEFAULT_IMPACT_LIMIT: usize = 50;
pub const CONTEXT_NEIGHBOR_DEPTH: u32 = 1;
pub const AUGMENT_TOP_K: usize = 3;
/// query 每条命中最多带几个流程名（再多就靠 context / node 详情看全量）。
pub const MAX_HIT_PROCESS_NAMES: usize = 3;
pub const DEFAULT_QUERY_PROCESS_SYMBOL_LIMIT: usize = 10;
pub const MAX_QUERY_PROCESS_SYMBOL_LIMIT: usize = 200;

pub fn clamp_process_symbol_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_QUERY_PROCESS_SYMBOL_LIMIT)
        .clamp(1, MAX_QUERY_PROCESS_SYMBOL_LIMIT)
}

pub fn list_repos(b: &dyn Backend) -> anyhow::Result<ReposOut> {
    Ok(ReposOut {
        repos: b.list_repos()?.into_iter().map(Into::into).collect(),
    })
}

pub struct QueryOptions<'a> {
    pub repo: Option<&'a str>,
    pub query: &'a str,
    pub limit: usize,
    pub max_symbols: usize,
    pub include_content: bool,
    pub task_context: Option<&'a str>,
    pub goal: Option<&'a str>,
}

pub fn query(b: &dyn Backend, opts: QueryOptions<'_>) -> anyhow::Result<QueryOut> {
    let QueryOptions {
        repo,
        query,
        limit,
        max_symbols,
        include_content,
        task_context,
        goal,
    } = opts;
    let mut hits = Vec::new();
    let mut process_map: BTreeMap<String, QueryProcessAgg> = BTreeMap::new();
    let mut process_symbols = Vec::new();
    let mut definitions = Vec::new();
    let mut next_process_order = 0usize;
    let search_limit = limit.saturating_mul(max_symbols).max(limit);
    let search_hits = b.search(repo, query, search_limit)?;
    let node_ids: Vec<String> = search_hits.iter().map(|h| h.node_id.clone()).collect();
    let enrichments = b.query_enrichment(repo, &node_ids, include_content)?;
    let context_terms = ranking_terms([Some(query), task_context, goal]);
    for h in search_hits {
        let enrichment = enrichments.get(&h.node_id).cloned().unwrap_or_default();
        let procs = &enrichment.processes;
        let mut hit = HitOut::from(h);
        if !procs.is_empty() {
            hit.processes = Some(
                procs
                    .iter()
                    .take(MAX_HIT_PROCESS_NAMES)
                    .map(|p| p.name.clone())
                    .collect(),
            );
            for p in procs {
                if !process_map.contains_key(&p.process_id) {
                    process_map.insert(
                        p.process_id.clone(),
                        QueryProcessAgg {
                            id: p.process_id.clone(),
                            summary: p.name.clone(),
                            process_type: p.process_type.clone(),
                            step_count: p.step_count,
                            total_score: 0.0,
                            cohesion_boost: 0.0,
                            context_boost: context_score(
                                &context_terms,
                                [p.name.as_str(), p.process_type.as_str()],
                                std::iter::empty::<&str>(),
                            ),
                            symbol_count: 0,
                            order: next_process_order,
                        },
                    );
                    next_process_order += 1;
                }
                let entry = process_map
                    .get_mut(&p.process_id)
                    .expect("process inserted above");
                entry.total_score += hit.score;
                entry.cohesion_boost = entry.cohesion_boost.max(enrichment.cohesion);
                entry.context_boost = entry.context_boost.max(context_score(
                    &context_terms,
                    [
                        p.name.as_str(),
                        p.process_type.as_str(),
                        hit.name.as_str(),
                        hit.label.as_str(),
                        hit.file.as_str(),
                    ],
                    enrichment.module.iter().map(String::as_str),
                ));
                entry.symbol_count += 1;
                process_symbols.push(QueryProcessSymbolOut {
                    id: hit.id.clone(),
                    name: hit.name.clone(),
                    symbol_type: hit.label.clone(),
                    file_path: hit.file.clone(),
                    start_line: hit.line,
                    score: hit.score,
                    process_id: p.process_id.clone(),
                    step_index: p.step,
                    module: enrichment.module.clone(),
                    content: enrichment.content.clone(),
                });
            }
        } else {
            if include_content && hit.snip.is_none() {
                hit.snip = enrichment.content.clone();
            }
            definitions.push(hit.clone());
        }
        if hits.len() < limit {
            hits.push(hit);
        }
    }
    let mut process_aggs: Vec<QueryProcessAgg> = process_map.into_values().collect();
    process_aggs.sort_by(|a, b| {
        let a_priority = a.priority();
        let b_priority = b.priority();
        b_priority
            .total_cmp(&a_priority)
            .then_with(|| b.symbol_count.cmp(&a.symbol_count))
            .then_with(|| a.order.cmp(&b.order))
    });
    process_aggs.truncate(limit);
    let processes: Vec<QueryProcessOut> = process_aggs
        .into_iter()
        .map(|p| {
            let priority = (p.priority() * 1000.0).round() / 1000.0;
            QueryProcessOut {
                id: p.id,
                summary: p.summary,
                priority,
                symbol_count: p.symbol_count,
                process_type: p.process_type,
                step_count: p.step_count,
            }
        })
        .collect();
    let allowed: HashSet<&str> = processes.iter().map(|p| p.id.as_str()).collect();
    let process_rank: HashMap<&str, usize> = processes
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.as_str(), i))
        .collect();
    process_symbols.retain(|s| allowed.contains(s.process_id.as_str()));
    process_symbols.sort_by(|a, b| {
        process_rank
            .get(a.process_id.as_str())
            .cmp(&process_rank.get(b.process_id.as_str()))
            .then_with(|| a.step_index.cmp(&b.step_index))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut kept_per_process: BTreeMap<String, usize> = BTreeMap::new();
    process_symbols.retain(|s| {
        let n = kept_per_process.entry(s.process_id.clone()).or_default();
        if *n >= max_symbols {
            return false;
        }
        *n += 1;
        true
    });
    let mut seen_process_symbols = HashSet::new();
    process_symbols.retain(|s| seen_process_symbols.insert((s.process_id.clone(), s.id.clone())));
    definitions.truncate(20);

    Ok(QueryOut {
        hits,
        processes,
        process_symbols,
        definitions,
    })
}

struct QueryProcessAgg {
    id: String,
    summary: String,
    process_type: String,
    step_count: Option<u32>,
    total_score: f32,
    cohesion_boost: f32,
    context_boost: f32,
    symbol_count: usize,
    order: usize,
}

impl QueryProcessAgg {
    fn priority(&self) -> f32 {
        self.total_score + self.cohesion_boost * 0.1 + self.context_boost
    }
}

fn ranking_terms<'a>(parts: impl IntoIterator<Item = Option<&'a str>>) -> Vec<String> {
    let mut terms = HashSet::new();
    for part in parts.into_iter().flatten() {
        for raw in part.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_') {
            let term = raw.trim_matches('_').to_ascii_lowercase();
            if term.len() >= 3 {
                terms.insert(term);
            }
        }
    }
    let mut out: Vec<String> = terms.into_iter().collect();
    out.sort();
    out
}

fn context_score<'a>(
    terms: &[String],
    fields: impl IntoIterator<Item = &'a str>,
    extra_fields: impl IntoIterator<Item = &'a str>,
) -> f32 {
    if terms.is_empty() {
        return 0.0;
    }
    let haystack = fields
        .into_iter()
        .chain(extra_fields)
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let matches = terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count();
    (matches as f32 * 0.05).min(0.25)
}

pub fn search_code(
    b: &dyn Backend,
    repo: Option<&str>,
    query: &str,
    limit: usize,
    context: usize,
    regex: bool,
    path_filter: Option<&str>,
) -> anyhow::Result<CodeSearchOut> {
    let result = b.search_code(repo, query, limit, context, regex, path_filter)?;
    Ok(CodeSearchOut {
        hits: result.hits.into_iter().map(Into::into).collect(),
        directories: result.directories.into_iter().map(Into::into).collect(),
    })
}

pub fn route_map(
    b: &dyn Backend,
    repo: Option<&str>,
    route: Option<&str>,
) -> anyhow::Result<RouteMapOut> {
    let routes: Vec<RouteOut> = b
        .route_map(repo, route)?
        .into_iter()
        .map(Into::into)
        .collect();
    let total = routes.len();
    let message = (total == 0).then(|| match route {
        Some(route) => format!("No routes matching {route:?}"),
        None => "No routes found in this project.".to_string(),
    });
    Ok(RouteMapOut {
        routes,
        total,
        message,
    })
}

pub fn tool_map(
    b: &dyn Backend,
    repo: Option<&str>,
    tool: Option<&str>,
) -> anyhow::Result<ToolMapOut> {
    let tools: Vec<ToolOut> = b
        .tool_map(repo, tool)?
        .into_iter()
        .map(Into::into)
        .collect();
    let total = tools.len();
    let message = (total == 0).then(|| match tool {
        Some(tool) => format!("No tools matching {tool:?}"),
        None => "No tool definitions found.".to_string(),
    });
    Ok(ToolMapOut {
        tools,
        total,
        message,
    })
}

pub fn graphql_map(
    b: &dyn Backend,
    repo: Option<&str>,
    operation: Option<&str>,
) -> anyhow::Result<GraphqlMapOut> {
    let operations: Vec<GraphqlOut> = b
        .graphql_map(repo, operation)?
        .into_iter()
        .map(Into::into)
        .collect();
    let total = operations.len();
    let message = (total == 0).then(|| match operation {
        Some(operation) => format!("No GraphQL operations matching {operation:?}"),
        None => "No GraphQL operations found.".to_string(),
    });
    Ok(GraphqlMapOut {
        operations,
        total,
        message,
    })
}

pub fn shape_check(
    b: &dyn Backend,
    repo: Option<&str>,
    route: Option<&str>,
) -> anyhow::Result<ShapeCheckOut> {
    let routes = b.route_map(repo, route)?;
    let mut out = Vec::new();
    for r in routes {
        if r.consumers.is_empty() || (r.response_keys.is_empty() && r.error_keys.is_empty()) {
            continue;
        }
        let known: HashSet<String> = r
            .response_keys
            .iter()
            .chain(r.error_keys.iter())
            .cloned()
            .collect();
        let success: HashSet<String> = r.response_keys.iter().cloned().collect();
        let mut has_mismatch = false;
        let consumers = r
            .consumers
            .into_iter()
            .map(|c| {
                let mismatched: Vec<String> = c
                    .accessed_keys
                    .iter()
                    .filter(|key| !known.contains(*key))
                    .cloned()
                    .collect();
                let error_path_keys: Vec<String> = c
                    .accessed_keys
                    .iter()
                    .filter(|key| known.contains(*key) && !success.contains(*key))
                    .cloned()
                    .collect();
                if !mismatched.is_empty() {
                    has_mismatch = true;
                }
                ShapeConsumerOut {
                    name: c.name,
                    file_path: c.file_path,
                    accessed_keys: c.accessed_keys,
                    mismatched,
                    mismatch_confidence: has_mismatch.then(|| {
                        if c.fetch_count.unwrap_or(1) > 1 {
                            "low".to_string()
                        } else {
                            "high".to_string()
                        }
                    }),
                    error_path_keys,
                }
            })
            .collect();
        out.push(ShapeRouteOut {
            route: r.route,
            handler: r.handler,
            response_keys: r.response_keys,
            error_keys: r.error_keys,
            consumers,
            status: has_mismatch.then(|| "MISMATCH".to_string()),
        });
    }
    let mismatch_count = out
        .iter()
        .filter(|r| r.status.as_deref() == Some("MISMATCH"))
        .count();
    let message = if out.is_empty() {
        "No routes with both response shapes and consumers found.".to_string()
    } else if mismatch_count > 0 {
        format!(
            "Found {} route(s) with response shape data. {} route(s) have consumer/shape mismatches.",
            out.len(), mismatch_count
        )
    } else {
        format!(
            "Found {} route(s) with response shape data and consumers.",
            out.len()
        )
    };
    Ok(ShapeCheckOut {
        total: out.len(),
        routes_with_shapes: out.len(),
        mismatches: (mismatch_count > 0).then_some(mismatch_count),
        routes: out,
        message,
    })
}

pub fn api_impact(
    b: &dyn Backend,
    repo: Option<&str>,
    route: Option<&str>,
    file: Option<&str>,
) -> anyhow::Result<ApiImpactOut> {
    if route.is_none() && file.is_none() {
        return Ok(ApiImpactOut {
            route: None,
            routes: Vec::new(),
            total: 0,
            error: Some("Either \"route\" or \"file\" parameter is required.".into()),
        });
    }
    let mut routes = b.route_map(repo, route)?;
    if let Some(file) = file {
        routes.retain(|r| r.handler.contains(file));
    }
    if routes.is_empty() {
        let target = route.or(file).unwrap_or("");
        return Ok(ApiImpactOut {
            route: None,
            routes: Vec::new(),
            total: 0,
            error: Some(format!("No routes found matching {target:?}.")),
        });
    }
    let mut impacted = Vec::new();
    for r in routes {
        let known: HashSet<String> = r
            .response_keys
            .iter()
            .chain(r.error_keys.iter())
            .cloned()
            .collect();
        let mut mismatches = Vec::new();
        for c in &r.consumers {
            let confidence = if c.fetch_count.unwrap_or(1) > 1 {
                "low"
            } else {
                "high"
            };
            for key in &c.accessed_keys {
                if !known.contains(key) {
                    mismatches.push(ApiMismatchOut {
                        consumer: c.file_path.clone(),
                        field: key.clone(),
                        reason: "accessed but not in response shape".into(),
                        confidence: confidence.into(),
                    });
                }
            }
        }
        let direct_consumers = r.consumers.len();
        let affected_flows = r.flows.len();
        let risk_level = api_risk_level(direct_consumers, !mismatches.is_empty()).to_string();
        let warning = (direct_consumers > 0).then(|| {
            format!(
                "Changing response shape will affect {direct_consumers} component{}",
                if direct_consumers == 1 { "" } else { "s" }
            )
        });
        impacted.push(ApiImpactRouteOut {
            route: r.route,
            handler: r.handler,
            response_shape: ResponseShapeOut {
                success: r.response_keys,
                error: r.error_keys,
            },
            middleware: r.middleware,
            consumers: r
                .consumers
                .into_iter()
                .map(|c| ApiConsumerOut {
                    name: c.name,
                    file: c.file_path,
                    accesses: c.accessed_keys,
                })
                .collect(),
            mismatches,
            execution_flows: r.flows,
            impact_summary: ApiImpactSummaryOut {
                direct_consumers,
                affected_flows,
                risk_level,
                warning,
            },
        });
    }
    let total = impacted.len();
    if total == 1 {
        Ok(ApiImpactOut {
            route: impacted.into_iter().next(),
            routes: Vec::new(),
            total,
            error: None,
        })
    } else {
        Ok(ApiImpactOut {
            route: None,
            routes: impacted,
            total,
            error: None,
        })
    }
}

fn api_risk_level(consumers: usize, has_mismatch: bool) -> &'static str {
    let mut level = if consumers >= 10 {
        2
    } else if consumers >= 4 {
        1
    } else {
        0
    };
    if has_mismatch {
        level = (level + 1).min(2);
    }
    match level {
        0 => "LOW",
        1 => "MEDIUM",
        _ => "HIGH",
    }
}

pub fn find_definition(
    b: &dyn Backend,
    repo: Option<&str>,
    symbol: &str,
) -> anyhow::Result<DefsOut> {
    find_definition_select(b, repo, &SymbolSelector::from_symbol(symbol))
}

pub fn find_definition_select(
    b: &dyn Backend,
    repo: Option<&str>,
    selector: &SymbolSelector,
) -> anyhow::Result<DefsOut> {
    Ok(DefsOut {
        defs: b
            .find_definition_by_selector(repo, selector)?
            .into_iter()
            .map(Into::into)
            .collect(),
    })
}

pub fn references(
    b: &dyn Backend,
    repo: Option<&str>,
    symbol: &str,
    limit: usize,
) -> anyhow::Result<RefsOut> {
    references_select(b, repo, &SymbolSelector::from_symbol(symbol), limit)
}

pub fn references_select(
    b: &dyn Backend,
    repo: Option<&str>,
    selector: &SymbolSelector,
    limit: usize,
) -> anyhow::Result<RefsOut> {
    Ok(RefsOut {
        refs: b
            .references_by_selector(repo, selector, limit)?
            .into_iter()
            .map(Into::into)
            .collect(),
    })
}

pub fn impact(
    b: &dyn Backend,
    repo: Option<&str>,
    symbol: &str,
    depth: u32,
    limit: usize,
) -> anyhow::Result<ImpactOut> {
    impact_select(
        b,
        repo,
        &SymbolSelector::from_symbol(symbol),
        ImpactDirection::Upstream,
        depth,
        limit,
    )
}

pub fn impact_select(
    b: &dyn Backend,
    repo: Option<&str>,
    selector: &SymbolSelector,
    direction: ImpactDirection,
    depth: u32,
    limit: usize,
) -> anyhow::Result<ImpactOut> {
    let defs = b.find_definition_by_selector(repo, selector)?;
    if should_disambiguate(selector, &defs) {
        return Ok(ImpactOut {
            status: "ambiguous".into(),
            candidates: defs.into_iter().map(Into::into).collect(),
            target: selector.label().to_string(),
            direction: direction.as_str().into(),
            impacted: Vec::new(),
            count: 0,
            by_depth: Vec::new(),
            affected_processes: Vec::new(),
        });
    }
    let refs = b.impact_by_selector(repo, selector, direction, depth, limit)?;
    // 受影响节点集合 = 目标符号自身（所有定义）+ 影响面内全部符号，去重后再
    // 做流程聚合，避免同一符号在某流程里被数两次。
    let mut node_ids: Vec<String> = defs.into_iter().map(|h| h.node_id).collect();
    node_ids.extend(refs.iter().map(|r| r.node_id.clone()));
    node_ids.sort();
    node_ids.dedup();

    // BTreeMap 按 process_id 有序，聚合结果与输入顺序无关（确定性输出）。
    let mut agg: BTreeMap<String, AffectedProcessOut> = BTreeMap::new();
    for id in &node_ids {
        for p in b.processes_of(repo, id)? {
            let entry = agg
                .entry(p.process_id.clone())
                .or_insert(AffectedProcessOut {
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

    let by_depth = depth_summary(&refs);
    let impacted: Vec<RefOut> = refs.into_iter().map(Into::into).collect();
    let count = impacted.len();
    Ok(ImpactOut {
        status: "ok".into(),
        candidates: Vec::new(),
        target: selector.label().to_string(),
        direction: direction.as_str().into(),
        impacted,
        count,
        by_depth,
        affected_processes,
    })
}

/// definition + callers + callees + references 拼成一个结构化结果。
pub fn context(b: &dyn Backend, repo: Option<&str>, symbol: &str) -> anyhow::Result<ContextOut> {
    context_select(b, repo, &SymbolSelector::from_symbol(symbol))
}

pub fn context_select(
    b: &dyn Backend,
    repo: Option<&str>,
    selector: &SymbolSelector,
) -> anyhow::Result<ContextOut> {
    let raw_defs = b.find_definition_by_selector(repo, selector)?;
    if should_disambiguate(selector, &raw_defs) {
        return Ok(ContextOut {
            status: "ambiguous".into(),
            symbol: selector.label().to_string(),
            candidates: raw_defs.into_iter().map(Into::into).collect(),
            defs: Vec::new(),
            callers: Vec::new(),
            callees: Vec::new(),
            refs: Vec::new(),
            processes: Vec::new(),
        });
    }
    let defs: Vec<HitOut> = raw_defs.into_iter().map(Into::into).collect();
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
        status: "ok".into(),
        symbol: selector.label().to_string(),
        candidates: Vec::new(),
        defs,
        callers: b
            .callers_by_selector(repo, selector, CONTEXT_NEIGHBOR_DEPTH)?
            .into_iter()
            .map(Into::into)
            .collect(),
        callees: b
            .callees_by_selector(repo, selector, CONTEXT_NEIGHBOR_DEPTH)?
            .into_iter()
            .map(Into::into)
            .collect(),
        refs: b
            .references_by_selector(repo, selector, DEFAULT_REFS_LIMIT)?
            .into_iter()
            .map(Into::into)
            .collect(),
        processes,
    })
}

fn should_disambiguate(selector: &SymbolSelector, defs: &[SearchHit]) -> bool {
    defs.len() > 1 && !selector.is_narrowed()
}

fn depth_summary(refs: &[SymbolRef]) -> Vec<DepthSummaryOut> {
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for r in refs {
        *counts.entry(r.depth).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(depth, count)| DepthSummaryOut { depth, count })
        .collect()
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
        items.push(AugmentItem {
            hit: hit.into(),
            callers,
            callees,
        });
    }
    Ok(AugmentOut { items })
}

pub fn analyze(b: &dyn Backend, repo_path: &str) -> anyhow::Result<AnalyzeOut> {
    let summary = b.analyze(repo_path)?;
    Ok(AnalyzeOut {
        repo: analyze_repo_name(&summary),
        status: analyze_status(&summary).to_string(),
        summary,
    })
}

fn analyze_repo_name(summary: &str) -> Option<String> {
    summary
        .strip_prefix("indexing scheduled: ")
        .or_else(|| summary.strip_prefix("update scheduled: "))
        .and_then(|rest| rest.split_whitespace().next())
        .map(|name| {
            name.trim_matches(|ch: char| matches!(ch, ':' | ',' | ';'))
                .to_string()
        })
        .filter(|name| !name.is_empty())
}

fn analyze_status(summary: &str) -> &'static str {
    if summary.starts_with("indexing scheduled: ") || summary.starts_with("update scheduled: ") {
        "indexing"
    } else {
        "completed"
    }
}

pub fn detect_changes(
    b: &dyn Backend,
    repo: Option<&str>,
    scope: &str,
    base_ref: Option<&str>,
) -> anyhow::Result<DetectChangesOut> {
    let detection = b.detect_changes(repo, scope, base_ref)?;
    detect_changes_from_detection(b, repo, detection)
}

fn detect_changes_from_detection(
    b: &dyn Backend,
    repo: Option<&str>,
    detection: ChangeDetection,
) -> anyhow::Result<DetectChangesOut> {
    let mut agg: BTreeMap<String, AffectedProcessOut> = BTreeMap::new();
    for symbol in &detection.symbols {
        for p in b.processes_of(repo, &symbol.node_id)? {
            let entry = agg
                .entry(p.process_id.clone())
                .or_insert(AffectedProcessOut {
                    process_id: p.process_id,
                    name: p.name,
                    process_type: p.process_type,
                    step_count: p.step_count,
                    first_affected_step: None,
                    affected_symbols: 0,
                });
            entry.affected_symbols += 1;
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
    let changed_count = detection.symbols.len();
    Ok(DetectChangesOut {
        repo: detection.repo,
        scope: detection.scope,
        base_ref: detection.base_ref,
        changed_ranges: detection.ranges.into_iter().map(Into::into).collect(),
        changed_symbols: detection.symbols.into_iter().map(Into::into).collect(),
        changed_count,
        affected_processes,
    })
}

/// 某仓库的源文件清单（含符号数），按 path 升序。repo 未注册 → Err（HTTP 面 404）。
pub fn list_files(b: &dyn Backend, repo: &str) -> anyhow::Result<FilesOut> {
    Ok(FilesOut {
        repo: repo.to_string(),
        files: b.list_files(repo)?,
    })
}
