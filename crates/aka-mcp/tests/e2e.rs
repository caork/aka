//! 端到端：内存 duplex transport 上跑完整 MCP 会话
//! （initialize → tools/list → tools/call query）。

use std::collections::HashSet;
use std::sync::Arc;

use aka_mcp::AkaMcpServer;
use rmcp::model::CallToolRequestParams;
use rmcp::ServiceExt;

mod support;

use support::fixture_backend::FixtureBackend;

const EXPECTED_TOOLS: [&str; 15] = [
    "list_repos",
    "query",
    "search_code",
    "context",
    "find_definition",
    "search_references",
    "impact",
    "detect_changes",
    "route_map",
    "tool_map",
    "graphql_map",
    "shape_check",
    "api_impact",
    "analyze",
    "augment",
];

#[tokio::test]
async fn initialize_list_and_call_query() -> anyhow::Result<()> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);

    let handler = AkaMcpServer::new(Arc::new(FixtureBackend::fixture()));
    let server_task = tokio::spawn(async move {
        let svc = handler.serve(server_io).await.expect("server initialize");
        let _ = svc.waiting().await;
    });

    // `()` 是 rmcp 自带的最小 ClientHandler；serve() 内部完成 initialize 握手。
    let client = ().serve(client_io).await?;
    let info = client.peer_info().expect("server info after initialize");
    assert_eq!(info.server_info.name, "aka-mcp");

    // tools/list：九个工具齐全，query 的 schema 带必填参数。
    let tools = client.list_all_tools().await?;
    let names: HashSet<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in EXPECTED_TOOLS {
        assert!(
            names.contains(expected),
            "missing tool {expected}, got {names:?}"
        );
    }
    let query_tool = tools.iter().find(|t| t.name == "query").unwrap();
    let schema = serde_json::to_value(query_tool.input_schema.as_ref())?;
    assert!(schema["properties"].get("query").is_some());
    assert!(schema["properties"].get("repo").is_some());
    assert!(schema["properties"].get("limit").is_some());
    let list_tool = tools.iter().find(|t| t.name == "list_repos").unwrap();
    let list_description = list_tool.description.as_deref().unwrap_or_default();
    assert!(list_description.contains("every tool call"));
    assert!(list_description.contains("workspace roots"));

    // tools/call query
    let args = serde_json::json!({ "repo": "fixture", "query": "handle" });
    let result = client
        .call_tool(
            CallToolRequestParams::new("query").with_arguments(args.as_object().unwrap().clone()),
        )
        .await?;
    assert_ne!(result.is_error, Some(true));
    let body: serde_json::Value = serde_json::from_str(&result.content[0].as_text().unwrap().text)?;
    assert_eq!(body["hits"][0]["name"], "handle_request");
    assert_eq!(body["hits"][0]["file"], "src/handler.rs");

    client.cancel().await?;
    server_task.await?;
    Ok(())
}
