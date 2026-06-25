//! MCP tool handler 输出 JSON 形状测试（直接调用，不走 transport）。

use std::sync::Arc;

use aka_mcp::backend::{Backend, RepoInfo, SearchHit, SymbolRef};
use aka_mcp::service::{
    AkaMcpServer, AnalyzeParams, ApiImpactParams, AugmentParams, CodeSearchParams,
    DetectChangesParams, GraphqlMapParams, ImpactParams, ImportRepoParams, QueryParams,
    ReferencesParams, RenameParams, RouteMapParams, SymbolParams, ToolMapParams, TopicMapParams,
    UpdateRepoParams,
};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use serde_json::Value;

mod support;

use support::fixture_backend::FixtureBackend;

fn server() -> AkaMcpServer {
    AkaMcpServer::new(Arc::new(FixtureBackend::fixture()))
}

/// 取工具结果里的 JSON 文本并解析。
fn text_json(res: &CallToolResult) -> Value {
    assert_ne!(res.is_error, Some(true), "tool errored: {res:?}");
    assert_eq!(res.content.len(), 1, "expect single text content");
    let raw = &res.content[0].as_text().expect("text content").text;
    serde_json::from_str(raw).expect("output is valid JSON")
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object().unwrap().keys().map(String::as_str).collect()
}

