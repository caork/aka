//! 手测入口：用内存 MockBackend 在 stdio 上跑 MCP 服务。
//!
//! ```bash
//! cargo run -p aka-mcp --bin aka-mcp-mock
//! # 或挂到 Claude Code：claude mcp add aka-mock -- cargo run -q -p aka-mcp --bin aka-mcp-mock
//! ```

use std::sync::Arc;

use aka_mcp::{serve_stdio, MockBackend};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // stdio 是协议通道，日志只能去 stderr。
    eprintln!("aka-mcp-mock: serving MCP on stdio (mock backend, repos: demo/beta)");
    serve_stdio(Arc::new(MockBackend::demo())).await
}
