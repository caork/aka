//! aka-mcp — rmcp MCP 服务（stdio），工具：
//! `list_repos` / `query` / `search_code` / `context` / `find_definition`
//! / `search_references` / `impact` / `analyze` / `augment`。
//!
//! 数据层经 [`backend::Backend`] trait 解耦：真实实现由 CLI / 桌面端注入。

pub mod backend;
pub mod ops;
pub mod service;

use std::sync::Arc;

use rmcp::ServiceExt;

pub use aka_core::{
    clamp_render_nodes, DEFAULT_RENDER_MAX_NODES, MAX_RENDER_NODES, MIN_RENDER_NODES,
};
pub use backend::{
    Backend, ChangeDetection, ChangedRange, ChangedSymbol, CodeLineMatch, CodeSearchHit,
    CodeSearchResult, DirectoryCount, ImpactDirection, ProcessHit, QueryEnrichment, RepoInfo,
    RepoProgress, RepoSettingsUpdate, RouteConsumer, RouteMapEntry, SearchHit, SymbolRef,
    SymbolSelector, ToolMapEntry,
};
pub use service::AkaMcpServer;

/// 在 stdio 上跑 MCP 服务，直到客户端断开。
pub async fn serve_stdio(backend: Arc<dyn Backend>) -> anyhow::Result<()> {
    let service = AkaMcpServer::new(backend)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
