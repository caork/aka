//! aka desktop shell with an embedded Rust backend.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
const CLIENT_INTEGRATIONS_DIR_NAME: &str = "client-integrations";
const DESKTOP_MCP_ADDR: &str = "127.0.0.1:4112";
const DESKTOP_MCP_URL: &str = "http://127.0.0.1:4112/mcp";

struct DesktopMcpRuntime {
    _rt: tokio::runtime::Runtime,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientIntegrationSyncOut {
    synced: Vec<ClientIntegrationAction>,
    skipped: Vec<ClientIntegrationAction>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientIntegrationAction {
    client: &'static str,
    detail: String,
}

#[cfg(target_os = "windows")]
const EMBEDDED_CBM_ENGINE: &[u8] = include_bytes!("../embedded-engine/codebase-memory-mcp.exe");

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

#[cfg(not(target_os = "windows"))]
fn bundled_engine_bin_name() -> &'static str {
    "codebase-memory-mcp"
}

#[cfg(not(target_os = "windows"))]
fn has_native_engine(dir: &std::path::Path) -> bool {
    let bin = bundled_engine_bin_name();
    dir.join(bin).is_file()
        || dir.join("bin").join(bin).is_file()
        || dir.join("build/c").join(bin).is_file()
}

#[cfg(not(target_os = "windows"))]
fn bundled_engine_dir(resource_dir: &std::path::Path) -> Option<PathBuf> {
    [
        resource_dir.join("engine"),
        resource_dir.join("resources").join("engine"),
    ]
    .into_iter()
    .find(|dir| has_native_engine(dir))
}

fn bundled_client_integrations_dir(resource_dir: &Path) -> Option<PathBuf> {
    [
        resource_dir.join(CLIENT_INTEGRATIONS_DIR_NAME),
        resource_dir
            .join("resources")
            .join(CLIENT_INTEGRATIONS_DIR_NAME),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."),
    ]
    .into_iter()
    .find(|dir| {
        dir.join("clients").is_dir()
            && dir
                .join(".claude-plugin")
                .join("marketplace.json")
                .is_file()
    })
}

fn desktop_home_dir() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))
}

fn copy_file_if_present(src: &Path, dst: &Path) -> anyhow::Result<bool> {
    if !src.is_file() {
        return Ok(false);
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dst)?;
    Ok(true)
}

fn copy_dir_replace(src: &Path, dst: &Path) -> anyhow::Result<bool> {
    if !src.is_dir() {
        return Ok(false);
    }
    if dst.exists() {
        std::fs::remove_dir_all(dst)?;
    }
    std::fs::create_dir_all(dst)?;
    copy_dir_recursive(src, dst)?;
    Ok(true)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_dir_recursive(&path, &target)?;
        } else if file_type.is_file() {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn resolve_command(name: &str) -> Option<PathBuf> {
    let direct = Path::new(name);
    if direct.components().count() > 1 {
        return executable_file(direct).then(|| direct.to_path_buf());
    }

    let dirs = command_search_dirs();
    command_candidates(name)
        .into_iter()
        .flat_map(|candidate| dirs.iter().map(move |dir| dir.join(&candidate)))
        .find(|path| executable_file(path))
}

fn command_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }

    #[cfg(target_os = "macos")]
    {
        push_command_dir(&mut dirs, "/opt/homebrew/bin");
        push_command_dir(&mut dirs, "/usr/local/bin");
        push_command_dir(&mut dirs, "/usr/bin");
        push_command_dir(&mut dirs, "/bin");
        if let Ok(home) = desktop_home_dir() {
            push_command_dir(&mut dirs, home.join(".local").join("bin"));
            push_command_dir(&mut dirs, home.join(".npm-global").join("bin"));
        }
    }

    dirs
}

fn push_command_dir(dirs: &mut Vec<PathBuf>, dir: impl Into<PathBuf>) {
    let dir = dir.into();
    if !dirs.iter().any(|existing| existing == &dir) {
        dirs.push(dir);
    }
}

fn command_candidates(name: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        if Path::new(name).extension().is_some() {
            return vec![name.to_string()];
        }
        let exts = std::env::var_os("PATHEXT")
            .map(|value| {
                value
                    .to_string_lossy()
                    .split(';')
                    .filter(|ext| !ext.is_empty())
                    .map(|ext| ext.to_ascii_lowercase())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![".exe".into(), ".cmd".into(), ".bat".into()]);
        let mut out = vec![name.to_string()];
        out.extend(exts.into_iter().map(|ext| format!("{name}{ext}")));
        out
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![name.to_string()]
    }
}

fn executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn desktop_cli_command(program: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    if let Ok(path) = std::env::join_paths(command_search_dirs()) {
        command.env("PATH", path);
    }
    command
}