#[tokio::test]
async fn list_repos_shape() {
    let res = server().list_repos_without_roots().await.unwrap();
    let v = text_json(&res);
    let repos = v["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0]["name"], "fixture");
    assert_eq!(repos[0]["nodes"], 5);
    assert_eq!(repos[0]["embeddings"], false);
    // beta 未索引：indexed_at 直接省略（token 友好）。
    assert!(repos[1].get("indexed_at").is_none());
    // 合同新增字段：status / source{kind,url} / detail（后两者显式 null）。
    assert_eq!(repos[0]["status"], "ready");
    assert_eq!(repos[0]["source"]["kind"], "local");
    assert!(repos[0]["source"]["url"].is_null());
    assert!(repos[0]["detail"].is_null());
    assert_eq!(repos[1]["source"]["kind"], "git");
    assert_eq!(repos[1]["source"]["url"], "https://example.com/beta.git");
}

#[test]
fn repo_path_aliases_deserialize_for_query_tools() {
    let q: QueryParams =
        serde_json::from_value(serde_json::json!({"repo_path": "/work/service", "query": "Order"}))
            .unwrap();
    assert_eq!(q.repo.as_deref(), Some("/work/service"));

    let s: SymbolParams = serde_json::from_value(
        serde_json::json!({"workspace_path": "/work/service/src", "name": "OrderService"}),
    )
    .unwrap();
    assert_eq!(s.repo.as_deref(), Some("/work/service/src"));
    assert_eq!(s.symbol.as_deref(), Some("OrderService"));

    let refs: ReferencesParams =
        serde_json::from_value(serde_json::json!({"workspace": "/work/service", "target": "save"}))
            .unwrap();
    assert_eq!(refs.repo.as_deref(), Some("/work/service"));

    let impact: ImpactParams =
        serde_json::from_value(serde_json::json!({"path": "/work/service", "target": "save"}))
            .unwrap();
    assert_eq!(impact.repo.as_deref(), Some("/work/service"));

    let code: CodeSearchParams =
        serde_json::from_value(serde_json::json!({"repository": "/work/service", "query": "S3"}))
            .unwrap();
    assert_eq!(code.repo.as_deref(), Some("/work/service"));

    let changes: DetectChangesParams =
        serde_json::from_value(serde_json::json!({"repo_path": "/work/service", "scope": "all"}))
            .unwrap();
    assert_eq!(changes.repo.as_deref(), Some("/work/service"));

    let route: RouteMapParams = serde_json::from_value(
        serde_json::json!({"workspace_path": "/work/service", "route": "/api/orders"}),
    )
    .unwrap();
    assert_eq!(route.repo.as_deref(), Some("/work/service"));

    let tool: ToolMapParams =
        serde_json::from_value(serde_json::json!({"workspace": "/work/service", "tool": "index"}))
            .unwrap();
    assert_eq!(tool.repo.as_deref(), Some("/work/service"));

    let graphql: GraphqlMapParams =
        serde_json::from_value(serde_json::json!({"path": "/work/service", "operation": "order"}))
            .unwrap();
    assert_eq!(graphql.repo.as_deref(), Some("/work/service"));

    let api: ApiImpactParams = serde_json::from_value(
        serde_json::json!({"repository": "/work/service", "route": "/api/orders"}),
    )
    .unwrap();
    assert_eq!(api.repo.as_deref(), Some("/work/service"));

    let augment: AugmentParams =
        serde_json::from_value(serde_json::json!({"repo_path": "/work/service", "query": "Order"}))
            .unwrap();
    assert_eq!(augment.repo.as_deref(), Some("/work/service"));

    let analyze: AnalyzeParams =
        serde_json::from_value(serde_json::json!({"path": "/work/service/src"})).unwrap();
    assert_eq!(analyze.repo_path, "/work/service/src");

    let import_git: ImportRepoParams =
        serde_json::from_value(serde_json::json!({"url": "https://example.com/service.git"}))
            .unwrap();
    assert_eq!(import_git.kind, "git");
    assert_eq!(import_git.src, "https://example.com/service.git");

    let import_local: ImportRepoParams = serde_json::from_value(
        serde_json::json!({"kind": "local", "workspace_path": "/work/service/src"}),
    )
    .unwrap();
    assert_eq!(import_local.kind, "local");
    assert_eq!(import_local.src, "/work/service/src");

    let update: UpdateRepoParams =
        serde_json::from_value(serde_json::json!({"path": "/work/service"})).unwrap();
    assert_eq!(update.repo, "/work/service");
}

#[tokio::test]
async fn query_shape() {
    let res = server()
        .query(Parameters(QueryParams {
            repo: Some("fixture".into()),
            query: "handle".into(),
            limit: None,
            task_context: None,
            goal: None,
            max_symbols: None,
            include_content: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(
        keys(&v),
        ["definitions", "hits", "process_symbols", "processes"]
    );
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!(hit["name"], "handle_request");
    assert_eq!(hit["label"], "Function");
    assert_eq!(hit["file"], "src/handler.rs");
    assert_eq!(hit["line"], 12);
    assert!(hit["score"].as_f64().unwrap() > 0.0);
    assert!(hit["snip"].as_str().unwrap().contains("handle_request"));
    // 流程归属：有归属时给流程名数组（最多 3 个）。
    let procs: Vec<&str> = hit["processes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert_eq!(procs, ["main → read_file", "main → write_output"]);

    let processes = v["processes"].as_array().unwrap();
    assert_eq!(processes.len(), 2);
    assert_eq!(processes[0]["id"], "fixture:proc:request-flow");
    assert_eq!(processes[0]["summary"], "main → read_file");
    assert_eq!(processes[0]["process_type"], "call_chain");
    assert_eq!(processes[0]["step_count"], 4);

    let process_symbols = v["process_symbols"].as_array().unwrap();
    assert_eq!(process_symbols.len(), 2);
    assert_eq!(process_symbols[0]["name"], "handle_request");
    assert_eq!(
        process_symbols[0]["process_id"],
        "fixture:proc:request-flow"
    );
    assert_eq!(process_symbols[0]["step_index"], 2);
    assert_eq!(process_symbols[0]["type"], "Function");
    assert_eq!(process_symbols[0]["filePath"], "src/handler.rs");
    assert_eq!(process_symbols[0]["startLine"], 12);
    assert_eq!(process_symbols[0]["module"], "IO Pipeline");
    assert_eq!(process_symbols[1]["name"], "handle_request");
    assert_eq!(process_symbols[1]["process_id"], "fixture:proc:output-flow");
    assert_eq!(process_symbols[1]["step_index"], 2);
    assert!(v["definitions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn query_omits_processes_when_empty() {
    // beta_main 不在任何流程里 → processes 字段整个省略（token 友好）。
    let res = server()
        .query(Parameters(QueryParams {
            repo: Some("beta".into()),
            query: "beta_main".into(),
            limit: None,
            task_context: None,
            goal: None,
            max_symbols: None,
            include_content: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].get("processes").is_none());
    assert!(v["processes"].as_array().unwrap().is_empty());
    assert!(v["process_symbols"].as_array().unwrap().is_empty());
    assert_eq!(v["definitions"].as_array().unwrap()[0]["name"], "beta_main");
}

#[tokio::test]
async fn query_honors_max_symbols_and_include_content() {
    let res = server()
        .query(Parameters(QueryParams {
            repo: Some("fixture".into()),
            query: "main".into(),
            limit: Some(1),
            task_context: None,
            goal: None,
            max_symbols: Some(1),
            include_content: Some(true),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["processes"].as_array().unwrap().len(), 1);
    let process_symbols = v["process_symbols"].as_array().unwrap();
    assert_eq!(process_symbols.len(), 1);
    assert_eq!(process_symbols[0]["name"], "main");
    assert!(process_symbols[0]["content"]
        .as_str()
        .unwrap()
        .contains("fn main"));
}

#[tokio::test]
async fn query_uses_task_context_for_process_ranking() {
    let res = server()
        .query(Parameters(QueryParams {
            repo: Some("fixture".into()),
            query: "handle".into(),
            limit: Some(2),
            task_context: Some("write output response".into()),
            goal: Some("trace output flow".into()),
            max_symbols: None,
            include_content: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    let processes = v["processes"].as_array().unwrap();
    assert_eq!(processes[0]["id"], "fixture:proc:output-flow");
    assert_eq!(processes[0]["summary"], "main → write_output");
}

#[tokio::test]
async fn query_expands_search_with_task_context_terms() {
    let res = server()
        .query(Parameters(QueryParams {
            repo: Some("fixture".into()),
            query: "investigate".into(),
            limit: Some(5),
            task_context: Some("handle request failure path".into()),
            goal: None,
            max_symbols: None,
            include_content: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits[0]["name"], "handle_request");
}

#[tokio::test]
async fn search_code_shape() {
    let res = server()
        .search_code(Parameters(CodeSearchParams {
            repo: Some("fixture".into()),
            query: "parse_config".into(),
            limit: Some(5),
            context: Some(1),
            regex: false,
            path_filter: Some("src/".into()),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(keys(&v), ["directories", "hits"]);
    let dirs = v["directories"].as_array().unwrap();
    assert_eq!(dirs[0]["dir"], "src");
    assert_eq!(dirs[0]["count"], 3);
    let hits = v["hits"].as_array().unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().any(|h| h["file"] == "src/handler.rs"));
    let first = &hits[0];
    let matches = first["matches"].as_array().unwrap();
    assert!(matches
        .iter()
        .any(|m| { m["matched"] == true && m["text"].as_str().unwrap().contains("parse_config") }));
    assert!(
        matches.iter().any(|m| m["matched"] == false),
        "context lines should be present and marked separately"
    );
}

#[tokio::test]
async fn search_code_regex_shape() {
    let res = server()
        .search_code(Parameters(CodeSearchParams {
            repo: Some("fixture".into()),
            query: "parse_[a-z]+".into(),
            limit: None,
            context: None,
            regex: true,
            path_filter: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert!(!v["hits"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn context_shape() {
    let res = server()
        .context(Parameters(SymbolParams {
            repo: None,
            symbol: Some("handle_request".into()),
            uid: None,
            file_path: None,
            kind: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["symbol"], "handle_request");
    assert_eq!(v["defs"].as_array().unwrap().len(), 1);
    let callers: Vec<_> = v["callers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["name"])
        .collect();
    assert_eq!(callers, [&Value::from("main")]);
    let callees: Vec<_> = v["callees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["name"])
        .collect();
    assert_eq!(
        callees,
        [&Value::from("parse_config"), &Value::from("write_output")]
    );
    assert_eq!(v["refs"].as_array().unwrap()[0]["edge"], "CALLS");
    // 流程归属：ProcessHit 原样序列化（process_id/name/process_type/step/step_count）。
    let procs = v["processes"].as_array().unwrap();
    assert_eq!(procs.len(), 2);
    assert_eq!(procs[0]["process_id"], "fixture:proc:request-flow");
    assert_eq!(procs[0]["name"], "main → read_file");
    assert_eq!(procs[0]["process_type"], "call_chain");
    assert_eq!(procs[0]["step"], 2);
    assert_eq!(procs[0]["step_count"], 4);
    assert_eq!(procs[1]["process_id"], "fixture:proc:output-flow");
}

#[tokio::test]
async fn context_ambiguous_symbol_returns_candidates_and_uid_disambiguates() {
    let res = server()
        .context(Parameters(SymbolParams {
            repo: Some("fixture".into()),
            symbol: Some("duplicate".into()),
            uid: None,
            file_path: None,
            kind: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["status"], "ambiguous");
    let candidates = v["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0]["id"], "fixture:fn:duplicate_lib");
    assert_eq!(candidates[1]["id"], "fixture:fn:duplicate_ui");
    assert!(v["defs"].as_array().unwrap().is_empty());

    let res = server()
        .context(Parameters(SymbolParams {
            repo: Some("fixture".into()),
            symbol: Some("duplicate".into()),
            uid: Some("fixture:fn:duplicate_ui".into()),
            file_path: None,
            kind: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["status"], "ok");
    let defs = v["defs"].as_array().unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0]["id"], "fixture:fn:duplicate_ui");
    assert_eq!(defs[0]["file"], "src/b.rs");
}

#[tokio::test]
async fn find_definition_shape() {
    let res = server()
        .find_definition(Parameters(SymbolParams {
            repo: None,
            symbol: Some("parse_config".into()),
            uid: None,
            file_path: None,
            kind: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    let defs = v["defs"].as_array().unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0]["file"], "src/config.rs");
    assert_eq!(defs[0]["line"], 8);

    // 未知符号 → 空数组而不是错误。
    let res = server()
        .find_definition(Parameters(SymbolParams {
            repo: None,
            symbol: Some("nope".into()),
            uid: None,
            file_path: None,
            kind: None,
        }))
        .await
        .unwrap();
    assert!(text_json(&res)["defs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn search_references_shape() {
    let res = server()
        .search_references(Parameters(ReferencesParams {
            repo: None,
            symbol: Some("read_file".into()),
            uid: None,
            file_path: None,
            kind: None,
            limit: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    let refs = v["refs"].as_array().unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0]["name"], "parse_config");
    assert_eq!(refs[0]["edge"], "CALLS");
    assert_eq!(refs[0]["depth"], 1);
}

#[tokio::test]
async fn impact_shape() {
    let res = server()
        .impact(Parameters(ImpactParams {
            repo: None,
            symbol: Some("read_file".into()),
            uid: None,
            file_path: None,
            kind: None,
            direction: None,
            depth: None, // 默认 2 跳
            limit: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["count"], 2);
    let impacted = v["impacted"].as_array().unwrap();
    assert_eq!(impacted[0]["name"], "parse_config");
    assert_eq!(impacted[0]["depth"], 1);
    assert_eq!(impacted[1]["name"], "handle_request");
    assert_eq!(impacted[1]["depth"], 2);

    // 流程视角：受影响集合 = read_file(4) + parse_config(3) + handle_request(2)。
    // request-flow 三个符号受波及、最早断在第 2 步；output-flow 只波及 handle_request。
    let procs = v["affected_processes"].as_array().unwrap();
    assert_eq!(procs.len(), 2);
    assert_eq!(procs[0]["process_id"], "fixture:proc:request-flow");
    assert_eq!(procs[0]["name"], "main → read_file");
    assert_eq!(procs[0]["process_type"], "call_chain");
    assert_eq!(procs[0]["step_count"], 4);
    assert_eq!(procs[0]["first_affected_step"], 2);
    assert_eq!(procs[0]["affected_symbols"], 3);
    assert_eq!(procs[1]["process_id"], "fixture:proc:output-flow");
    assert_eq!(procs[1]["affected_symbols"], 1);
    assert_eq!(procs[1]["first_affected_step"], 2);
}

#[tokio::test]
async fn impact_supports_downstream_direction() {
    let res = server()
        .impact(Parameters(ImpactParams {
            repo: Some("fixture".into()),
            symbol: Some("handle_request".into()),
            uid: None,
            file_path: None,
            kind: None,
            direction: Some("downstream".into()),
            depth: Some(1),
            limit: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["status"], "ok");
    assert_eq!(v["direction"], "downstream");
    let names: Vec<_> = v["impacted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["parse_config", "write_output"]);
    assert_eq!(v["by_depth"].as_array().unwrap()[0]["count"], 2);
}

#[tokio::test]
async fn impact_affected_processes_empty_without_membership() {
    // beta_main 没有流程数据 → affected_processes 为空数组（字段始终存在）。
    let res = server()
        .impact(Parameters(ImpactParams {
            repo: Some("beta".into()),
            symbol: Some("beta_main".into()),
            uid: None,
            file_path: None,
            kind: None,
            direction: None,
            depth: None,
            limit: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert!(v["affected_processes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn analyze_shape() {
    let res = server()
        .analyze(Parameters(AnalyzeParams {
            repo_path: "/tmp/fixture".into(),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(keys(&v), ["repo", "status", "summary"]);
    assert_eq!(v["repo"], "fixture");
    assert_eq!(v["status"], "indexing");
    assert!(v["summary"].as_str().unwrap().contains("/tmp/fixture"));
}

#[tokio::test]
async fn import_repo_shape() {
    let res = server()
        .import_repo(Parameters(ImportRepoParams {
            kind: "git".into(),
            src: "https://github.com/example/service.git".into(),
            name: Some("service".into()),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(keys(&v), ["repo", "status", "summary"]);
    assert_eq!(v["repo"], "service");
    assert_eq!(v["status"], "indexing");
    assert_eq!(
        v["summary"],
        "indexing scheduled: service (git import + analyze)"
    );
}

#[tokio::test]
async fn update_repo_shape() {
    let res = server()
        .update_repo(Parameters(UpdateRepoParams {
            repo: "fixture".into(),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(keys(&v), ["repo", "status", "summary"]);
    assert_eq!(v["repo"], "fixture");
    assert_eq!(v["status"], "indexing");
    assert_eq!(v["summary"], "update scheduled: fixture (re-analyze)");
}

#[tokio::test]
async fn detect_changes_shape() {
    let res = server()
        .detect_changes(Parameters(DetectChangesParams {
            repo: Some("fixture".into()),
            scope: Some("unstaged".into()),
            base_ref: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["repo"], "fixture");
    assert_eq!(v["scope"], "unstaged");
    assert_eq!(v["changed_count"], 1);
    assert_eq!(
        v["changed_ranges"].as_array().unwrap()[0]["file"],
        "src/handler.rs"
    );
    let symbols = v["changed_symbols"].as_array().unwrap();
    assert_eq!(symbols[0]["id"], "fixture:fn:handle_request");
    assert_eq!(
        symbols[0]["ranges"].as_array().unwrap()[0]["start_line"],
        12
    );
    let procs = v["affected_processes"].as_array().unwrap();
    assert_eq!(procs.len(), 2);
    let request_flow = procs
        .iter()
        .find(|p| p["process_id"] == "fixture:proc:request-flow")
        .unwrap();
    assert_eq!(request_flow["first_affected_step"], 2);
}

#[tokio::test]
async fn route_map_shape() {
    let res = server()
        .route_map(Parameters(RouteMapParams {
            repo: Some("fixture".into()),
            route: Some("/api".into()),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["total"], 1);
    let route = &v["routes"].as_array().unwrap()[0];
    assert_eq!(route["route"], "/api/config");
    assert_eq!(route["handler"], "src/routes/config.rs");
    assert_eq!(route["middleware"].as_array().unwrap()[0], "withAuth");
    assert_eq!(route["responseKeys"].as_array().unwrap()[0], "data");
    assert_eq!(
        route["consumers"].as_array().unwrap()[0]["name"],
        "ConfigPanel"
    );
    assert_eq!(
        route["consumers"].as_array().unwrap()[0]["accessedKeys"]
            .as_array()
            .unwrap()[1],
        "missing"
    );
    assert_eq!(route["flows"].as_array().unwrap()[0], "main → read_file");
}

#[tokio::test]
async fn tool_map_shape() {
    let res = server()
        .tool_map(Parameters(ToolMapParams {
            repo: Some("fixture".into()),
            tool: Some("index".into()),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["total"], 1);
    let tool = &v["tools"].as_array().unwrap()[0];
    assert_eq!(tool["name"], "index_repo");
    assert_eq!(tool["filePath"], "src/tools/index_repo.rs");
    assert!(tool["description"].as_str().unwrap().contains("Index"));
    assert_eq!(
        tool["handlers"].as_array().unwrap()[0]["name"],
        "handle_request"
    );
    assert_eq!(tool["flows"].as_array().unwrap()[0], "main → write_output");
}

#[tokio::test]
async fn graphql_map_shape() {
    let res = server()
        .graphql_map(Parameters(GraphqlMapParams {
            repo: Some("fixture".into()),
            operation: Some("order".into()),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["total"], 1);
    let operation = &v["operations"].as_array().unwrap()[0];
    assert_eq!(operation["name"], "order");
    assert_eq!(operation["operationType"], "query");
    assert_eq!(operation["filePath"], "src/graphql/schema.rs");
    assert_eq!(
        operation["handlers"].as_array().unwrap()[0]["name"],
        "handle_request"
    );
    assert_eq!(
        operation["flows"].as_array().unwrap()[0],
        "main → read_file"
    );
}

#[tokio::test]
async fn topic_map_shape() {
    let res = server()
        .topic_map(Parameters(TopicMapParams {
            repo: Some("fixture".into()),
            topic: Some("orders".into()),
            broker: Some("kafka".into()),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["total"], 1);
    let topic = &v["topics"].as_array().unwrap()[0];
    assert_eq!(topic["name"], "orders.created");
    assert_eq!(topic["broker"], "kafka");
    assert_eq!(topic["source"], "oss-analyzer-fixture");
    assert_eq!(
        topic["consumerGroups"].as_array().unwrap()[0],
        "orders-service"
    );
    assert_eq!(
        topic["producers"].as_array().unwrap()[0]["name"],
        "handle_request"
    );
    assert_eq!(
        topic["consumers"].as_array().unwrap()[0]["filePath"],
        "src/io.rs"
    );
    assert_eq!(topic["flows"].as_array().unwrap()[0], "main → read_file");
}

#[tokio::test]
async fn shape_check_reports_mismatch() {
    let res = server()
        .shape_check(Parameters(RouteMapParams {
            repo: Some("fixture".into()),
            route: Some("config".into()),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["total"], 1);
    assert_eq!(v["mismatches"], 1);
    let route = &v["routes"].as_array().unwrap()[0];
    assert_eq!(route["status"], "MISMATCH");
    let consumer = &route["consumers"].as_array().unwrap()[0];
    assert_eq!(consumer["mismatched"].as_array().unwrap()[0], "missing");
    assert_eq!(consumer["mismatchConfidence"], "high");
}

#[tokio::test]
async fn api_impact_shape() {
    let res = server()
        .api_impact(Parameters(ApiImpactParams {
            repo: Some("fixture".into()),
            route: Some("/api/config".into()),
            file: None,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["total"], 1);
    let route = &v["route"];
    assert_eq!(route["route"], "/api/config");
    assert_eq!(
        route["responseShape"]["success"].as_array().unwrap()[0],
        "data"
    );
    assert_eq!(
        route["consumers"].as_array().unwrap()[0]["file"],
        "src/ui/config_panel.tsx"
    );
    assert_eq!(
        route["mismatches"].as_array().unwrap()[0]["field"],
        "missing"
    );
    assert_eq!(route["impactSummary"]["riskLevel"], "MEDIUM");
}

#[tokio::test]
async fn augment_shape() {
    let res = server()
        .augment(Parameters(AugmentParams {
            repo: None,
            query: "main".into(),
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    let items = v["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "substring 'main' → main + beta_main");
    let first = &items[0];
    assert_eq!(first["hit"]["name"], "main");
    assert!(first["callers"].as_array().unwrap().is_empty());
    let callees: Vec<_> = first["callees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["name"])
        .collect();
    assert_eq!(callees, [&Value::from("handle_request")]);
}

#[tokio::test]
async fn rename_shape_defaults_to_dry_run() {
    let res = server()
        .rename(Parameters(RenameParams {
            repo: Some("fixture".into()),
            symbol: Some("parse_config".into()),
            new_symbol: "parse_settings".into(),
            uid: None,
            file_path: None,
            kind: None,
            dry_run: true,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["status"], "ok");
    assert_eq!(v["target"], "parse_config");
    assert_eq!(v["replacement"], "parse_settings");
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["applied"], false);
    assert_eq!(v["changed_files"], 1);
    assert_eq!(v["count"], 1);
    let edit = &v["edits"].as_array().unwrap()[0];
    assert_eq!(edit["file"], "src/handler.rs");
    assert_eq!(edit["line"], 12);
    assert_eq!(edit["column"], 9);
    assert!(edit["before"].as_str().unwrap().contains("parse_config"));
    assert!(edit["after"].as_str().unwrap().contains("parse_settings"));
}

#[tokio::test]
async fn rename_reports_ambiguous_candidates() {
    let res = server()
        .rename(Parameters(RenameParams {
            repo: Some("fixture".into()),
            symbol: Some("duplicate".into()),
            new_symbol: "deduped".into(),
            uid: None,
            file_path: None,
            kind: None,
            dry_run: true,
        }))
        .await
        .unwrap();
    let v = text_json(&res);
    assert_eq!(v["status"], "ambiguous");
    assert_eq!(v["count"], 0);
    assert_eq!(v["candidates"].as_array().unwrap().len(), 2);
    assert!(v["message"]
        .as_str()
        .unwrap()
        .contains("Multiple definitions"));
}

// ---- 错误路径：Backend 故障必须变成 in-band tool error，不能炸协议 ----

struct FailBackend;

impl Backend for FailBackend {
    fn list_repos(&self) -> anyhow::Result<Vec<RepoInfo>> {
        anyhow::bail!("index corrupted")
    }
    fn search(&self, _: Option<&str>, _: &str, _: usize) -> anyhow::Result<Vec<SearchHit>> {
        anyhow::bail!("index corrupted")
    }
    fn find_definition(&self, _: Option<&str>, _: &str) -> anyhow::Result<Vec<SearchHit>> {
        anyhow::bail!("index corrupted")
    }
    fn references(&self, _: Option<&str>, _: &str, _: usize) -> anyhow::Result<Vec<SymbolRef>> {
        anyhow::bail!("index corrupted")
    }
    fn callers(&self, _: Option<&str>, _: &str, _: u32) -> anyhow::Result<Vec<SymbolRef>> {
        anyhow::bail!("index corrupted")
    }
    fn callees(&self, _: Option<&str>, _: &str, _: u32) -> anyhow::Result<Vec<SymbolRef>> {
        anyhow::bail!("index corrupted")
    }
    fn impact(&self, _: Option<&str>, _: &str, _: u32, _: usize) -> anyhow::Result<Vec<SymbolRef>> {
        anyhow::bail!("index corrupted")
    }
    fn analyze(&self, _: &str) -> anyhow::Result<String> {
        anyhow::bail!("index corrupted")
    }
}

#[tokio::test]
async fn backend_error_is_in_band() {
    let server = AkaMcpServer::new(Arc::new(FailBackend));
    let res = server
        .query(Parameters(QueryParams {
            repo: None,
            query: "x".into(),
            limit: None,
            task_context: None,
            goal: None,
            max_symbols: None,
            include_content: None,
        }))
        .await
        .unwrap(); // 协议层 Ok
    assert_eq!(res.is_error, Some(true));
    let text = &res.content[0].as_text().unwrap().text;
    assert!(text.contains("index corrupted"));
}
