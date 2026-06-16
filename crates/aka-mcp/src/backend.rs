//! 数据层接缝 — MCP / HTTP 工具逻辑全部面向这个 trait 编写。
//!
//! 真实实现（tantivy 检索 + SQLite/CSR 图查询）由集成批次接入；
//! 生产入口由 CLI / 桌面端注入真实 backend；测试夹具只存在于 integration tests。
//!
//! 约定：
//! - `repo = None` 表示「所有已注册仓库」；`Some(name)` 按 registry 里的仓库名过滤。
//! - 查不到不是错误：返回空 Vec。`Err` 只用于真正的故障（索引损坏、IO 等）。
//! - 方法是同步签名；调用方（async 上下文）负责用 `spawn_blocking` 包装。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// GitNexus-compatible symbol selector.  `symbol` is the human name, while
/// `uid` is the graph node id returned as `id` by query/find/context results.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolSelector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl SymbolSelector {
    pub fn from_symbol(symbol: &str) -> Self {
        Self {
            symbol: Some(symbol.to_string()),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.symbol.as_deref().is_none_or(str::is_empty)
            && self.uid.as_deref().is_none_or(str::is_empty)
    }

    pub fn is_narrowed(&self) -> bool {
        self.uid.as_deref().is_some_and(|v| !v.is_empty())
            || self.file_path.as_deref().is_some_and(|v| !v.is_empty())
            || self.kind.as_deref().is_some_and(|v| !v.is_empty())
    }

    pub fn label(&self) -> &str {
        self.symbol
            .as_deref()
            .filter(|v| !v.is_empty())
            .or_else(|| self.uid.as_deref().filter(|v| !v.is_empty()))
            .unwrap_or("")
    }

    pub fn matches_hit(&self, hit: &SearchHit) -> bool {
        if self.uid.as_deref().is_some_and(|uid| uid != hit.node_id) {
            return false;
        }
        if self
            .symbol
            .as_deref()
            .is_some_and(|symbol| symbol != hit.name)
        {
            return false;
        }
        if self
            .kind
            .as_deref()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case(&hit.label))
        {
            return false;
        }
        if let Some(file) = self.file_path.as_deref() {
            let want = normalize_selector_path(file);
            let got = normalize_selector_path(&hit.file_path);
            if got != want && !got.ends_with(&format!("/{want}")) {
                return false;
            }
        }
        true
    }
}

fn normalize_selector_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

pub fn dedup_symbol_refs(refs: &mut Vec<SymbolRef>) {
    let mut seen = std::collections::HashSet::new();
    refs.retain(|r| seen.insert((r.node_id.clone(), r.edge_type.clone(), r.depth)));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpactDirection {
    Upstream,
    Downstream,
    Both,
}

impl ImpactDirection {
    pub fn parse(raw: Option<&str>) -> anyhow::Result<Self> {
        match raw.unwrap_or("upstream").to_ascii_lowercase().as_str() {
            "upstream" | "callers" | "reverse" | "dependents" => Ok(Self::Upstream),
            "downstream" | "callees" | "forward" | "dependencies" => Ok(Self::Downstream),
            "both" | "all" => Ok(Self::Both),
            other => anyhow::bail!(
                "invalid impact direction {other:?}; expected upstream, downstream, or both"
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upstream => "upstream",
            Self::Downstream => "downstream",
            Self::Both => "both",
        }
    }
}

/// 全文 / 符号检索命中。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub node_id: String,
    pub name: String,
    /// 节点类型（Function / Class / Method / File …），对应图谱 label。
    pub label: String,
    /// 切块类型（ast-function / char …）；命中来自 chunk 文档时携带（合同只增不改）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub file_path: String,
    pub start_line: u32,
    pub score: f32,
    pub snippet: Option<String>,
}

/// 源码行级命中（code search 的 raw match + surrounding context）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeLineMatch {
    pub line: u32,
    pub text: String,
    /// true = query/regex directly matched this line; false = surrounding context.
    #[serde(default)]
    pub matched: bool,
}

/// 源码搜索命中：按节点/文件聚合，携带 raw match lines。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeSearchHit {
    pub node_id: String,
    pub name: String,
    pub label: String,
    pub file_path: String,
    pub start_line: u32,
    pub score: f32,
    pub matches: Vec<CodeLineMatch>,
}

/// 顶层目录命中分布。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectoryCount {
    pub dir: String,
    pub count: usize,
}

