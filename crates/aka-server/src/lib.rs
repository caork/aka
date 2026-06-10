//! aka-server — axum HTTP API（headless / 远程模式）。
//!
//! 数据层与 MCP 面共用 [`aka_mcp::Backend`] trait 和 [`aka_mcp::ops`] 的
//! DTO / 聚合函数，保证 HTTP 与 MCP 两个面的输出形状一致。
//!
//! 路由：
//! - `GET    /api/health`              — 存活探针
//! - `GET    /api/repos`               — 已索引仓库列表（含 status/source/detail）
//! - `POST   /api/query`               — 混合检索 `{ repo?, query, limit? }`
//! - `POST   /api/symbol/context`      — 符号 360° 上下文 `{ repo?, symbol }`
//! - `GET    /api/graph/lod`           — 图 LOD 数据 `?repo=&max_nodes=`（缺省用 per-repo
//!   render_max_nodes 设置，没有则 50_000；一律 clamp 到硬上限；Backend 不支持时 501）
//! - `POST   /api/repos/import`        — 导入 `{kind:"git",url,name?}` 或 `{kind:"local",path}` → 202
//! - `POST   /api/repos/import-zip`    — multipart（name + file）→ 202
//! - `POST   /api/repos/{name}/update` — git pull+analyze / local 重 analyze → 202（zip 来源 400）
//! - `POST   /api/repos/{name}/update-zip` — multipart（file）→ 202
//! - `POST   /api/repos/{name}/settings` — `{embeddings_enabled, render_max_nodes}` → 200
//! - `DELETE /api/repos/{name}`        — 移除注册 + 数据目录 → 200
//! - `GET    /api/node`                — 节点详情 `?repo=&id=`
//! - `GET    /api/graph/ego`           — ego 子图 `?repo=&id=&depth=&max_nodes=`
//! - `GET    /api/source`              — 源码切片 `?repo=&path=&start=&end=`（1-based 含端）
//! - `GET    /api/file/symbols`        — 文件内符号列表 `?repo=&path=`（line 升序）
//!
//! 导入 / 更新都是 202 语义：handler 不等 analyze，任务在 Backend 内部线程执行，
//! 进度经 `GET /api/repos` 的 `status` 字段（ready/indexing/failed）轮询。
//!
//! CORS 仅放行 localhost / 127.0.0.1 / [::1] 来源（任意端口）。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, Query, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tower_http::cors::{AllowOrigin, CorsLayer};

use aka_mcp::ops;
pub use aka_mcp::{
    clamp_render_nodes, Backend, MockBackend, RepoInfo, RepoSettingsUpdate, SearchHit, SymbolRef,
    MAX_RENDER_NODES, MIN_RENDER_NODES,
};

type AppState = Arc<dyn Backend>;

/// zip 上传体上限：200MB。
const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024;

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
        .route("/api/repos/import", post(repos_import))
        .route("/api/repos/import-zip", post(repos_import_zip))
        .route("/api/repos/{name}/update", post(repo_update))
        .route("/api/repos/{name}/update-zip", post(repo_update_zip))
        .route("/api/repos/{name}/settings", post(repo_settings))
        .route("/api/repos/{name}", delete(repo_delete))
        .route("/api/query", post(query))
        .route("/api/symbol/context", post(symbol_context))
        .route("/api/node", get(node_detail))
        .route("/api/graph/lod", get(graph_lod))
        .route("/api/graph/ego", get(graph_ego))
        .route("/api/source", get(source_slice))
        .route("/api/file/symbols", get(file_symbols))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
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
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
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

/// Backend 错误 → HTTP 状态码（按错误文案分类，约定见 Backend trait 文档）：
/// - "not supported"（默认未覆写）→ 501
/// - 未注册 / 找不到 → 404
/// - 业务拒绝（zip 仓库走 update-zip、参数无效、重名）→ 400
/// - 其余 → 500
fn error_response(e: &anyhow::Error) -> Response {
    let msg = format!("{e:#}");
    let status = if msg.contains("not supported") {
        StatusCode::NOT_IMPLEMENTED
    } else if msg.contains("not registered") || msg.contains("未注册") || msg.contains("not found")
    {
        StatusCode::NOT_FOUND
    } else if msg.contains("update-zip") || msg.contains("invalid") || msg.contains("already") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, Json(json!({ "error": msg }))).into_response()
}

/// 管理面（导入/更新/删除/详情）的统一执行：blocking 池 + 错误分类 + 自定义成功码。
async fn run_managed<T, F>(backend: AppState, ok: StatusCode, f: F) -> Response
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce(&dyn Backend) -> anyhow::Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(move || f(backend.as_ref())).await {
        Ok(Ok(v)) => (ok, Json(v)).into_response(),
        Ok(Err(e)) => error_response(&e),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("backend task failed: {e}") })),
        )
            .into_response(),
    }
}

