//! aka-mcp — rmcp MCP 服务（stdio），八个工具：
//! `list_repos` / `query` / `context` / `find_definition` / `search_references`
//! / `impact` / `analyze` / `augment`。
//!
//! 数据层经 [`backend::Backend`] trait 解耦：真实实现（aka-search / aka-graph）
//! 由集成批次接入；[`mock::MockBackend`] 提供内存假数据用于测试与手测
//! （`cargo run -p aka-mcp --bin aka-mcp-mock`）。

pub mod backend;
pub mod mock;
pub mod ops;
pub mod service;

use std::sync::Arc;

use rmcp::ServiceExt;

pub use aka_core::{
    clamp_render_nodes, DEFAULT_RENDER_MAX_NODES, MAX_RENDER_NODES, MIN_RENDER_NODES,
};
pub use backend::{Backend, ProcessHit, RepoInfo, RepoSettingsUpdate, SearchHit, SymbolRef};
pub use mock::MockBackend;
pub use service::AkaMcpServer;

/// 在 stdio 上跑 MCP 服务，直到客户端断开。
pub async fn serve_stdio(backend: Arc<dyn Backend>) -> anyhow::Result<()> {
    let service = AkaMcpServer::new(backend)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
