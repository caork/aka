//! aka-server — axum HTTP API（headless / 远程模式）。
//!
//! 数据层与 MCP 面共用 [`aka_mcp::Backend`] trait 和 [`aka_mcp::ops`] 的
//! DTO / 聚合函数，保证 HTTP 与 MCP 两个面的输出形状一致。
//!
//! 路由：
//! - `GET  /api/health`         — 存活探针
//! - `GET  /api/repos`          — 已索引仓库列表
//! - `POST /api/query`          — 混合检索 `{ repo?, query, limit? }`
//! - `POST /api/symbol/context` — 符号 360° 上下文 `{ repo?, symbol }`
//! - `GET  /api/graph/lod`      — 图 LOD 数据 `?repo=&max_nodes=`（Backend 不支持时 501）
//!
//! CORS 仅放行 localhost / 127.0.0.1 / [::1] 来源（任意端口）。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, extract::Query, extract::State};
use serde::Deserialize;
use serde_json::json;
use tower_http::cors::{AllowOrigin, CorsLayer};

use aka_mcp::ops;
pub use aka_mcp::{Backend, MockBackend, RepoInfo, SearchHit, SymbolRef};

type AppState = Arc<dyn Backend>;

/// 绑定 `addr` 并阻塞服务 HTTP API，直到任务被取消。
pub async fn serve(backend: Arc<dyn Backend>, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(backend)).await?;
    Ok(())
}

/// 构建完整路由（含 CORS）。独立导出便于 tower `oneshot` 测试。
pub fn router(backend: Arc<dyn Backend>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/repos", get(repos))
        .route("/api/query", post(query))
        .route("/api/symbol/context", post(symbol_context))
        .route("/api/graph/lod", get(graph_lod))
        .layer(cors_localhost())
        .with_state(backend)
}

// ---- CORS ----

/// 仅放行 localhost 来源（http/https，任意端口）。
fn cors_localhost() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            origin.to_str().is_ok_and(is_localhost_origin)
        }))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE])
}

fn is_localhost_origin(origin: &str) -> bool {
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    // 去掉端口；IPv6 字面量形如 `[::1]:5173`。
    let host = if let Some(v6) = rest.strip_prefix('[') {
        match v6.split_once(']') {
            Some((h, _)) => h,
            None => return false,
        }
    } else {
        rest.split(':').next().unwrap_or("")
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

// ---- 错误：Backend 故障 → 500 + JSON ----

/// HTTP 面的统一错误体：`{"error": "..."}`。
struct ApiError(String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": self.0 })),
        )
            .into_response()
    }
}

/// 把同步 Backend 调用挪到 blocking 线程池（与 MCP 面同一约定）。
async fn run<T, F>(backend: AppState, f: F) -> Result<Json<T>, ApiError>
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce(&dyn Backend) -> anyhow::Result<T> + Send + 'static,
{
    let res = tokio::task::spawn_blocking(move || f(backend.as_ref()))
        .await
        .map_err(|e| ApiError(format!("backend task failed: {e}")))?;
    res.map(Json).map_err(|e| ApiError(format!("{e:#}")))
}

// ---- 请求体 ----

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    /// 仓库名；缺省 = 所有已索引仓库。
    #[serde(default)]
    pub repo: Option<String>,
    pub query: String,
    /// 默认 10，上限 100。
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ContextRequest {
    #[serde(default)]
    pub repo: Option<String>,
    pub symbol: String,
}

// ---- handlers ----

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": "aka-server" }))
}

async fn repos(State(b): State<AppState>) -> Result<Json<ops::ReposOut>, ApiError> {
    run(b, ops::list_repos).await
}

async fn query(
    State(b): State<AppState>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<ops::QueryOut>, ApiError> {
    let limit = req
        .limit
        .unwrap_or(ops::DEFAULT_QUERY_LIMIT)
        .clamp(1, ops::MAX_QUERY_LIMIT);
    run(b, move |b| ops::query(b, req.repo.as_deref(), &req.query, limit)).await
}

async fn symbol_context(
    State(b): State<AppState>,
    Json(req): Json<ContextRequest>,
) -> Result<Json<ops::ContextOut>, ApiError> {
    run(b, move |b| ops::context(b, req.repo.as_deref(), &req.symbol)).await
}

#[derive(Deserialize)]
struct LodParams {
    repo: String,
    #[serde(default = "default_max_nodes")]
    max_nodes: usize,
}

fn default_max_nodes() -> usize {
    50_000
}

/// 图 LOD 数据 — Backend 未接图存储（如 Mock）时保持 501 语义。
async fn graph_lod(State(b): State<AppState>, Query(p): Query<LodParams>) -> Response {
    let res = tokio::task::spawn_blocking(move || b.graph_lod(&p.repo, p.max_nodes)).await;
    match res {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) if e.to_string().contains("not supported") => (
            StatusCode::NOT_IMPLEMENTED,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("{e:#}") })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("backend task failed: {e}") })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::is_localhost_origin;

    #[test]
    fn localhost_origins() {
        assert!(is_localhost_origin("http://localhost:5173"));
        assert!(is_localhost_origin("http://localhost"));
        assert!(is_localhost_origin("https://127.0.0.1:8443"));
        assert!(is_localhost_origin("http://[::1]:3000"));

        assert!(!is_localhost_origin("http://evil.com"));
        assert!(!is_localhost_origin("http://localhost.evil.com"));
        assert!(!is_localhost_origin("https://aka.example"));
        assert!(!is_localhost_origin("ftp://localhost"));
    }
}
