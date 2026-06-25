//! aka desktop shell with an embedded Rust backend.

use std::fs::OpenOptions;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Once,
};
use std::time::{SystemTime, UNIX_EPOCH};

use aka_cli::AkaBackend;
use aka_core::{
    clamp_index_max_secs, clamp_lsp_enrichment_max_secs, AkaSettings,
    DEFAULT_LSP_ENRICHMENT_MAX_SECS,
};
use aka_mcp::{clamp_render_nodes, ops, Backend, RepoSettingsUpdate, MAX_RENDER_NODES};
use serde::Deserialize;
use serde_json::json;
use tauri::{Manager, State};

type BackendState = Arc<AkaBackend>;

const AKA_HOME_DIR_NAME: &str = "aka-home";
const APP_DATA_DIR_NAME: &str = "com.aka.desktop";
const DESKTOP_MCP_ADDR: &str = "127.0.0.1:4112";
const DESKTOP_LOG_FILE_NAME: &str = "aka-desktop.log";

#[cfg(all(target_os = "windows", feature = "embedded-engine"))]
const EMBEDDED_AKA_ENGINE_DLL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/engine/aka_engine.dll"
));

#[cfg(target_os = "windows")]
const EMBEDDED_AKA_ENGINE_SHA: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/engine/ENGINE_SHA"
));

struct DesktopMcpRuntime {
    _rt: tokio::runtime::Runtime,
}

fn fallback_app_data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(APP_DATA_DIR_NAME);
        }
    }
    if cfg!(target_os = "windows") {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join(APP_DATA_DIR_NAME);
        }
    }
    if let Some(xdg_data_home) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg_data_home).join(APP_DATA_DIR_NAME);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join(APP_DATA_DIR_NAME);
    }
    std::env::temp_dir().join(APP_DATA_DIR_NAME)
}

pub fn install_desktop_diagnostics() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        log_desktop_event(format!(
            "startup version={} os={} arch={} exe={}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|e| format!("<unavailable: {e}>"))
        ));

        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log_desktop_event(format!(
                "panic: {info}\nbacktrace:\n{}",
                std::backtrace::Backtrace::force_capture()
            ));
            default_hook(info);
        }));
    });
}

fn log_desktop_event(message: impl AsRef<str>) {
    let log_dir = fallback_app_data_dir().join("logs");
    if std::fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    let log_path = log_dir.join(DESKTOP_LOG_FILE_NAME);
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) else {
        return;
    };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let _ = writeln!(file, "[{ts}] {}", message.as_ref());
}

#[cfg(all(target_os = "windows", feature = "embedded-engine"))]
fn materialize_embedded_engine(app_data_dir: &std::path::Path) -> anyhow::Result<PathBuf> {
    let engine_sha = std::str::from_utf8(EMBEDDED_AKA_ENGINE_SHA)
        .unwrap_or("unknown")
        .trim();
    let engine_dir = app_data_dir
        .join("bundled-engine")
        .join(format!("{}-{engine_sha}", env!("CARGO_PKG_VERSION")));
    materialize_embedded_resource(
        &engine_dir,
        "ENGINE_SHA",
        EMBEDDED_AKA_ENGINE_SHA,
        "desktop embedded engine sha",
    )?;
    let engine_dll = materialize_embedded_resource(
        &engine_dir,
        "aka_engine.dll",
        EMBEDDED_AKA_ENGINE_DLL,
        "desktop embedded engine dll",
    )?;
    std::env::set_var("AKA_ENGINE_DLL", &engine_dll);
    std::env::set_var("AKA_ENGINE_LIB_DIR", &engine_dir);
    log_desktop_event(format!(
        "desktop engine dll={} source=embedded-dll",
        engine_dll.display()
    ));
    Ok(engine_dir)
}

