//! HTTP API 集成测试：tower `oneshot` 直接驱动 Router，不开真实端口。

use std::sync::{Arc, OnceLock};

use aka_server::{router, Backend, RepoInfo, RepoSettingsUpdate, SearchHit, SymbolRef};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

mod support;

use support::fixture_backend::FixtureBackend;

struct EnvGuard {
    aka_home: Option<std::ffi::OsString>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.aka_home {
            std::env::set_var("AKA_HOME", value);
        } else {
            std::env::remove_var("AKA_HOME");
        }
    }
}

async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

fn isolate_aka_home(name: &str) -> EnvGuard {
    let guard = EnvGuard {
        aka_home: std::env::var_os("AKA_HOME"),
    };
    let home =
        std::env::temp_dir().join(format!("aka-server-settings-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::env::set_var("AKA_HOME", home);
    guard
}

fn app() -> axum::Router {
    router(Arc::new(FixtureBackend::fixture()))
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("response body is JSON")
}

fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn health_ok() {
    let res = app()
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["status"], "ok");
}

#[tokio::test]
async fn app_settings_default_and_update() {
    let _lock = env_lock().await;
    let _guard = isolate_aka_home("api");

    let res = app()
        .oneshot(Request::get("/api/settings").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["indexMaxSecs"], 60);

    let res = app()
        .oneshot(post_json("/api/settings", json!({ "indexMaxSecs": 3 })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["indexMaxSecs"], 10);
}

#[tokio::test]
async fn repos_lists_fixture_data() {
    let res = app()
        .oneshot(Request::get("/api/repos").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    let repos = v["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0]["name"], "fixture");
    assert_eq!(repos[0]["nodes"], 5);
    // 合同字段：status / source{kind,url} / detail / render_max_nodes。
    assert_eq!(repos[0]["status"], "ready");
    assert_eq!(repos[0]["source"]["kind"], "local");
    assert!(repos[0]["source"]["url"].is_null());
    assert!(repos[0]["detail"].is_null());
    // render_max_nodes 必须显式出现且为 null（未设置 = 默认）。
    assert!(repos[0]
        .as_object()
        .unwrap()
        .contains_key("render_max_nodes"));
    assert!(repos[0]["render_max_nodes"].is_null());
    assert_eq!(repos[1]["source"]["kind"], "git");
    assert_eq!(repos[1]["source"]["url"], "https://example.com/beta.git");
}

#[tokio::test]
async fn query_returns_hits() {
    let res = app()
        .oneshot(post_json(
            "/api/query",
            json!({ "repo": "fixture", "query": "handle" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["name"], "handle_request");
    assert_eq!(hits[0]["file"], "src/handler.rs");
    assert_eq!(hits[0]["line"], 12);
    let processes = v["processes"].as_array().unwrap();
    assert_eq!(processes[0]["id"], "fixture:proc:request-flow");
    assert_eq!(processes[0]["summary"], "main → read_file");
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
    assert!(v["definitions"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn detect_changes_returns_changed_symbols_and_processes() {
    let res = app()
        .oneshot(post_json(
            "/api/detect-changes",
            json!({ "repo": "fixture", "scope": "unstaged" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["repo"], "fixture");
    assert_eq!(v["changed_count"], 1);
    assert_eq!(v["changed_symbols"][0]["id"], "fixture:fn:handle_request");
    assert_eq!(v["affected_processes"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn route_tool_shape_and_api_impact_endpoints() {
    let res = app()
        .oneshot(post_json(
            "/api/route-map",
            json!({ "repo": "fixture", "route": "config" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["routes"][0]["route"], "/api/config");
    assert_eq!(v["routes"][0]["consumers"][0]["accessedKeys"][1], "missing");

    let res = app()
        .oneshot(post_json(
            "/api/tool-map",
            json!({ "repo": "fixture", "tool": "index" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["tools"][0]["name"], "index_repo");
    assert_eq!(v["tools"][0]["handlers"][0]["name"], "handle_request");

    let res = app()
        .oneshot(post_json(
            "/api/graphql-map",
            json!({ "repo": "fixture", "operation": "order" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["operations"][0]["name"], "order");
    assert_eq!(v["operations"][0]["operationType"], "query");
    assert_eq!(v["operations"][0]["handlers"][0]["name"], "handle_request");

    let res = app()
        .oneshot(post_json(
            "/api/topic-map",
            json!({ "repo": "fixture", "topic": "orders", "broker": "kafka" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["topics"][0]["name"], "orders.created");
    assert_eq!(v["topics"][0]["broker"], "kafka");
    assert_eq!(v["topics"][0]["producers"][0]["name"], "handle_request");
    assert_eq!(v["topics"][0]["flows"][0], "main → read_file");

    let res = app()
        .oneshot(post_json(
            "/api/shape-check",
            json!({ "repo": "fixture", "route": "config" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["mismatches"], 1);
    assert_eq!(v["routes"][0]["status"], "MISMATCH");

    let res = app()
        .oneshot(post_json(
            "/api/api-impact",
            json!({ "repo": "fixture", "route": "config" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["route"]["impactSummary"]["riskLevel"], "MEDIUM");
}

#[tokio::test]
async fn search_code_returns_raw_matches_and_directory_distribution() {
    let res = app()
        .oneshot(post_json(
            "/api/search/code",
            json!({ "repo": "fixture", "query": "parse_config", "context": 1 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    let dirs = v["directories"].as_array().unwrap();
    assert_eq!(dirs[0]["dir"], "src");
    assert_eq!(dirs[0]["count"], 3);
    let hits = v["hits"].as_array().unwrap();
    assert!(!hits.is_empty());
    assert!(hits.iter().any(|h| h["file"] == "src/handler.rs"));
    let matches = hits[0]["matches"].as_array().unwrap();
    assert!(matches
        .iter()
        .any(|m| { m["matched"] == true && m["text"].as_str().unwrap().contains("parse_config") }));
    assert!(
        matches.iter().any(|m| m["matched"] == false),
        "context lines should be present and marked separately"
    );
}

#[tokio::test]
async fn symbol_context_aggregates() {
    let res = app()
        .oneshot(post_json(
            "/api/symbol/context",
            json!({ "symbol": "handle_request" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["symbol"], "handle_request");
    assert_eq!(v["defs"].as_array().unwrap().len(), 1);
    assert_eq!(v["callers"].as_array().unwrap()[0]["name"], "main");
    assert_eq!(v["callees"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn graph_lod_is_501_on_unsupported_backend() {
    // FixtureBackend 不覆写 graph_lod → 默认 "not supported" → 501。
    let res = app()
        .oneshot(
            Request::get("/api/graph/lod?repo=fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    let v = body_json(res).await;
    assert!(v["error"].as_str().unwrap().contains("not supported"));
}

#[tokio::test]
async fn graph_lod_missing_repo_is_400() {
    let res = app()
        .oneshot(Request::get("/api/graph/lod").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cors_allows_localhost_only() {
    // localhost 来源 → 回显 allow-origin
    let res = app()
        .oneshot(
            Request::get("/api/health")
                .header(header::ORIGIN, "http://localhost:5173")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("http://localhost:5173")
    );

    // 外部来源 → 不带 CORS 头（浏览器侧会拒绝）
    let res = app()
        .oneshot(
            Request::get("/api/health")
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(res
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());
}

// ---- 仓库管理端点：Mock 默认不支持 → 501；参数错误 → 400 ----

fn multipart_body(boundary: &str, name: Option<&str>, file: Option<&[u8]>) -> Vec<u8> {
    let mut body = Vec::new();
    if let Some(n) = name {
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{n}\r\n")
                .as_bytes(),
        );
    }
    if let Some(f) = file {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"u.zip\"\r\nContent-Type: application/zip\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(f);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

fn multipart_req(uri: &str, boundary: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn import_invalid_kind_is_400() {
    let res = app()
        .oneshot(post_json(
            "/api/repos/import",
            json!({ "kind": "svn", "url": "x" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // kind 对但缺 src → 400。
    let res = app()
        .oneshot(post_json("/api/repos/import", json!({ "kind": "git" })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn management_endpoints_are_501_on_fixture() {
    // Mock 不覆写仓库管理方法 → 默认 bail "not supported" → 501。
    let cases: Vec<Request<Body>> = vec![
        post_json(
            "/api/repos/import",
            json!({ "kind": "git", "url": "https://x/y.git" }),
        ),
        post_json(
            "/api/repos/fixture/settings",
            json!({ "embeddings_enabled": true }),
        ),
        Request::post("/api/repos/fixture/update")
            .body(Body::empty())
            .unwrap(),
        Request::delete("/api/repos/fixture")
            .body(Body::empty())
            .unwrap(),
        Request::get("/api/node?repo=fixture&id=fixture:fn:main")
            .body(Body::empty())
            .unwrap(),
        Request::get("/api/graph/ego?repo=fixture&id=fixture:fn:main")
            .body(Body::empty())
            .unwrap(),
        Request::get("/api/source?repo=fixture&path=src/main.rs")
            .body(Body::empty())
            .unwrap(),
        Request::get("/api/file/symbols?repo=fixture&path=src/main.rs")
            .body(Body::empty())
            .unwrap(),
        Request::get("/api/files?repo=fixture")
            .body(Body::empty())
            .unwrap(),
        multipart_req(
            "/api/repos/import-zip",
            "XB",
            multipart_body("XB", Some("z"), Some(b"PK")),
        ),
        multipart_req(
            "/api/repos/fixture/update-zip",
            "XB",
            multipart_body("XB", None, Some(b"PK")),
        ),
    ];
    for req in cases {
        let uri = req.uri().clone();
        let res = app().oneshot(req).await.unwrap();
        assert_eq!(
            res.status(),
            StatusCode::NOT_IMPLEMENTED,
            "{uri} should be 501 on fixture"
        );
        let v = body_json(res).await;
        assert!(v["error"].as_str().unwrap().contains("not supported"));
    }
}

#[tokio::test]
async fn import_zip_missing_fields_is_400() {
    // 缺 file 字段。
    let res = app()
        .oneshot(multipart_req(
            "/api/repos/import-zip",
            "XB",
            multipart_body("XB", Some("z"), None),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // update-zip 缺 file 字段。
    let res = app()
        .oneshot(multipart_req(
            "/api/repos/fixture/update-zip",
            "XB",
            multipart_body("XB", None, None),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ---- 仓库管理端点：覆写了管理方法的 Backend → 202/200 + 合同形状 ----

struct ManagedBackend;

impl Backend for ManagedBackend {
    fn list_repos(&self) -> anyhow::Result<Vec<RepoInfo>> {
        Ok(vec![])
    }
    fn search(&self, _: Option<&str>, _: &str, _: usize) -> anyhow::Result<Vec<SearchHit>> {
        Ok(vec![])
    }
    fn find_definition(&self, _: Option<&str>, _: &str) -> anyhow::Result<Vec<SearchHit>> {
        Ok(vec![])
    }
    fn references(&self, _: Option<&str>, _: &str, _: usize) -> anyhow::Result<Vec<SymbolRef>> {
        Ok(vec![])
    }
    fn callers(&self, _: Option<&str>, _: &str, _: u32) -> anyhow::Result<Vec<SymbolRef>> {
        Ok(vec![])
    }
    fn callees(&self, _: Option<&str>, _: &str, _: u32) -> anyhow::Result<Vec<SymbolRef>> {
        Ok(vec![])
    }
    fn impact(&self, _: Option<&str>, _: &str, _: u32, _: usize) -> anyhow::Result<Vec<SymbolRef>> {
        Ok(vec![])
    }
    fn analyze(&self, _: &str) -> anyhow::Result<String> {
        Ok("ok".into())
    }

    fn import_repo(&self, kind: &str, src: &str, name: Option<&str>) -> anyhow::Result<String> {
        assert_eq!(kind, "git");
        assert_eq!(src, "https://example.com/hello.git");
        Ok(name.unwrap_or("hello").to_string())
    }
    fn import_repo_zip(&self, name: &str, zip_path: &std::path::Path) -> anyhow::Result<String> {
        // server 必须先把上传体落盘再调 backend。
        let bytes = std::fs::read(zip_path)?;
        assert_eq!(bytes, b"PK-fake-zip");
        let _ = std::fs::remove_file(zip_path);
        Ok(name.to_string())
    }
    fn update_repo(&self, name: &str) -> anyhow::Result<String> {
        if name == "ziprepo" {
            anyhow::bail!("zip-source repo: upload a new archive via update-zip instead")
        }
        Ok(format!("update scheduled: {name}"))
    }
    fn update_repo_zip(&self, name: &str, zip_path: &std::path::Path) -> anyhow::Result<String> {
        let _ = std::fs::remove_file(zip_path);
        Ok(name.to_string())
    }
    fn remove_repo(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_repo_settings(&self, name: &str, settings: RepoSettingsUpdate) -> anyhow::Result<()> {
        assert_eq!(name, "fixture");
        assert!(settings.embeddings_enabled);
        // server 必须先 clamp 再传给 backend（1_000..=500_000）。
        if let Some(v) = settings.render_max_nodes {
            assert!(
                (1_000..=500_000).contains(&v),
                "render_max_nodes 应已被 server clamp，收到 {v}"
            );
        }
        Ok(())
    }
    fn graph_lod(&self, repo: &str, max_nodes: Option<usize>) -> anyhow::Result<Value> {
        // 回显收到的预算，便于断言 server 侧 clamp / 透传语义。
        Ok(json!({ "repo": repo, "max_nodes": max_nodes, "total_nodes": 42, "returned_nodes": 1 }))
    }
    fn graph_clusters(&self, repo: &str) -> anyhow::Result<Value> {
        Ok(json!({
            "repo": repo,
            "classes": ["Community"],
            "nodes": [
                { "i": 0, "id": "cluster:0", "x": 0.0, "y": 0.0, "s": 3.0, "c": 0, "l": 0, "name": "core" }
            ],
            "edges": [],
            "edge_weights": [],
            "cluster_labels": ["core"],
            "cluster_summaries": [{
                "cluster": 0,
                "label": "core",
                "display_label": "Core",
                "label_basis": ["label:core"],
                "top_symbols": [{
                    "id": "core::run",
                    "name": "run",
                    "label": "Function",
                    "file_path": "src/core.rs",
                    "start_line": 12,
                    "score": 42
                }],
                "top_files": [{
                    "path": "src/core.rs",
                    "nodes": 7,
                    "symbols": 5
                }],
                "quality": {
                    "cohesion": 0.82,
                    "boundary_ratio": 0.18,
                    "internal_edges": 23,
                    "external_edges": 5,
                    "confidence": 0.76,
                    "explanation": "82% internal edges, 18% boundary edges; strongest file signal is src/core.rs."
                }
            }],
            "total_nodes": 42,
            "returned_nodes": 1
        }))
    }
    fn read_source(
        &self,
        repo: &str,
        path: &str,
        start: Option<u32>,
        end: Option<u32>,
    ) -> anyhow::Result<Value> {
        assert_eq!(repo, "fixture");
        if path.contains("..") {
            anyhow::bail!("invalid path (escapes repo root): {path}");
        }
        if path == "gone.rs" {
            anyhow::bail!("file not found in repo: {path}");
        }
        Ok(json!({
            "path": path,
            "abs_path": format!("/tmp/fixture/{path}"),
            "total_lines": 240,
            "start": start.unwrap_or(1),
            "end": end.unwrap_or(240),
            "lines": ["fn main() {", "}"],
            "truncated": false,
        }))
    }
    fn file_symbols(&self, repo: &str, path: &str) -> anyhow::Result<Value> {
        if repo != "fixture" {
            anyhow::bail!("未注册的仓库: {repo}");
        }
        let symbols = if path == "src/empty.ts" {
            json!([])
        } else {
            json!([
                {"id": "fixture:fn:alpha", "name": "alpha", "label": "Function", "file": path, "line": 3, "end_line": 9},
                {"id": "fixture:cls:Beta", "name": "Beta", "label": "Class", "file": path, "line": 50, "end_line": 54},
            ])
        };
        Ok(json!({ "path": path, "symbols": symbols }))
    }
    fn list_files(&self, repo: &str) -> anyhow::Result<Vec<aka_server::ops::FileEntry>> {
        if repo != "fixture" {
            anyhow::bail!("未注册的仓库: {repo}");
        }
        // 故意逆序返回，断言 backend 自身保证升序（这里直接给已排序数据）。
        Ok(vec![
            aka_server::ops::FileEntry {
                path: "src/a.ts".into(),
                symbols: 3,
            },
            aka_server::ops::FileEntry {
                path: "src/b.ts".into(),
                symbols: 1,
            },
        ])
    }
    fn node_detail(&self, repo: &str, id: &str) -> anyhow::Result<Value> {
        Ok(json!({
            "id": id, "name": "main", "label": "Function",
            "file": "src/main.rs", "line": 3, "end_line": 9,
            "properties": {"name": "main"},
            "degree": {"callers": 0, "callees": 1, "refs": 0},
            "repo": repo,
        }))
    }
    fn ego_graph(&self, _: &str, id: &str, depth: u32, max_nodes: usize) -> anyhow::Result<Value> {
        Ok(json!({
            "classes": ["Function"],
            "nodes": [{"i": 0, "id": id, "x": 0.0, "y": 0.0, "s": 1.0, "c": 0, "l": 0, "name": "main"}],
            "edges": [],
            "depth": depth, "max_nodes": max_nodes,
        }))
    }
}

fn managed() -> axum::Router {
    router(Arc::new(ManagedBackend))
}

#[tokio::test]
async fn import_git_is_202_with_name() {
    let res = managed()
        .oneshot(post_json(
            "/api/repos/import",
            json!({ "kind": "git", "url": "https://example.com/hello.git" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let v = body_json(res).await;
    assert_eq!(v["name"], "hello");
}

#[tokio::test]
async fn import_zip_is_202_and_writes_temp_file() {
    let res = managed()
        .oneshot(multipart_req(
            "/api/repos/import-zip",
            "ZB",
            multipart_body("ZB", Some("zipped"), Some(b"PK-fake-zip")),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let v = body_json(res).await;
    assert_eq!(v["name"], "zipped");
}

#[tokio::test]
async fn update_is_202_and_zip_source_is_400() {
    let res = managed()
        .oneshot(
            Request::post("/api/repos/hello/update")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let v = body_json(res).await;
    assert_eq!(v["name"], "hello");

    // zip 来源仓库 → 400 提示走 update-zip。
    let res = managed()
        .oneshot(
            Request::post("/api/repos/ziprepo/update")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let v = body_json(res).await;
    assert!(v["error"].as_str().unwrap().contains("update-zip"));
}

#[tokio::test]
async fn settings_delete_node_ego_shapes() {
    // settings → 200 {"ok":true}（render_max_nodes 缺省 = 恢复默认）
    let res = managed()
        .oneshot(post_json(
            "/api/repos/fixture/settings",
            json!({ "embeddings_enabled": true }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["ok"], true);

    // 显式 null 与缺省等价。
    let res = managed()
        .oneshot(post_json(
            "/api/repos/fixture/settings",
            json!({ "embeddings_enabled": true, "render_max_nodes": null }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // 超出硬上限 / 低于下限的值被 server clamp（ManagedBackend 内有范围断言）。
    for v in [1u32, 999, 50_000, 600_000, u32::MAX] {
        let res = managed()
            .oneshot(post_json(
                "/api/repos/fixture/settings",
                json!({ "embeddings_enabled": true, "render_max_nodes": v }),
            ))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "render_max_nodes={v} 应 clamp 后成功"
        );
        assert_eq!(body_json(res).await["ok"], true);
    }

    // delete → 200 {"ok":true}
    let res = managed()
        .oneshot(
            Request::delete("/api/repos/fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["ok"], true);

    // node 详情合同形状。
    let res = managed()
        .oneshot(
            Request::get("/api/node?repo=fixture&id=fixture:fn:main")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["id"], "fixture:fn:main");
    assert!(v["properties"].is_object());
    assert!(v["degree"]["callers"].is_number());

    // ego 子图：depth/max_nodes 默认值穿透 + LodGraph 同形。
    let res = managed()
        .oneshot(
            Request::get("/api/graph/ego?repo=fixture&id=fixture:fn:main")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["depth"], 2);
    assert_eq!(v["max_nodes"], 2000);
    assert_eq!(v["nodes"][0]["i"], 0);
    assert!(v["classes"].is_array());
    assert!(v["edges"].is_array());
}

#[tokio::test]
async fn lod_max_nodes_clamped_and_default_is_none() {
    // 未显式传 max_nodes → backend 收到 None（由其解析 per-repo 设置）。
    let res = managed()
        .oneshot(
            Request::get("/api/graph/lod?repo=fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert!(v["max_nodes"].is_null(), "缺省必须透传 None 给 backend");

    // 显式传值 → clamp 到 1_000..=500_000 后透传。
    for (q, expect) in [
        (1usize, 1_000u64),
        (999, 1_000),
        (50_000, 50_000),
        (9_999_999, 500_000),
    ] {
        let res = managed()
            .oneshot(
                Request::get(format!("/api/graph/lod?repo=fixture&max_nodes={q}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let v = body_json(res).await;
        assert_eq!(v["max_nodes"], expect, "max_nodes={q} 应 clamp 成 {expect}");
    }
}

#[tokio::test]
async fn graph_clusters_endpoint_returns_cluster_overview() {
    let res = managed()
        .oneshot(
            Request::get("/api/graph/clusters?repo=fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["classes"], json!(["Community"]));
    assert_eq!(v["nodes"][0]["id"], "cluster:0");
    assert_eq!(v["cluster_labels"], json!(["core"]));
    assert_eq!(v["cluster_summaries"][0]["display_label"], "Core");
    assert_eq!(
        v["cluster_summaries"][0]["top_files"][0]["path"],
        "src/core.rs"
    );
    assert_eq!(v["cluster_summaries"][0]["top_symbols"][0]["name"], "run");
    assert_eq!(v["cluster_summaries"][0]["quality"]["internal_edges"], 23);
}

#[tokio::test]
async fn ego_max_nodes_clamped_to_hard_limit() {
    // ego 的上限统一用 MAX_RENDER_NODES（500_000）。
    let res = managed()
        .oneshot(
            Request::get("/api/graph/ego?repo=fixture&id=fixture:fn:main&max_nodes=600000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["max_nodes"], 500_000);
}

#[tokio::test]
async fn source_endpoint_contract_and_errors() {
    // 正常切片 → 200 + 合同形状。
    let res = managed()
        .oneshot(
            Request::get("/api/source?repo=fixture&path=src/x.ts&start=10&end=80")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["path"], "src/x.ts");
    assert_eq!(v["abs_path"], "/tmp/fixture/src/x.ts");
    assert_eq!(v["total_lines"], 240);
    assert_eq!(v["start"], 10);
    assert_eq!(v["end"], 80);
    assert!(v["lines"].is_array());
    assert_eq!(v["truncated"], false);

    // 路径穿越 → 400。
    let res = managed()
        .oneshot(
            Request::get("/api/source?repo=fixture&path=..%2Fsecret.txt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let v = body_json(res).await;
    assert!(v["error"].as_str().unwrap().contains("invalid path"));

    // 文件不存在 → 404。
    let res = managed()
        .oneshot(
            Request::get("/api/source?repo=fixture&path=gone.rs")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // 缺必填参数 → 400。
    let res = managed()
        .oneshot(
            Request::get("/api/source?repo=fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn file_symbols_contract_and_errors() {
    // 正常 → 200 + 合同形状（path 回显 + symbols 按 line 升序）。
    let res = managed()
        .oneshot(
            Request::get("/api/file/symbols?repo=fixture&path=src/x.ts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["path"], "src/x.ts");
    let symbols = v["symbols"].as_array().unwrap();
    assert_eq!(symbols.len(), 2);
    assert_eq!(symbols[0]["id"], "fixture:fn:alpha");
    assert_eq!(symbols[0]["name"], "alpha");
    assert_eq!(symbols[0]["label"], "Function");
    assert_eq!(symbols[0]["file"], "src/x.ts");
    assert_eq!(symbols[0]["line"], 3);
    assert_eq!(symbols[0]["end_line"], 9);
    assert_eq!(symbols[1]["line"], 50);

    // 文件存在但没有符号 → 200 + 空数组。
    let res = managed()
        .oneshot(
            Request::get("/api/file/symbols?repo=fixture&path=src/empty.ts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert!(v["symbols"].as_array().unwrap().is_empty());

    // repo 不在 registry → 404。
    let res = managed()
        .oneshot(
            Request::get("/api/file/symbols?repo=nope&path=src/x.ts")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // 缺必填参数 → 400。
    let res = managed()
        .oneshot(
            Request::get("/api/file/symbols?repo=fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn files_contract_and_errors() {
    // 正常 → 200 + 合同形状（repo 回显 + files 按 path 升序，每项含 symbols 计数）。
    let res = managed()
        .oneshot(
            Request::get("/api/files?repo=fixture")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["repo"], "fixture");
    let files = v["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["path"], "src/a.ts");
    assert_eq!(files[0]["symbols"], 3);
    assert_eq!(files[1]["path"], "src/b.ts");
    assert_eq!(files[1]["symbols"], 1);
    // 升序保证：path 单调递增。
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);

    // repo 不在 registry → 404。
    let res = managed()
        .oneshot(
            Request::get("/api/files?repo=nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // 缺必填参数 → 400。
    let res = managed()
        .oneshot(Request::get("/api/files").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ---- 错误路径：Backend 故障 → 500 + {"error": ...} ----

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
async fn backend_error_becomes_500() {
    let res = router(Arc::new(FailBackend))
        .oneshot(post_json("/api/query", json!({ "query": "x" })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let v = body_json(res).await;
    assert!(v["error"].as_str().unwrap().contains("index corrupted"));
}
