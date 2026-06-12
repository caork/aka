//! aka desktop shell with an embedded Rust backend.

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use aka_cli::AkaBackend;
use aka_mcp::{clamp_render_nodes, ops, Backend, RepoSettingsUpdate, MAX_RENDER_NODES};
use serde::Deserialize;
use serde_json::json;
use tauri::{Manager, State};

type BackendState = Arc<AkaBackend>;

const AKA_HOME_DIR_NAME: &str = "aka-home";
const APP_DATA_DIR_NAME: &str = "com.aka.desktop";

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

fn fallback_resource_dir() -> PathBuf {
    let Ok(exe) = std::env::current_exe() else {
        return std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir());
    };
    if let Some(contents) = exe.parent().and_then(|macos| macos.parent()) {
        let resources = contents.join("Resources");
        if resources.exists() {
            return resources;
        }
    }
    if let Some(parent) = exe.parent() {
        let resources = parent.join("resources");
        if resources.exists() {
            return resources;
        }
        return parent.to_path_buf();
    }
    std::env::temp_dir()
}

fn bundled_engine_bin_name() -> &'static str {
    if cfg!(windows) {
        "codebase-memory-mcp.exe"
    } else {
        "codebase-memory-mcp"
    }
}

fn has_native_engine(dir: &std::path::Path) -> bool {
    let bin = bundled_engine_bin_name();
    dir.join(bin).is_file()
        || dir.join("bin").join(bin).is_file()
        || dir.join("build/c").join(bin).is_file()
}

fn bundled_engine_dir(resource_dir: &std::path::Path) -> Option<PathBuf> {
    [
        resource_dir.join("engine"),
        resource_dir.join("resources").join("engine"),
    ]
    .into_iter()
    .find(|dir| has_native_engine(dir))
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

async fn run_backend<T, F>(backend: State<'_, BackendState>, f: F) -> Result<T, String>
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce(BackendState) -> anyhow::Result<T> + Send + 'static,
{
    let backend = Arc::clone(&backend);
    tauri::async_runtime::spawn_blocking(move || f(backend))
        .await
        .map_err(|e| format!("backend task failed: {e}"))?
        .map_err(|e| format!("{e:#}"))
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

    let resource_dir = app
        .path()
        .resource_dir()
        .unwrap_or_else(|_| fallback_resource_dir());
    Ok(
        if let Some(engine_dir) = bundled_engine_dir(&resource_dir) {
            AkaBackend::with_engine_dir(engine_dir)
        } else {
            AkaBackend::new()
        },
    )
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let backend = configure_desktop_runtime(app).map_err(|e| {
                Box::<dyn std::error::Error>::from(format!("configure desktop runtime: {e:#}"))
            })?;
            app.manage(Arc::new(backend));
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
            delete_repo,
            clear_app_data,
            app_version,
            open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