#[cfg(all(target_os = "windows", feature = "embedded-engine"))]
fn materialize_embedded_resource(
    engine_dir: &std::path::Path,
    file_name: &str,
    bytes: &[u8],
    label: &str,
) -> anyhow::Result<PathBuf> {
    let path = engine_dir.join(file_name);
    let needs_write = match std::fs::read(&path) {
        Ok(existing) => existing != bytes,
        Err(_) => true,
    };

    if needs_write {
        std::fs::create_dir_all(engine_dir)?;
        let tmp = engine_dir.join(format!("{file_name}.tmp-{}", std::process::id()));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path).or_else(|rename_err| {
            let _ = std::fs::remove_file(&path);
            std::fs::rename(&tmp, &path).map_err(|_| rename_err)
        })?;
        log_desktop_event(format!(
            "{label} materialized path={} bytes={}",
            path.display(),
            bytes.len()
        ));
    } else {
        log_desktop_event(format!("{label} already present path={}", path.display()));
    }

    Ok(path)
}

#[derive(Debug, Deserialize)]
struct ImportRequest {
    kind: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ZipImportRequest {
    name: String,
    path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoSettingsRequest {
    embeddings_enabled: bool,
    #[serde(default)]
    render_max_nodes: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettingsRequest {
    index_max_secs: u64,
    #[serde(default)]
    lsp_enrichment_enabled: bool,
    #[serde(default)]
    lsp_enrichment_max_secs: Option<u64>,
}

async fn run_backend<T, F>(backend: State<'_, BackendState>, f: F) -> Result<T, String>
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce(BackendState) -> anyhow::Result<T> + Send + 'static,
{
    let backend = Arc::clone(&backend);
    tauri::async_runtime::spawn_blocking(move || f(backend))
        .await
        .map_err(|e| {
            let detail = format!("backend task failed: {e}");
            log_desktop_event(format!("backend error: {detail}"));
            detail
        })?
        .map_err(|e| {
            let detail = format!("{e:#}");
            log_desktop_event(format!("backend error: {detail}"));
            detail
        })
}

fn copy_zip_to_temp(path: &str) -> anyhow::Result<std::path::PathBuf> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let src = std::path::Path::new(path);
    let tmp = std::env::temp_dir().join(format!(
        "aka-desktop-zip-{}-{}.zip",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::copy(src, &tmp).map_err(|e| {
        anyhow::anyhow!(
            "copy zip to temp failed ({} -> {}): {e}",
            src.display(),
            tmp.display()
        )
    })?;
    Ok(tmp)
}

fn configure_desktop_runtime(app: &tauri::App) -> anyhow::Result<AkaBackend> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| fallback_app_data_dir());
    let aka_home = app_data_dir.join(AKA_HOME_DIR_NAME);
    std::fs::create_dir_all(&aka_home)?;
    std::env::set_var("AKA_HOME", &aka_home);

    configure_backend(app, &app_data_dir)
}

pub fn configure_cli_runtime() -> anyhow::Result<()> {
    install_desktop_diagnostics();
    let app_data_dir = fallback_app_data_dir();
    if std::env::var_os("AKA_HOME").is_none() {
        let aka_home = app_data_dir.join(AKA_HOME_DIR_NAME);
        std::fs::create_dir_all(&aka_home)?;
        std::env::set_var("AKA_HOME", &aka_home);
    }
    configure_cli_engine_runtime(&app_data_dir)
}

#[cfg(all(target_os = "windows", feature = "embedded-engine"))]
fn configure_backend(
    _app: &tauri::App,
    app_data_dir: &std::path::Path,
) -> anyhow::Result<AkaBackend> {
    let engine_dir = materialize_embedded_engine(app_data_dir).map_err(|err| {
        anyhow::anyhow!("desktop embedded engine DLL materialization failed: {err:#}")
    })?;
    log_desktop_event(format!(
        "desktop engine dir={} source=embedded-resource",
        engine_dir.display()
    ));
    let backend = AkaBackend::new();
    Ok(backend
        .with_job_event_sink(|message| log_desktop_event(format!("backend job: {message}")))
        .with_workspace_auto_index())
}

#[cfg(all(target_os = "windows", not(feature = "embedded-engine")))]
fn configure_backend(
    _app: &tauri::App,
    _app_data_dir: &std::path::Path,
) -> anyhow::Result<AkaBackend> {
    Err(anyhow::anyhow!(
        "Windows desktop runtime requires the embedded-engine feature and aka_engine.dll resource"
    ))
}

#[cfg(all(target_os = "windows", feature = "embedded-engine"))]
fn configure_cli_engine_runtime(app_data_dir: &std::path::Path) -> anyhow::Result<()> {
    if std::env::var_os("AKA_ENGINE_DLL").is_none() {
        materialize_embedded_engine(app_data_dir)?;
    }
    Ok(())
}

#[cfg(all(target_os = "windows", not(feature = "embedded-engine")))]
fn configure_cli_engine_runtime(_app_data_dir: &std::path::Path) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(
        "Windows runtime requires the embedded-engine feature and aka_engine.dll resource"
    ))
}

#[cfg(not(target_os = "windows"))]
fn configure_backend(
    _app: &tauri::App,
    _app_data_dir: &std::path::Path,
) -> anyhow::Result<AkaBackend> {
    let backend = AkaBackend::new();
    Ok(backend
        .with_job_event_sink(|message| log_desktop_event(format!("backend job: {message}")))
        .with_workspace_auto_index())
}

#[cfg(not(target_os = "windows"))]
fn configure_cli_engine_runtime(_app_data_dir: &std::path::Path) -> anyhow::Result<()> {
    Ok(())
}

fn start_desktop_mcp_server(backend: BackendState) -> anyhow::Result<DesktopMcpRuntime> {
    let addr: SocketAddr = DESKTOP_MCP_ADDR.parse()?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .thread_name("aka-desktop-mcp")
        .enable_all()
        .build()?;
    let backend: Arc<dyn Backend> = backend;
    rt.spawn(async move {
        if let Err(e) = aka_mcp::serve_http(backend, addr).await {
            log_desktop_event(format!("desktop MCP server stopped: {e:#}"));
            eprintln!("aka desktop MCP server stopped: {e:#}");
        }
    });
    Ok(DesktopMcpRuntime { _rt: rt })
}

#[tauri::command]
async fn list_repos(backend: State<'_, BackendState>) -> Result<ops::ReposOut, String> {
    run_backend(backend, |b| ops::list_repos(b.as_ref())).await
}

#[tauri::command]
async fn query(
    backend: State<'_, BackendState>,
    repo: Option<String>,
    query: String,
    limit: Option<usize>,
) -> Result<ops::QueryOut, String> {
    let limit = limit
        .unwrap_or(ops::DEFAULT_QUERY_LIMIT)
        .clamp(1, ops::MAX_QUERY_LIMIT);
    run_backend(backend, move |b| {
        ops::query(
            b.as_ref(),
            ops::QueryOptions {
                repo: repo.as_deref(),
                query: &query,
                limit,
                max_symbols: ops::DEFAULT_QUERY_PROCESS_SYMBOL_LIMIT,
                include_content: false,
                task_context: None,
                goal: None,
            },
        )
    })
    .await
}

#[tauri::command]
async fn symbol_context(
    backend: State<'_, BackendState>,
    repo: Option<String>,
    symbol: String,
) -> Result<ops::ContextOut, String> {
    run_backend(backend, move |b| {
        ops::context(b.as_ref(), repo.as_deref(), &symbol)
    })
    .await
}

#[tauri::command]
async fn graph_lod(
    backend: State<'_, BackendState>,
    repo: String,
    max_nodes: Option<usize>,
) -> Result<serde_json::Value, String> {
    let max_nodes = max_nodes.map(|n| {
        let n32 = u32::try_from(n).unwrap_or(u32::MAX);
        clamp_render_nodes(n32) as usize
    });
    run_backend(backend, move |b| b.graph_lod(&repo, max_nodes)).await
}

#[tauri::command]
async fn graph_clusters(
    backend: State<'_, BackendState>,
    repo: String,
) -> Result<serde_json::Value, String> {
    run_backend(backend, move |b| b.graph_clusters(&repo)).await
}

#[tauri::command]
async fn graph_ego(
    backend: State<'_, BackendState>,
    repo: String,
    id: String,
    depth: Option<u32>,
    max_nodes: Option<usize>,
) -> Result<serde_json::Value, String> {
    let depth = depth.unwrap_or(2).min(8);
    let max_nodes = max_nodes
        .unwrap_or(2000)
        .clamp(1, MAX_RENDER_NODES as usize);
    run_backend(backend, move |b| b.ego_graph(&repo, &id, depth, max_nodes)).await
}

#[tauri::command]
async fn node_detail(
    backend: State<'_, BackendState>,
    repo: String,
    id: String,
) -> Result<serde_json::Value, String> {
    run_backend(backend, move |b| b.node_detail(&repo, &id)).await
}

#[tauri::command]
async fn source(
    backend: State<'_, BackendState>,
    repo: String,
    path: String,
    start: Option<u32>,
    end: Option<u32>,
) -> Result<serde_json::Value, String> {
    run_backend(backend, move |b| b.read_source(&repo, &path, start, end)).await
}

#[tauri::command]
async fn repo_files(
    backend: State<'_, BackendState>,
    repo: String,
) -> Result<ops::FilesOut, String> {
    run_backend(backend, move |b| ops::list_files(b.as_ref(), &repo)).await
}

#[tauri::command]
async fn file_symbols(
    backend: State<'_, BackendState>,
    repo: String,
    path: String,
) -> Result<serde_json::Value, String> {
    run_backend(backend, move |b| b.file_symbols(&repo, &path)).await
}

#[tauri::command]
async fn import_repo(
    backend: State<'_, BackendState>,
    request: ImportRequest,
) -> Result<serde_json::Value, String> {
    let src = match request.kind.as_str() {
        "git" => request.url.clone(),
        "local" => request.path.clone(),
        other => {
            return Err(format!(
                "invalid import kind {other:?} (expect \"git\" or \"local\")"
            ))
        }
    };
    let Some(src) = src.filter(|s| !s.trim().is_empty()) else {
        return Err("invalid import request: git needs \"url\", local needs \"path\"".into());
    };
    run_backend(backend, move |b| {
        let name = b.import_repo(&request.kind, &src, request.name.as_deref())?;
        Ok(json!({ "name": name }))
    })
    .await
}

#[tauri::command]
async fn import_repo_zip(
    backend: State<'_, BackendState>,
    request: ZipImportRequest,
) -> Result<serde_json::Value, String> {
    let name = request.name.trim().to_string();
    if name.is_empty() {
        return Err("invalid zip import request: name is required".into());
    }
    run_backend(backend, move |b| {
        let zip = copy_zip_to_temp(&request.path)?;
        let cleanup = zip.clone();
        let name = match b.import_repo_zip(&name, &zip) {
            Ok(name) => name,
            Err(e) => {
                let _ = std::fs::remove_file(&cleanup);
                return Err(e);
            }
        };
        Ok(json!({ "name": name }))
    })
    .await
}

#[tauri::command]
async fn update_repo(
    backend: State<'_, BackendState>,
    name: String,
) -> Result<serde_json::Value, String> {
    run_backend(backend, move |b| {
        let detail = b.update_repo(&name)?;
        Ok(json!({ "name": name, "detail": detail }))
    })
    .await
}

#[tauri::command]
async fn update_repo_zip(
    backend: State<'_, BackendState>,
    name: String,
    path: String,
) -> Result<serde_json::Value, String> {
    run_backend(backend, move |b| {
        let zip = copy_zip_to_temp(&path)?;
        let cleanup = zip.clone();
        let name = match b.update_repo_zip(&name, &zip) {
            Ok(name) => name,
            Err(e) => {
                let _ = std::fs::remove_file(&cleanup);
                return Err(e);
            }
        };
        Ok(json!({ "name": name }))
    })
    .await
}

#[tauri::command]
async fn set_repo_settings(
    backend: State<'_, BackendState>,
    name: String,
    settings: RepoSettingsRequest,
) -> Result<serde_json::Value, String> {
    let settings = RepoSettingsUpdate {
        embeddings_enabled: settings.embeddings_enabled,
        render_max_nodes: settings.render_max_nodes.map(clamp_render_nodes),
    };
    run_backend(backend, move |b| {
        b.set_repo_settings(&name, settings)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[tauri::command]
async fn get_app_settings() -> Result<AkaSettings, String> {
    AkaSettings::load().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn set_app_settings(settings: AppSettingsRequest) -> Result<AkaSettings, String> {
    AkaSettings {
        index_max_secs: clamp_index_max_secs(settings.index_max_secs),
        lsp_enrichment_enabled: settings.lsp_enrichment_enabled,
        lsp_enrichment_max_secs: clamp_lsp_enrichment_max_secs(
            settings
                .lsp_enrichment_max_secs
                .unwrap_or(DEFAULT_LSP_ENRICHMENT_MAX_SECS),
        ),
    }
    .save()
    .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn delete_repo(
    backend: State<'_, BackendState>,
    name: String,
) -> Result<serde_json::Value, String> {
    run_backend(backend, move |b| {
        b.remove_repo(&name)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[tauri::command]
async fn clear_app_data(backend: State<'_, BackendState>) -> Result<serde_json::Value, String> {
    run_backend(backend, move |b| {
        b.clear_runtime_data()?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[tauri::command]
async fn app_version() -> Result<String, String> {
    Ok(env!("CARGO_PKG_VERSION").to_string())
}

#[tauri::command]
async fn open_url(url: String) -> Result<serde_json::Value, String> {
    let url = validate_external_url(&url)?.to_string();
    spawn_url_opener(&url).map_err(|e| format!("open url failed: {e}"))?;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
async fn open_editor_url(url: String) -> Result<serde_json::Value, String> {
    let url = validate_editor_url(&url)?.to_string();
    spawn_url_opener(&url).map_err(|e| format!("open editor failed: {e}"))?;
    Ok(json!({ "ok": true }))
}

fn validate_external_url(url: &str) -> Result<&str, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("url is required".into());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("url contains invalid control characters".into());
    }
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("only http(s) urls can be opened".into());
    }
    Ok(trimmed)
}

fn validate_editor_url(url: &str) -> Result<&str, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("editor url is required".into());
    }
    if trimmed.chars().any(char::is_control) {
        return Err("editor url contains invalid control characters".into());
    }
    if !trimmed.starts_with("vscode://file/") {
        return Err("only vscode://file/ urls can be opened".into());
    }
    Ok(trimmed)
}

fn spawn_url_opener(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn()?;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).spawn()?;
        return Ok(());
    }

    #[allow(unreachable_code)]
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "opening urls is unsupported on this platform",
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_desktop_diagnostics();
    let native_updater_enabled = matches!(
        option_env!("AKA_ENABLE_NATIVE_UPDATER"),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    );
    log_desktop_event(format!(
        "tauri start native_updater_enabled={native_updater_enabled}"
    ));

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init());

    if native_updater_enabled {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    let result = builder
        .setup(|app| {
            let backend = configure_desktop_runtime(app).map_err(|e| {
                Box::<dyn std::error::Error>::from(format!("configure desktop runtime: {e:#}"))
            })?;
            log_desktop_event("desktop runtime configured");
            backend.start_auto_indexer();
            let backend = Arc::new(backend);
            match start_desktop_mcp_server(Arc::clone(&backend)) {
                Ok(mcp_runtime) => {
                    log_desktop_event("desktop MCP server started");
                    app.manage(mcp_runtime);
                }
                Err(e) => {
                    log_desktop_event(format!("desktop MCP server unavailable: {e:#}"));
                    eprintln!("aka desktop MCP server unavailable: {e:#}");
                }
            }
            app.manage(backend);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_repos,
            query,
            symbol_context,
            graph_lod,
            graph_clusters,
            graph_ego,
            node_detail,
            source,
            repo_files,
            file_symbols,
            import_repo,
            import_repo_zip,
            update_repo,
            update_repo_zip,
            set_repo_settings,
            get_app_settings,
            set_app_settings,
            delete_repo,
            clear_app_data,
            app_version,
            open_url,
            open_editor_url,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = result {
        log_desktop_event(format!(
            "fatal: error while running tauri application: {e:#}"
        ));
        panic!("error while running tauri application: {e:#}");
    }
}