fn claude_plugin_installed(claude: &Path) -> bool {
    let Ok(output) = desktop_cli_command(claude)
        .args(["plugin", "list"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout).contains("aka@aka")
}

fn sync_claude_plugin(
    resources: &Path,
    out: &mut ClientIntegrationSyncOut,
    run_cli: bool,
) -> anyhow::Result<()> {
    let Some(claude) = resolve_command("claude") else {
        out.skipped.push(ClientIntegrationAction {
            client: "claude-code",
            detail: "Claude CLI not found".into(),
        });
        return Ok(());
    };

    let marketplace = resources.join(".claude-plugin").join("marketplace.json");
    if !marketplace.is_file() {
        out.skipped.push(ClientIntegrationAction {
            client: "claude-code",
            detail: "bundled marketplace metadata not found".into(),
        });
        return Ok(());
    }

    if !claude_plugin_installed(&claude) {
        out.skipped.push(ClientIntegrationAction {
            client: "claude-code",
            detail: "aka@aka plugin is not installed".into(),
        });
        return Ok(());
    }

    if !run_cli {
        out.skipped.push(ClientIntegrationAction {
            client: "claude-code",
            detail: "Claude plugin update needs a manual sync from Settings".into(),
        });
        return Ok(());
    }

    let _ = desktop_cli_command(&claude)
        .args(["plugin", "marketplace", "add"])
        .arg(resources)
        .status();
    let status = desktop_cli_command(&claude)
        .args(["plugin", "update", "aka@aka"])
        .status()?;
    if status.success() {
        out.synced.push(ClientIntegrationAction {
            client: "claude-code",
            detail: "updated aka@aka plugin via Claude CLI".into(),
        });
    } else {
        out.skipped.push(ClientIntegrationAction {
            client: "claude-code",
            detail: format!("claude plugin update exited with {status}"),
        });
    }
    Ok(())
}

fn sync_opencode_integration(
    resources: &Path,
    out: &mut ClientIntegrationSyncOut,
    create_missing: bool,
) -> anyhow::Result<()> {
    let home = desktop_home_dir()?;
    let src = resources.join("clients").join("opencode");
    let plugin_src = src.join("plugins").join("aka.js");
    let skill_src = src.join("skills").join("aka-code-graph");
    let plugin_dst = home
        .join(".config")
        .join("opencode")
        .join("plugins")
        .join("aka.js");
    let skill_dst = home
        .join(".config")
        .join("opencode")
        .join("skills")
        .join("aka-code-graph");

    let plugin_installed = plugin_dst.is_file();
    let skill_installed = skill_dst.join("SKILL.md").is_file();
    if !create_missing && !plugin_installed && !skill_installed {
        out.skipped.push(ClientIntegrationAction {
            client: "opencode",
            detail: "OpenCode integration is not installed".into(),
        });
        return Ok(());
    }

    let mut changed = Vec::new();
    if (create_missing || plugin_installed) && copy_file_if_present(&plugin_src, &plugin_dst)? {
        changed.push(format!("plugin -> {}", plugin_dst.display()));
    }
    if (create_missing || skill_installed) && copy_dir_replace(&skill_src, &skill_dst)? {
        changed.push(format!("skill -> {}", skill_dst.display()));
    }

    if changed.is_empty() {
        out.skipped.push(ClientIntegrationAction {
            client: "opencode",
            detail: "bundled OpenCode plugin or skill not found".into(),
        });
    } else {
        out.synced.push(ClientIntegrationAction {
            client: "opencode",
            detail: changed.join("; "),
        });
    }
    Ok(())
}

fn sync_codex_integration(out: &mut ClientIntegrationSyncOut) -> anyhow::Result<()> {
    let cfg = desktop_home_dir()?.join(".codex").join("config.toml");
    if cfg.is_file() {
        let text = std::fs::read_to_string(&cfg).unwrap_or_default();
        if text.contains("[mcp_servers.aka]") {
            if text.contains(DESKTOP_MCP_URL) {
                out.synced.push(ClientIntegrationAction {
                    client: "codex",
                    detail: "existing mcp_servers.aka config uses the desktop MCP server".into(),
                });
            } else {
                out.skipped.push(ClientIntegrationAction {
                    client: "codex",
                    detail: "aka MCP config is not the desktop HTTP URL; no client file changed"
                        .into(),
                });
            }
            return Ok(());
        }
    }
    out.skipped.push(ClientIntegrationAction {
        client: "codex",
        detail: "no aka MCP config found; run clients/install.sh --client codex once".into(),
    });
    Ok(())
}

fn sync_client_integrations_from_resources(
    resources: &Path,
    run_cli: bool,
    create_missing: bool,
) -> anyhow::Result<ClientIntegrationSyncOut> {
    let mut out = ClientIntegrationSyncOut {
        synced: Vec::new(),
        skipped: Vec::new(),
    };
    sync_claude_plugin(resources, &mut out, run_cli)?;
    sync_opencode_integration(resources, &mut out, create_missing)?;
    sync_codex_integration(&mut out)?;
    Ok(out)
}

#[cfg(target_os = "windows")]
fn ensure_embedded_engine_dir(app_data_dir: &std::path::Path) -> anyhow::Result<PathBuf> {
    let engine_dir = app_data_dir.join("engine");
    let engine_bin = engine_dir.join("codebase-memory-mcp.exe");
    std::fs::create_dir_all(&engine_dir)?;

    let needs_write = std::fs::read(&engine_bin)
        .map(|existing| existing != EMBEDDED_CBM_ENGINE)
        .unwrap_or(true);
    if needs_write {
        std::fs::write(&engine_bin, EMBEDDED_CBM_ENGINE)?;
    }

    Ok(engine_dir)
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
struct ClientIntegrationSyncRequest {
    #[serde(default)]
    run_cli: bool,
    #[serde(default)]
    create_missing: bool,
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

    configure_backend(app, &app_data_dir)
}

pub fn configure_cli_runtime() -> anyhow::Result<()> {
    let app_data_dir = fallback_app_data_dir();
    if std::env::var_os("AKA_HOME").is_none() {
        let aka_home = app_data_dir.join(AKA_HOME_DIR_NAME);
        std::fs::create_dir_all(&aka_home)?;
        std::env::set_var("AKA_HOME", &aka_home);
    }
    configure_cli_engine_runtime(&app_data_dir)
}

#[cfg(target_os = "windows")]
fn configure_backend(
    _app: &tauri::App,
    app_data_dir: &std::path::Path,
) -> anyhow::Result<AkaBackend> {
    Ok(AkaBackend::with_engine_dir(ensure_embedded_engine_dir(
        app_data_dir,
    )?))
}

#[cfg(target_os = "windows")]
fn configure_cli_engine_runtime(app_data_dir: &std::path::Path) -> anyhow::Result<()> {
    if std::env::var_os("AKA_ENGINE_DIR").is_none() && std::env::var_os("AKA_CBM_BIN").is_none() {
        let engine_dir = ensure_embedded_engine_dir(app_data_dir)?;
        std::env::set_var("AKA_ENGINE_DIR", engine_dir);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn configure_backend(
    app: &tauri::App,
    _app_data_dir: &std::path::Path,
) -> anyhow::Result<AkaBackend> {
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

#[cfg(not(target_os = "windows"))]
fn configure_cli_engine_runtime(_app_data_dir: &std::path::Path) -> anyhow::Result<()> {
    if std::env::var_os("AKA_ENGINE_DIR").is_none() && std::env::var_os("AKA_CBM_BIN").is_none() {
        let resource_dir = fallback_resource_dir();
        if let Some(engine_dir) = bundled_engine_dir(&resource_dir) {
            std::env::set_var("AKA_ENGINE_DIR", engine_dir);
        }
    }
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
async fn sync_client_integrations(
    app: tauri::AppHandle,
    request: Option<ClientIntegrationSyncRequest>,
) -> Result<ClientIntegrationSyncOut, String> {
    let request = request.unwrap_or(ClientIntegrationSyncRequest {
        run_cli: false,
        create_missing: false,
    });
    let run_cli = request.run_cli;
    let create_missing = request.create_missing;
    tauri::async_runtime::spawn_blocking(move || {
        let resource_dir = app
            .path()
            .resource_dir()
            .unwrap_or_else(|_| fallback_resource_dir());
        let resources = bundled_client_integrations_dir(&resource_dir).ok_or_else(|| {
            anyhow::anyhow!(
                "bundled client integrations not found in {}",
                resource_dir.display()
            )
        })?;
        sync_client_integrations_from_resources(&resources, run_cli, create_missing)
    })
    .await
    .map_err(|e| format!("client integration sync failed: {e}"))?
    .map_err(|e| format!("{e:#}"))
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let backend = configure_desktop_runtime(app).map_err(|e| {
                Box::<dyn std::error::Error>::from(format!("configure desktop runtime: {e:#}"))
            })?;
            backend.start_auto_indexer();
            let backend = Arc::new(backend);
            match start_desktop_mcp_server(Arc::clone(&backend)) {
                Ok(mcp_runtime) => {
                    app.manage(mcp_runtime);
                }
                Err(e) => {
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
            delete_repo,
            clear_app_data,
            app_version,
            sync_client_integrations,
            open_url,
            open_editor_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
