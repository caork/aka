//! aka-mcp — rmcp MCP 服务（stdio），工具：
//! `list_repos` / `query` / `search_code` / `context` / `find_definition`
//! / `search_references` / `impact` / `rename` / `route_map` / `tool_map`
//! / `graphql_map` / `topic_map` / `shape_check` / `api_impact` / `analyze`
//! / `import_repo` / `update_repo` / `augment`。
//!
//! 数据层经 [`backend::Backend`] trait 解耦：真实实现由 CLI / 桌面端注入。

pub mod backend;
pub mod ops;
pub mod service;

use std::net::SocketAddr;
use std::sync::Arc;

use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::ServiceExt;

pub use aka_core::{
    clamp_render_nodes, DEFAULT_RENDER_MAX_NODES, MAX_RENDER_NODES, MIN_RENDER_NODES,
};
pub use backend::{
    Backend, ChangeDetection, ChangedRange, ChangedSymbol, CodeLineMatch, CodeSearchHit,
    CodeSearchResult, DirectoryCount, GraphqlMapEntry, ImpactDirection, ProcessHit,
    QueryEnrichment, RenameEdit, RenamePlan, RepoInfo, RepoProgress, RepoSettingsPatch,
    RepoSettingsUpdate, RouteConsumer, RouteMapEntry, SearchHit, SymbolRef, SymbolSelector,
    ToolMapEntry, TopicEndpoint, TopicMapEntry,
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

/// 在 Streamable HTTP 上跑 MCP 服务，直到 listener 退出。
pub async fn serve_http(backend: Arc<dyn Backend>, addr: SocketAddr) -> anyhow::Result<()> {
    let service = StreamableHttpService::new(
        move || Ok(AkaMcpServer::new(Arc::clone(&backend))),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_allowed_hosts([
            "localhost".to_string(),
            "localhost:4112".to_string(),
            "127.0.0.1".to_string(),
            "127.0.0.1:4112".to_string(),
            "::1".to_string(),
            "[::1]:4112".to_string(),
            addr.to_string(),
        ]),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
