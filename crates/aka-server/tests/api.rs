//! HTTP API 集成测试：tower `oneshot` 直接驱动 Router，不开真实端口。

use std::sync::Arc;

use aka_server::{Backend, MockBackend, RepoInfo, SearchHit, SymbolRef, router};
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

fn app() -> axum::Router {
    router(Arc::new(MockBackend::demo()))
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
async fn repos_lists_mock_data() {
    let res = app()
        .oneshot(Request::get("/api/repos").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    let repos = v["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 2);
    assert_eq!(repos[0]["name"], "demo");
    assert_eq!(repos[0]["nodes"], 5);
    // 合同字段：status / source{kind,url} / detail。
    assert_eq!(repos[0]["status"], "ready");
    assert_eq!(repos[0]["source"]["kind"], "local");
    assert!(repos[0]["source"]["url"].is_null());
    assert!(repos[0]["detail"].is_null());
    assert_eq!(repos[1]["source"]["kind"], "git");
    assert_eq!(repos[1]["source"]["url"], "https://example.com/beta.git");
}

#[tokio::test]
async fn query_returns_hits() {
    let res = app()
        .oneshot(post_json(
            "/api/query",
            json!({ "repo": "demo", "query": "handle" }),
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
    // MockBackend 不覆写 graph_lod → 默认 "not supported" → 501。
    let res = app()
        .oneshot(
            Request::get("/api/graph/lod?repo=demo")
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
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

// ---- 仓库管理端点：Mock 默认不支持 → 501；参数错误 → 400 ----

fn multipart_body(boundary: &str, name: Option<&str>, file: Option<&[u8]>) -> Vec<u8> {
    let mut body = Vec::new();
    if let Some(n) = name {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{n}\r\n"
            )
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
async fn management_endpoints_are_501_on_mock() {
    // Mock 不覆写仓库管理方法 → 默认 bail "not supported" → 501。
    let cases: Vec<Request<Body>> = vec![
        post_json("/api/repos/import", json!({ "kind": "git", "url": "https://x/y.git" })),
        post_json("/api/repos/demo/settings", json!({ "embeddings_enabled": true })),
        Request::post("/api/repos/demo/update").body(Body::empty()).unwrap(),
        Request::delete("/api/repos/demo").body(Body::empty()).unwrap(),
        Request::get("/api/node?repo=demo&id=demo:fn:main").body(Body::empty()).unwrap(),
        Request::get("/api/graph/ego?repo=demo&id=demo:fn:main").body(Body::empty()).unwrap(),
        multipart_req(
            "/api/repos/import-zip",
            "XB",
            multipart_body("XB", Some("z"), Some(b"PK")),
        ),
        multipart_req(
            "/api/repos/demo/update-zip",
            "XB",
            multipart_body("XB", None, Some(b"PK")),
        ),
    ];
    for req in cases {
        let uri = req.uri().clone();
        let res = app().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED, "{uri} should be 501 on mock");
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
            "/api/repos/demo/update-zip",
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
    fn impact(
        &self,
        _: Option<&str>,
        _: &str,
        _: u32,
        _: usize,
    ) -> anyhow::Result<Vec<SymbolRef>> {
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
    fn set_repo_settings(&self, name: &str, enabled: bool) -> anyhow::Result<()> {
        assert_eq!(name, "demo");
        assert!(enabled);
        Ok(())
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
        .oneshot(Request::post("/api/repos/hello/update").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let v = body_json(res).await;
    assert_eq!(v["name"], "hello");

    // zip 来源仓库 → 400 提示走 update-zip。
    let res = managed()
        .oneshot(Request::post("/api/repos/ziprepo/update").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let v = body_json(res).await;
    assert!(v["error"].as_str().unwrap().contains("update-zip"));
}

#[tokio::test]
async fn settings_delete_node_ego_shapes() {
    // settings → 200 {"ok":true}
    let res = managed()
        .oneshot(post_json(
            "/api/repos/demo/settings",
            json!({ "embeddings_enabled": true }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["ok"], true);

    // delete → 200 {"ok":true}
    let res = managed()
        .oneshot(Request::delete("/api/repos/demo").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["ok"], true);

    // node 详情合同形状。
    let res = managed()
        .oneshot(
            Request::get("/api/node?repo=demo&id=demo:fn:main")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_json(res).await;
    assert_eq!(v["id"], "demo:fn:main");
    assert!(v["properties"].is_object());
    assert!(v["degree"]["callers"].is_number());

    // ego 子图：depth/max_nodes 默认值穿透 + LodGraph 同形。
    let res = managed()
        .oneshot(
            Request::get("/api/graph/ego?repo=demo&id=demo:fn:main")
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
    fn impact(
        &self,
        _: Option<&str>,
        _: &str,
        _: u32,
        _: usize,
    ) -> anyhow::Result<Vec<SymbolRef>> {
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
