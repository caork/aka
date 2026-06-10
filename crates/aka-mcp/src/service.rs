//! rmcp ServerHandler — 八个 MCP 工具，全部面向 [`Backend`] trait。
//!
//! 输出统一为紧凑 JSON 文本（`Content::text`），格式见 [`crate::ops`]。
//! Backend 执行错误走 in-band tool error（`is_error: true`），LLM 可见可重试；
//! 只有序列化/运行时故障才上报协议级 `ErrorData`。

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError,
    ServerHandler,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::backend::Backend;
use crate::ops;

/// aka 的 MCP 服务（tools-only）。
#[derive(Clone)]
pub struct AkaMcpServer {
    backend: Arc<dyn Backend>,
}

impl AkaMcpServer {
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self { backend }
    }

    /// 把同步 Backend 调用挪到 blocking 线程池，结果序列化成紧凑 JSON 文本。
    async fn run<T, F>(&self, f: F) -> Result<CallToolResult, McpError>
    where
        T: serde::Serialize + Send + 'static,
        F: FnOnce(&dyn Backend) -> anyhow::Result<T> + Send + 'static,
    {
        let backend = Arc::clone(&self.backend);
        let res = tokio::task::spawn_blocking(move || f(backend.as_ref()))
            .await
            .map_err(|e| McpError::internal_error(format!("backend task failed: {e}"), None))?;
        match res {
            Ok(v) => {
                let body = serde_json::to_string(&v).map_err(|e| {
                    McpError::internal_error(format!("serialize tool output: {e}"), None)
                })?;
                Ok(CallToolResult::success(vec![Content::text(body)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "backend error: {e:#}"
            ))])),
        }
    }
}

fn clamp_limit(limit: Option<usize>, default: usize) -> usize {
    limit.unwrap_or(default).clamp(1, ops::MAX_QUERY_LIMIT)
}

// ---- 工具参数 ----

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QueryParams {
    /// Repository name to search in. Omit to search all indexed repositories.
    #[serde(default)]
    pub repo: Option<String>,
    /// Search query: symbol name, keywords, or natural language.
    pub query: String,
    /// Max results to return (default 10, max 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SymbolParams {
    /// Repository name. Omit to look across all indexed repositories.
    #[serde(default)]
    pub repo: Option<String>,
    /// Exact symbol name (function, class, method...).
    pub symbol: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReferencesParams {
    /// Repository name. Omit to look across all indexed repositories.
    #[serde(default)]
    pub repo: Option<String>,
    /// Exact symbol name whose references to list.
    pub symbol: String,
    /// Max references to return (default 25, max 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImpactParams {
    /// Repository name. Omit to look across all indexed repositories.
    #[serde(default)]
    pub repo: Option<String>,
    /// Exact symbol name to compute the blast radius for.
    pub symbol: String,
    /// How many reverse-dependency hops to traverse (default 2).
    #[serde(default)]
    pub depth: Option<u32>,
    /// Max impacted symbols to return (default 50, max 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnalyzeParams {
    /// Absolute path of the repository to (re)index.
    pub repo_path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AugmentParams {
    /// Repository name. Omit to search all indexed repositories.
    #[serde(default)]
    pub repo: Option<String>,
    /// Search query, typically the symbol or text under the cursor.
    pub query: String,
}

// ---- 八个工具 ----

#[tool_router]
impl AkaMcpServer {
    #[tool(
        description = "List all indexed repositories with node/edge counts and index status. Call this first to discover what is searchable."
    )]
    pub async fn list_repos(&self) -> Result<CallToolResult, McpError> {
        self.run(ops::list_repos).await
    }

    #[tool(
        description = "Search the code knowledge graph for symbols (functions, classes, files) matching a query. Returns compact JSON hits with id, name, label, file, line, score, and an optional snippet."
    )]
    pub async fn query(
        &self,
        Parameters(p): Parameters<QueryParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = clamp_limit(p.limit, ops::DEFAULT_QUERY_LIMIT);
        self.run(move |b| ops::query(b, p.repo.as_deref(), &p.query, limit))
            .await
    }

    #[tool(
        description = "Get the 360-degree context of a symbol in one call: its definition(s), direct callers, direct callees, and incoming references. Prefer this over separate lookups when exploring an unfamiliar symbol."
    )]
    pub async fn context(
        &self,
        Parameters(p): Parameters<SymbolParams>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |b| ops::context(b, p.repo.as_deref(), &p.symbol))
            .await
    }

    #[tool(
        description = "Find where a symbol is defined (exact name match). Returns one hit per definition with file and line."
    )]
    pub async fn find_definition(
        &self,
        Parameters(p): Parameters<SymbolParams>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |b| ops::find_definition(b, p.repo.as_deref(), &p.symbol))
            .await
    }

    #[tool(
        description = "List code locations that reference a symbol (callers, importers, etc.), one hop in the graph. Use 'impact' for the transitive blast radius."
    )]
    pub async fn search_references(
        &self,
        Parameters(p): Parameters<ReferencesParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = clamp_limit(p.limit, ops::DEFAULT_REFS_LIMIT);
        self.run(move |b| ops::references(b, p.repo.as_deref(), &p.symbol, limit))
            .await
    }

    #[tool(
        description = "Estimate the blast radius of changing a symbol: all transitive reverse dependencies up to a depth. Each result carries the hop distance ('depth')."
    )]
    pub async fn impact(
        &self,
        Parameters(p): Parameters<ImpactParams>,
    ) -> Result<CallToolResult, McpError> {
        let depth = p.depth.unwrap_or(ops::DEFAULT_IMPACT_DEPTH).clamp(1, 10);
        let limit = clamp_limit(p.limit, ops::DEFAULT_IMPACT_LIMIT);
        self.run(move |b| ops::impact(b, p.repo.as_deref(), &p.symbol, depth, limit))
            .await
    }

    #[tool(
        description = "Trigger (re)indexing of a repository by absolute path. Returns a short summary of the scheduled/completed analysis."
    )]
    pub async fn analyze(
        &self,
        Parameters(p): Parameters<AnalyzeParams>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |b| ops::analyze(b, &p.repo_path)).await
    }

    #[tool(
        description = "Lightweight context for editor hooks: top-3 search hits for a query, each with its one-hop callers and callees. Cheaper than 'context'; intended for automatic prompt augmentation."
    )]
    pub async fn augment(
        &self,
        Parameters(p): Parameters<AugmentParams>,
    ) -> Result<CallToolResult, McpError> {
        self.run(move |b| ops::augment(b, p.repo.as_deref(), &p.query))
            .await
    }
}

#[tool_handler(
    name = "aka-mcp",
    instructions = "Code knowledge graph for the indexed repositories. Start with list_repos, use query to search, context for a 360-degree view of one symbol, and impact before refactoring."
)]
impl ServerHandler for AkaMcpServer {}
