//! 端到端：内存 duplex transport 上跑完整 MCP 会话
//! （initialize → tools/list → tools/call query）。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aka_mcp::backend::Backend;
use aka_mcp::AkaMcpServer;
use rmcp::handler::client::ClientHandler;
use rmcp::model::{CallToolRequestParams, ListRootsResult, Root};
use rmcp::ServiceExt;

mod support;

use support::fixture_backend::FixtureBackend;

const EXPECTED_TOOLS: [&str; 17] = [
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
    "import_repo",
    "update_repo",
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

    // tools/list：工具齐全，query 的 schema 带必填参数。
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

#[derive(Clone)]
struct RootsClient {
    roots: Vec<Root>,
}

impl ClientHandler for RootsClient {
    async fn list_roots(
        &self,
        _: rmcp::service::RequestContext<rmcp::service::RoleClient>,
    ) -> Result<ListRootsResult, rmcp::ErrorData> {
        Ok(ListRootsResult::new(self.roots.clone()))
    }
}

struct RecordingBackend {
    inner: FixtureBackend,
    queued: Arc<Mutex<Vec<Vec<PathBuf>>>>,
}

impl RecordingBackend {
    fn new(queued: Arc<Mutex<Vec<Vec<PathBuf>>>>) -> Self {
        Self {
            inner: FixtureBackend::fixture(),
            queued,
        }
    }
}

impl Backend for RecordingBackend {
    fn queue_workspaces(&self, roots: &[PathBuf]) -> anyhow::Result<Vec<String>> {
        self.queued.lock().unwrap().push(roots.to_vec());
        Ok(vec!["workspace".into()])
    }

    fn list_repos(&self) -> anyhow::Result<Vec<aka_mcp::RepoInfo>> {
        self.inner.list_repos()
    }

    fn search(
        &self,
        repo: Option<&str>,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<aka_mcp::SearchHit>> {
        self.inner.search(repo, query, limit)
    }

    fn find_definition(
        &self,
        repo: Option<&str>,
        symbol: &str,
    ) -> anyhow::Result<Vec<aka_mcp::SearchHit>> {
        self.inner.find_definition(repo, symbol)
    }

    fn references(
        &self,
        repo: Option<&str>,
        symbol: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<aka_mcp::SymbolRef>> {
        self.inner.references(repo, symbol, limit)
    }

    fn callers(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
    ) -> anyhow::Result<Vec<aka_mcp::SymbolRef>> {
        self.inner.callers(repo, symbol, depth)
    }

    fn callees(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
    ) -> anyhow::Result<Vec<aka_mcp::SymbolRef>> {
        self.inner.callees(repo, symbol, depth)
    }

    fn impact(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
        limit: usize,
    ) -> anyhow::Result<Vec<aka_mcp::SymbolRef>> {
        self.inner.impact(repo, symbol, depth, limit)
    }

    fn analyze(&self, repo_path: &str) -> anyhow::Result<String> {
        self.inner.analyze(repo_path)
    }
}

#[tokio::test]
async fn tool_call_queues_client_roots_before_query() -> anyhow::Result<()> {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);
    let queued = Arc::new(Mutex::new(Vec::new()));
    let handler = AkaMcpServer::new(Arc::new(RecordingBackend::new(Arc::clone(&queued))));
    let server_task = tokio::spawn(async move {
        let svc = handler.serve(server_io).await.expect("server initialize");
        let _ = svc.waiting().await;
    });
    let root = std::env::temp_dir().join("aka mcp root test");
    let uri = format!("file://{}", root.to_string_lossy().replace(' ', "%20"));
    let client = RootsClient {
        roots: vec![
            Root::new(uri),
            Root::new("file://example.com/remote"),
            Root::new("https://example.com/repo"),
        ],
    }
    .serve(client_io)
    .await?;

    let args = serde_json::json!({ "repo": "fixture", "query": "handle" });
    let result = client
        .call_tool(
            CallToolRequestParams::new("query").with_arguments(args.as_object().unwrap().clone()),
        )
        .await?;

    assert_ne!(result.is_error, Some(true));
    let queued = queued.lock().unwrap().clone();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0], vec![root]);

    client.cancel().await?;
    server_task.await?;
    Ok(())
}