fn bad_request(msg: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg.into() }))).into_response()
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
    /// 缺省 = 用 per-repo render_max_nodes 设置（Backend 内解析，没有则 50_000）。
    #[serde(default)]
    max_nodes: Option<usize>,
}

/// 把显式传入的渲染预算 clamp 到 `MIN_RENDER_NODES..=MAX_RENDER_NODES`（架构红线）。
fn clamp_render_budget(n: usize) -> usize {
    let n32 = u32::try_from(n).unwrap_or(u32::MAX);
    clamp_render_nodes(n32) as usize
}

/// 图 LOD 数据 — Backend 未接图存储（如 Mock）时保持 501 语义。
/// 显式传 max_nodes → clamp 到硬上限后透传；缺省 → None，由 Backend 解析 per-repo 设置。
async fn graph_lod(State(b): State<AppState>, Query(p): Query<LodParams>) -> Response {
    let max_nodes = p.max_nodes.map(clamp_render_budget);
    run_managed(b, StatusCode::OK, move |b| b.graph_lod(&p.repo, max_nodes)).await
}

// ---- 仓库管理 + 节点详情 / ego 子图 ----

#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    /// `git` 或 `local`。
    pub kind: String,
    /// kind = git 时必填。
    #[serde(default)]
    pub url: Option<String>,
    /// kind = local 时必填。
    #[serde(default)]
    pub path: Option<String>,
    /// 可选仓库名（git 缺省取 URL 末段，local 缺省取目录名）。
    #[serde(default)]
    pub name: Option<String>,
}

/// `POST /api/repos/import` → 202 `{"name": ...}`，分析任务在 Backend 后台线程执行。
async fn repos_import(State(b): State<AppState>, Json(req): Json<ImportRequest>) -> Response {
    let src = match req.kind.as_str() {
        "git" => req.url.clone(),
        "local" => req.path.clone(),
        _ => {
            return bad_request(format!(
                "invalid import kind {:?} (expect \"git\" or \"local\")",
                req.kind
            ));
        }
    };
    let Some(src) = src.filter(|s| !s.trim().is_empty()) else {
        return bad_request("invalid import request: git needs \"url\", local needs \"path\"");
    };
    run_managed(b, StatusCode::ACCEPTED, move |b| {
        let name = b.import_repo(&req.kind, &src, req.name.as_deref())?;
        Ok(json!({ "name": name }))
    })
    .await
}

/// multipart 收集结果：name 字段（import 必填）+ zip 落到的临时文件。
async fn collect_multipart(mp: &mut Multipart) -> Result<(Option<String>, Option<PathBuf>), String> {
    let mut name: Option<String> = None;
    let mut tmp: Option<PathBuf> = None;
    loop {
        let field = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return Err(format!("invalid multipart body: {e}")),
        };
        match field.name() {
            Some("name") => match field.text().await {
                Ok(t) => name = Some(t.trim().to_string()),
                Err(e) => return Err(format!("invalid multipart field name: {e}")),
            },
            Some("file") => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| format!("invalid multipart field file: {e}"))?;
                let path = temp_upload_path();
                std::fs::write(&path, &bytes)
                    .map_err(|e| format!("write upload temp file failed: {e}"))?;
                tmp = Some(path);
            }
            _ => {} // 未知字段忽略
        }
    }
    Ok((name, tmp))
}

/// 上传 zip 的唯一临时路径（pid + 单调计数防并发撞名）。
fn temp_upload_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "aka-upload-{}-{}.zip",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// `POST /api/repos/import-zip`（multipart：name + file）→ 202 `{"name": ...}`。
async fn repos_import_zip(State(b): State<AppState>, mut mp: Multipart) -> Response {
    let (name, tmp) = match collect_multipart(&mut mp).await {
        Ok(v) => v,
        Err(e) => return bad_request(e),
    };
    let (Some(name), Some(tmp)) = (name.filter(|n| !n.is_empty()), tmp) else {
        return bad_request("multipart must contain fields \"name\" and \"file\"");
    };
    let cleanup = tmp.clone();
    let res = run_managed(b, StatusCode::ACCEPTED, move |b| {
        let name = b.import_repo_zip(&name, &tmp)?;
        Ok(json!({ "name": name }))
    })
    .await;
    if res.status() != StatusCode::ACCEPTED {
        let _ = std::fs::remove_file(&cleanup); // 后台任务没接手 → server 清掉临时件
    }
    res
}