/// 源码搜索完整结果（raw matches + 目录分布）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeSearchResult {
    pub hits: Vec<CodeSearchHit>,
    pub directories: Vec<DirectoryCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedRange {
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangedSymbol {
    pub node_id: String,
    pub name: String,
    pub label: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub ranges: Vec<ChangedRange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeDetection {
    pub repo: String,
    pub scope: String,
    pub base_ref: Option<String>,
    pub ranges: Vec<ChangedRange>,
    pub symbols: Vec<ChangedSymbol>,
}

/// 图遍历得到的符号引用（callers / callees / references / impact）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolRef {
    pub node_id: String,
    pub name: String,
    pub label: String,
    pub file_path: String,
    pub start_line: u32,
    /// 到达该节点经过的边类型（CALLS / IMPORTS / …）。
    pub edge_type: String,
    /// 距离起点的跳数，从 1 开始。
    pub depth: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameEdit {
    pub file_path: String,
    pub line: u32,
    pub column: u32,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenamePlan {
    pub status: String,
    pub target: String,
    pub replacement: String,
    pub dry_run: bool,
    pub edits: Vec<RenameEdit>,
    pub changed_files: usize,
    pub applied: bool,
    pub message: Option<String>,
    pub candidates: Vec<SearchHit>,
}

/// 符号所属的执行流程（图里 label = `Process` 的合成节点，调用链「入口→终点」）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessHit {
    pub process_id: String,
    pub name: String,
    /// 流程类型（来自 Process 节点 props 的 processType）。
    pub process_type: String,
    /// 该符号在流程内的步号（STEP_IN_PROCESS 边携带）；缺失 = 边上没有步号。
    pub step: Option<u32>,
    /// 流程总步数（Process 节点 props 的 stepCount）。
    pub step_count: Option<u32>,
}

/// Query 命中的补充图谱信息。用于把 GitNexus 的 query 语义（流程分组、
/// 社区/模块提示、可选源码内容）压到一次批量取数里。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct QueryEnrichment {
    /// 节点所属流程；一个符号可参与多条流程。
    pub processes: Vec<ProcessHit>,
    /// Community/模块名（GitNexus 输出里的 `module`），没有则省略。
    pub module: Option<String>,
    /// Community cohesion，作为流程排序的轻微加权信号。
    pub cohesion: f32,
    /// include_content=true 时返回的符号源码切片。
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RouteConsumer {
    pub name: String,
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accessed_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetch_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RouteMapEntry {
    pub id: String,
    pub route: String,
    pub handler: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub middleware: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub error_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<RouteConsumer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolMapEntry {
    pub id: String,
    pub name: String,
    pub file_path: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handlers: Vec<SearchHit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GraphqlMapEntry {
    pub id: String,
    pub name: String,
    pub operation_type: String,
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handlers: Vec<SearchHit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,
}

/// 后台导入 / 更新任务的实时进度。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoProgress {
    /// 当前阶段：queued / checkout / extract / engine / index / register / done。
    pub stage: String,
    /// 给 UI 展示的当前动作。
    pub message: String,
    /// 0..100 的粗略总体进度。
    pub percent: f32,
    /// 当前阶段内的计数（engine 事件提供时携带）。
    pub current: Option<u64>,
    /// 当前阶段内的总量（engine 事件提供时携带）。
    pub total: Option<u64>,
    pub files: u64,
    pub nodes: u64,
    pub edges: u64,
    pub chunks: u64,
    /// 最近日志尾部，最新的在最后。
    pub logs: Vec<String>,
}

/// 已注册仓库的概要信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoInfo {
    pub name: String,
    pub path: String,
    pub nodes: u64,
    pub edges: u64,
    /// 索引完成时间（unix 秒）；None = 注册过但尚未成功索引。
    pub indexed_at: Option<u64>,
    pub embeddings_enabled: bool,
    /// 运行时状态：`ready` / `indexing` / `failed`。
    pub status: String,
    /// 来源：`local` / `git` / `zip`。
    pub source_kind: String,
    /// git 来源的 clone URL。
    pub source_url: Option<String>,
    /// 失败原因等补充信息（status = failed 时携带）。
    pub detail: Option<String>,
    /// per-repo 预览渲染节点预算；None = 默认 50_000。
    pub render_max_nodes: Option<u32>,
    /// status = indexing / failed 时携带的实时进度和日志。
    pub progress: Option<RepoProgress>,
}

/// per-repo 设置更新（settings 端点与 Backend 接缝共用一个形状）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSettingsUpdate {
    pub embeddings_enabled: bool,
    /// None = 恢复默认渲染预算（50_000）；Some(v) 写入前必须 clamp 到
    /// `aka_core::MIN_RENDER_NODES..=aka_core::MAX_RENDER_NODES`。
    #[serde(default)]
    pub render_max_nodes: Option<u32>,
}

/// 数据层抽象。所有工具（MCP 九工具 + HTTP API）只依赖这个 trait。
pub trait Backend: Send + Sync + 'static {
    /// Queue one or more workspace roots for background indexing when supported.
    ///
    /// This lets Streamable HTTP MCP clients share the desktop backend while still
    /// giving aka a way to discover the agent's current project roots via MCP
    /// `roots/list`. Implementations should silently ignore already registered or
    /// already queued roots and return the names of newly queued repositories.
    fn queue_workspaces(&self, roots: &[std::path::PathBuf]) -> anyhow::Result<Vec<String>> {
        let _ = roots;
        Ok(Vec::new())
    }

    fn list_repos(&self) -> anyhow::Result<Vec<RepoInfo>>;

    /// 关键词 / 语义混合检索。
    fn search(
        &self,
        repo: Option<&str>,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchHit>>;

    /// 行级源码搜索：返回 raw match lines 与顶层目录分布。
    fn search_code(
        &self,
        repo: Option<&str>,
        query: &str,
        limit: usize,
        context: usize,
        regex: bool,
        path_filter: Option<&str>,
    ) -> anyhow::Result<CodeSearchResult> {
        let _ = (repo, query, limit, context, regex, path_filter);
        anyhow::bail!("search_code not supported by this backend")
    }

    /// 按符号名精确定位定义（可能重名，返回多条）。
    fn find_definition(&self, repo: Option<&str>, symbol: &str) -> anyhow::Result<Vec<SearchHit>>;

    /// GitNexus-compatible definition lookup with uid/file/kind disambiguation.
    fn find_definition_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let Some(symbol) = selector.symbol.as_deref() else {
            return Ok(Vec::new());
        };
        Ok(self
            .find_definition(repo, symbol)?
            .into_iter()
            .filter(|hit| selector.matches_hit(hit))
            .collect())
    }

    /// 指向该符号的所有引用边（任意边类型，一跳）。
    fn references(
        &self,
        repo: Option<&str>,
        symbol: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<SymbolRef>>;

    fn references_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        limit: usize,
    ) -> anyhow::Result<Vec<SymbolRef>> {
        let Some(symbol) = selector.symbol.as_deref() else {
            return Ok(Vec::new());
        };
        self.references(repo, symbol, limit)
    }

    /// 反向调用链（谁调用了它），BFS 到 `depth` 跳。
    fn callers(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
    ) -> anyhow::Result<Vec<SymbolRef>>;

    fn callers_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        depth: u32,
    ) -> anyhow::Result<Vec<SymbolRef>> {
        let Some(symbol) = selector.symbol.as_deref() else {
            return Ok(Vec::new());
        };
        self.callers(repo, symbol, depth)
    }

    /// 正向调用链（它调用了谁），BFS 到 `depth` 跳。
    fn callees(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
    ) -> anyhow::Result<Vec<SymbolRef>>;

    fn callees_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        depth: u32,
    ) -> anyhow::Result<Vec<SymbolRef>> {
        let Some(symbol) = selector.symbol.as_deref() else {
            return Ok(Vec::new());
        };
        self.callees(repo, symbol, depth)
    }

    /// 改动影响面：可达的反向依赖集合，截断到 `limit`。
    fn impact(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
        limit: usize,
    ) -> anyhow::Result<Vec<SymbolRef>>;

    fn impact_by_selector(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        direction: ImpactDirection,
        depth: u32,
        limit: usize,
    ) -> anyhow::Result<Vec<SymbolRef>> {
        match direction {
            ImpactDirection::Upstream => {
                let Some(symbol) = selector.symbol.as_deref() else {
                    return Ok(Vec::new());
                };
                self.impact(repo, symbol, depth, limit)
            }
            ImpactDirection::Downstream => self.callees_by_selector(repo, selector, depth),
            ImpactDirection::Both => {
                let mut out = self.impact_by_selector(
                    repo,
                    selector,
                    ImpactDirection::Upstream,
                    depth,
                    limit,
                )?;
                out.extend(self.callees_by_selector(repo, selector, depth)?);
                dedup_symbol_refs(&mut out);
                out.truncate(limit);
                Ok(out)
            }
        }
    }

    /// Graph-aware rename. Implementations should use the indexed definition +
    /// reference graph to restrict candidate files/locations, return
    /// `status="ambiguous"` with candidates when the selector is not narrow
    /// enough, and only write files when `dry_run=false`.
    fn rename_symbol(
        &self,
        repo: Option<&str>,
        selector: &SymbolSelector,
        replacement: &str,
        dry_run: bool,
    ) -> anyhow::Result<RenamePlan> {
        let _ = (repo, selector, replacement, dry_run);
        anyhow::bail!("rename not supported by this backend")
    }

    /// 节点所属的执行流程（沿 `符号-[STEP_IN_PROCESS]->Process` 边查归属）。
    /// `node_id` 是图谱节点 id（不是符号名）；节点不存在或没有流程数据
    /// 一律返回空 Vec（合同只增不改，默认空实现保旧实现 / 测试不破）。
    fn processes_of(&self, repo: Option<&str>, node_id: &str) -> anyhow::Result<Vec<ProcessHit>> {
        let _ = (repo, node_id);
        Ok(Vec::new())
    }

    /// 批量补充 query 结果所需的流程、模块/社区、可选源码内容。
    ///
    /// 默认实现逐个调用 `processes_of`，保证旧 Backend 不破；真实 Backend
    /// 应覆写为批量查询以避免 GitNexus 曾经修过的 N+1 热点。
    fn query_enrichment(
        &self,
        repo: Option<&str>,
        node_ids: &[String],
        include_content: bool,
    ) -> anyhow::Result<std::collections::HashMap<String, QueryEnrichment>> {
        let _ = include_content;
        let mut out = std::collections::HashMap::new();
        for id in node_ids {
            out.insert(
                id.clone(),
                QueryEnrichment {
                    processes: self.processes_of(repo, id)?,
                    ..QueryEnrichment::default()
                },
            );
        }
        Ok(out)
    }

    /// 触发（重新）分析一个仓库，返回任务描述 / 结果摘要。
    fn analyze(&self, repo_path: &str) -> anyhow::Result<String>;

    fn detect_changes(
        &self,
        repo: Option<&str>,
        scope: &str,
        base_ref: Option<&str>,
    ) -> anyhow::Result<ChangeDetection> {
        let _ = (repo, scope, base_ref);
        anyhow::bail!("detect_changes not supported by this backend")
    }

    fn route_map(
        &self,
        repo: Option<&str>,
        route: Option<&str>,
    ) -> anyhow::Result<Vec<RouteMapEntry>> {
        let _ = (repo, route);
        anyhow::bail!("route_map not supported by this backend")
    }

    fn tool_map(
        &self,
        repo: Option<&str>,
        tool: Option<&str>,
    ) -> anyhow::Result<Vec<ToolMapEntry>> {
        let _ = (repo, tool);
        anyhow::bail!("tool_map not supported by this backend")
    }

    fn graphql_map(
        &self,
        repo: Option<&str>,
        operation: Option<&str>,
    ) -> anyhow::Result<Vec<GraphqlMapEntry>> {
        let _ = (repo, operation);
        anyhow::bail!("graphql_map not supported by this backend")
    }

    /// 图 LOD 快照（aka-graph `LodGraph` 的 JSON 形状），给可视化用。
    /// `max_nodes = None` 时由实现解析 per-repo 的 render_max_nodes 设置
    /// （没有则默认 50_000）；无论来源，最终预算必须 clamp 到硬上限。
    /// 默认不支持——只有接了图存储的 Backend 才覆写。
    fn graph_lod(&self, repo: &str, max_nodes: Option<usize>) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, max_nodes);
        anyhow::bail!("graph_lod not supported by this backend")
    }

    /// 簇级总览（GraphJSON 同形）：每个 Community/Cluster 一个节点，边为簇间聚合权重。
    fn graph_clusters(&self, repo: &str) -> anyhow::Result<serde_json::Value> {
        let _ = repo;
        anyhow::bail!("graph_clusters not supported by this backend")
    }

    // ── 仓库管理（导入 / 更新 / 删除 / 设置）──────────────────────
    // 默认全部 bail "not supported"——只有真实 Backend（aka-cli）覆写。
    // 导入 / 更新都是 202 语义：调用立即返回仓库名，分析任务在后台执行，
    // 进度经 `repo_runtime_status` / `list_repos` 的 status 字段暴露。

    /// 导入新仓库。`kind` = `git`（src 为 clone URL）或 `local`（src 为本地路径）。
    /// 返回最终仓库名；分析在后台执行。
    fn import_repo(&self, kind: &str, src: &str, name: Option<&str>) -> anyhow::Result<String> {
        let _ = (kind, src, name);
        anyhow::bail!("import_repo not supported by this backend")
    }

    /// 重新拉取并分析（git: pull + analyze；local: 直接 analyze；zip: 报错提示走 update-zip）。
    fn update_repo(&self, name: &str) -> anyhow::Result<String> {
        let _ = name;
        anyhow::bail!("update_repo not supported by this backend")
    }

    /// 用新 zip 覆盖更新 zip 来源仓库（清空 checkout 后重新解压 + analyze）。
    fn update_repo_zip(&self, name: &str, zip_path: &std::path::Path) -> anyhow::Result<String> {
        let _ = (name, zip_path);
        anyhow::bail!("update_repo_zip not supported by this backend")
    }

    /// 从 zip 包导入新仓库（解压到受管 checkout 目录 + 后台 analyze）。
    fn import_repo_zip(&self, name: &str, zip_path: &std::path::Path) -> anyhow::Result<String> {
        let _ = (name, zip_path);
        anyhow::bail!("import_repo_zip not supported by this backend")
    }

    /// 移除仓库：注册表 + 数据目录；受管 checkout 一并删除（用户本地路径不动）。
    fn remove_repo(&self, name: &str) -> anyhow::Result<()> {
        let _ = name;
        anyhow::bail!("remove_repo not supported by this backend")
    }

    /// 每仓库设置（embedding 开关 + 渲染节点预算；向量回填是后续版本）。
    fn set_repo_settings(&self, name: &str, settings: RepoSettingsUpdate) -> anyhow::Result<()> {
        let _ = (name, settings);
        anyhow::bail!("set_repo_settings not supported by this backend")
    }

    /// 节点详情（完整 properties + 度数概要），给前端弹窗用。
    fn node_detail(&self, repo: &str, id: &str) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, id);
        anyhow::bail!("node_detail not supported by this backend")
    }

    /// 以某节点为中心的 ego 子图（与 LodGraph 同形 JSON），给中心化重渲用。
    fn ego_graph(
        &self,
        repo: &str,
        id: &str,
        depth: u32,
        max_nodes: usize,
    ) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, id, depth, max_nodes);
        anyhow::bail!("ego_graph not supported by this backend")
    }

    /// 读取仓库内某文件的源码切片（详情面板用）。`start`/`end` 为 1-based 含端
    /// 行号，缺省 = 整个文件（单次最多 2000 行）。返回 JSON 合同：
    /// `{path, abs_path, total_lines, start, end, lines: [..], truncated}`。
    /// 实现必须防路径穿越（canonicalize 后仍须位于 repo 根目录内）。
    /// 默认不支持——只有拿得到 repo checkout 的 Backend 才覆写。
    fn read_source(
        &self,
        repo: &str,
        path: &str,
        start: Option<u32>,
        end: Option<u32>,
    ) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, path, start, end);
        anyhow::bail!("read_source not supported by this backend")
    }

    /// 文件内符号列表（源码预览高亮用）：`path` 与 nodes 表 file_path 精确匹配，
    /// 返回 JSON 合同 `{path, symbols: [{id, name, label, file, line, end_line}]}`，
    /// symbols 按 line 升序；无行号的节点（File / Folder / Community 等）滤掉。
    /// 文件存在但没有符号 → 空数组；repo 未注册 → Err（HTTP 面 404）。
    /// 默认不支持——只有接了图存储的 Backend 才覆写。
    fn file_symbols(&self, repo: &str, path: &str) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, path);
        anyhow::bail!("file_symbols not supported by this backend")
    }

    /// 仓库源文件清单：每个含真实定义（start_line 非空）的文件 → 符号数，
    /// 按 path 升序，确定性输出。file_path 为 NULL 的聚合节点排除。
    /// repo 未注册 → Err（HTTP 面 404）。
    /// 默认不支持——只有接了图存储的 Backend 才覆写。
    fn list_files(&self, repo: &str) -> anyhow::Result<Vec<crate::ops::FileEntry>> {
        let _ = repo;
        anyhow::bail!("list_files not supported by this backend")
    }

    /// 后台任务运行时状态：仓库名 → (status, detail)。
    /// status ∈ {indexing, failed}；不在 map 里 = ready。
    fn repo_runtime_status(&self) -> std::collections::HashMap<String, (String, Option<String>)> {
        Default::default()
    }
}
