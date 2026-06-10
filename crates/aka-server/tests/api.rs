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