/// `POST /api/repos/{name}/update-zip`（multipart：file）→ 202。
async fn repo_update_zip(
    State(b): State<AppState>,
    AxumPath(name): AxumPath<String>,
    mut mp: Multipart,
) -> Response {
    let (_, tmp) = match collect_multipart(&mut mp).await {
        Ok(v) => v,
        Err(e) => return bad_request(e),
    };
    let Some(tmp) = tmp else {
        return bad_request("multipart must contain field \"file\"");
    };
    let cleanup = tmp.clone();
    let res = run_managed(b, StatusCode::ACCEPTED, move |b| {
        let name = b.update_repo_zip(&name, &tmp)?;
        Ok(json!({ "name": name }))
    })
    .await;
    if res.status() != StatusCode::ACCEPTED {
        let _ = std::fs::remove_file(&cleanup);
    }
    res
}

/// `POST /api/repos/{name}/update` → 202（git: pull+analyze；local: 重 analyze；zip 来源 400）。
async fn repo_update(State(b): State<AppState>, AxumPath(name): AxumPath<String>) -> Response {
    run_managed(b, StatusCode::ACCEPTED, move |b| {
        let summary = b.update_repo(&name)?;
        Ok(json!({ "name": name, "detail": summary }))
    })
    .await
}

#[derive(Debug, Deserialize)]
pub struct SettingsRequest {
    pub embeddings_enabled: bool,
    /// 缺省 / null = 恢复默认渲染预算（50_000）。
    #[serde(default)]
    pub render_max_nodes: Option<u32>,
}

/// `POST /api/repos/{name}/settings` → 200 `{"ok":true}`。
/// render_max_nodes 写入前 clamp 到 `MIN_RENDER_NODES..=MAX_RENDER_NODES`。
async fn repo_settings(
    State(b): State<AppState>,
    AxumPath(name): AxumPath<String>,
    Json(req): Json<SettingsRequest>,
) -> Response {
    let settings = RepoSettingsUpdate {
        embeddings_enabled: req.embeddings_enabled,
        render_max_nodes: req.render_max_nodes.map(clamp_render_nodes),
    };
    run_managed(b, StatusCode::OK, move |b| {
        b.set_repo_settings(&name, settings)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

/// `DELETE /api/repos/{name}` → 200 `{"ok":true}`。
async fn repo_delete(State(b): State<AppState>, AxumPath(name): AxumPath<String>) -> Response {
    run_managed(b, StatusCode::OK, move |b| {
        b.remove_repo(&name)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[derive(Debug, Deserialize)]
struct NodeParams {
    repo: String,
    id: String,
}

/// `GET /api/node?repo=&id=` — 节点详情。
async fn node_detail(State(b): State<AppState>, Query(p): Query<NodeParams>) -> Response {
    run_managed(b, StatusCode::OK, move |b| b.node_detail(&p.repo, &p.id)).await
}

#[derive(Debug, Deserialize)]
struct EgoParams {
    repo: String,
    id: String,
    #[serde(default = "default_ego_depth")]
    depth: u32,
    #[serde(default = "default_ego_max_nodes")]
    max_nodes: usize,
}

fn default_ego_depth() -> u32 {
    2
}

fn default_ego_max_nodes() -> usize {
    2000
}

/// `GET /api/graph/ego?repo=&id=&depth=&max_nodes=` — 与 /api/graph/lod 同形的 ego 子图。
async fn graph_ego(State(b): State<AppState>, Query(p): Query<EgoParams>) -> Response {
    let depth = p.depth.min(8);
    let max_nodes = p.max_nodes.clamp(1, MAX_RENDER_NODES as usize);
    run_managed(b, StatusCode::OK, move |b| {
        b.ego_graph(&p.repo, &p.id, depth, max_nodes)
    })
    .await
}

#[derive(Debug, Deserialize)]
struct SourceParams {
    repo: String,
    /// repo 内相对路径。
    path: String,
    /// 1-based 起始行（含）；缺省 = 1。
    #[serde(default)]
    start: Option<u32>,
    /// 1-based 结束行（含）；缺省 = 文件末行（单次最多 2000 行）。
    #[serde(default)]
    end: Option<u32>,
}

/// `GET /api/source?repo=&path=&start=&end=` — 源码切片（详情面板用）。
/// repo / 文件不存在 → 404；路径穿越 / 二进制文件 → 400；Mock 不支持 → 501。
async fn source_slice(State(b): State<AppState>, Query(p): Query<SourceParams>) -> Response {
    run_managed(b, StatusCode::OK, move |b| {
        b.read_source(&p.repo, &p.path, p.start, p.end)
    })
    .await
}

#[derive(Debug, Deserialize)]
struct FileSymbolsParams {
    repo: String,
    /// repo 内相对路径（与 nodes 表 file_path 精确匹配）。
    path: String,
}

/// `GET /api/file/symbols?repo=&path=` — 文件内符号列表（源码预览高亮用）。
/// repo 未注册 → 404；文件没有符号 → 200 空数组；Mock 不支持 → 501。
async fn file_symbols(State(b): State<AppState>, Query(p): Query<FileSymbolsParams>) -> Response {
    run_managed(b, StatusCode::OK, move |b| b.file_symbols(&p.repo, &p.path)).await
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
