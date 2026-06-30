//! aka desktop shell with an embedded Rust backend.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Once,
};
use std::time::{SystemTime, UNIX_EPOCH};

use aka_cli::AkaBackend;
use aka_core::{
    clamp_index_max_secs, clamp_oss_analyzer_enrichment_max_secs, AkaSettings,
    DEFAULT_OSS_ANALYZER_ENRICHMENT_MAX_SECS,
};
use aka_mcp::{clamp_render_nodes, ops, Backend, RepoSettingsPatch, MAX_RENDER_NODES};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use tauri::{path::BaseDirectory, Manager, State};

type BackendState = Arc<AkaBackend>;

const AKA_HOME_DIR_NAME: &str = "aka-home";
const APP_DATA_DIR_NAME: &str = "com.aka.desktop";
const DESKTOP_MCP_ADDR: &str = "127.0.0.1:4112";
const DESKTOP_MCP_URL: &str = "http://127.0.0.1:4112/mcp";
const DESKTOP_LOG_FILE_NAME: &str = "aka-desktop.log";
const CLIENT_INTEGRATIONS_RESOURCE: &str = "client-integrations/clients";

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientIntegrationInstallRequest {
    client: String,
    #[serde(default)]
    reinstall: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientIntegrationsOut {
    mcp_url: String,
    resource_dir: Option<String>,
    clients: Vec<ClientIntegrationStatus>,
    last_action: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientIntegrationStatus {
    client: String,
    label: String,
    installed: bool,
    available: bool,
    health: String,
    summary: String,
    details: Vec<String>,
    version: Option<String>,
    bundled_version: Option<String>,
    paths: Vec<ClientIntegrationPathStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientIntegrationPathStatus {
    label: String,
    path: String,
    exists: bool,
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
    #[serde(default)]
    embeddings_enabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_patch")]
    description: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_patch")]
    render_max_nodes: Option<Option<u32>>,
}

fn deserialize_optional_patch<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettingsRequest {
    index_max_secs: u64,
    #[serde(default)]
    oss_analyzer_enrichment_enabled: Option<bool>,
    #[serde(default)]
    oss_analyzer_enrichment_max_secs: Option<u64>,
    #[serde(default)]
    scip_index_path: Option<PathBuf>,
    #[serde(default)]
    oss_analyzer_facts_path: Option<PathBuf>,
    #[serde(default)]
    lsp_enrichment_enabled: Option<bool>,
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

fn home_dir() -> anyhow::Result<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }
    anyhow::bail!("home directory is unavailable")
}

fn client_integrations_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    if let Ok(path) = app
        .path()
        .resolve(CLIENT_INTEGRATIONS_RESOURCE, BaseDirectory::Resource)
    {
        if path.exists() {
            return Some(path);
        }
    }

    let repo_clients_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("clients");
    if repo_clients_path.exists() {
        return Some(repo_clients_path);
    }

    let generated_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("client-integrations")
        .join("clients");
    generated_path.exists().then_some(generated_path)
}

fn read_json_file(path: &Path) -> anyhow::Result<serde_json::Value> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(Into::into)
}

fn read_plugin_version(path: &Path) -> Option<String> {
    read_json_file(&path.join(".claude-plugin").join("plugin.json"))
        .ok()
        .and_then(|value| {
            value
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
}

fn path_status(label: &str, path: impl Into<PathBuf>) -> ClientIntegrationPathStatus {
    let path = path.into();
    ClientIntegrationPathStatus {
        label: label.to_string(),
        exists: path.exists(),
        path: path.display().to_string(),
    }
}

fn copy_file_checked(src: &Path, dst: &Path) -> anyhow::Result<()> {
    let Some(parent) = dst.parent() else {
        anyhow::bail!("destination has no parent: {}", dst.display());
    };
    fs::create_dir_all(parent)?;
    fs::copy(src, dst).map(|_| ()).map_err(Into::into)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if !src.is_dir() {
        anyhow::bail!("source directory does not exist: {}", src.display());
    }
    if dst.exists() {
        fs::remove_dir_all(dst)?;
    }
    fs::create_dir_all(dst)?;
    copy_dir_contents(src, dst)
}

fn copy_dir_contents(src: &Path, dst: &Path) -> anyhow::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::create_dir_all(&dst_path)?;
            copy_dir_contents(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            copy_file_checked(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn require_safe_claude_plugin_target(target: &Path) -> anyhow::Result<()> {
    if !target.exists() {
        return Ok(());
    }
    let manifest = target.join(".claude-plugin").join("plugin.json");
    let value = read_json_file(&manifest).map_err(|e| {
        anyhow::anyhow!(
            "目标目录已存在但不像 AKA Claude Code 插件: {} ({e:#})",
            target.display()
        )
    })?;
    let is_aka = value.get("name").and_then(|v| v.as_str()) == Some("aka")
        && value
            .get("repository")
            .and_then(|v| v.as_str())
            .is_some_and(|repo| repo.contains("caork/aka"));
    if !is_aka {
        anyhow::bail!(
            "目标目录已存在但不是 AKA Claude Code 插件: {}",
            target.display()
        );
    }
    Ok(())
}

fn require_safe_aka_skill_target(target: &Path) -> anyhow::Result<()> {
    if !target.exists() {
        return Ok(());
    }
    let skill = target.join("SKILL.md");
    let text = fs::read_to_string(&skill).map_err(|e| {
        anyhow::anyhow!(
            "目标 skill 目录已存在但无法确认来源: {} ({e:#})",
            target.display()
        )
    })?;
    if !text.contains("name: aka-code-graph") || !text.contains("aka 代码知识图谱") {
        anyhow::bail!(
            "目标 skill 目录已存在但不是 AKA aka-code-graph: {}",
            target.display()
        );
    }
    Ok(())
}

fn require_safe_opencode_plugin_target(target: &Path) -> anyhow::Result<()> {
    if !target.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(target).map_err(|e| {
        anyhow::anyhow!(
            "目标 OpenCode plugin 已存在但无法确认来源: {} ({e:#})",
            target.display()
        )
    })?;
    if !text.contains("aka OpenCode plugin") {
        anyhow::bail!(
            "目标 OpenCode plugin 已存在但不是 AKA plugin: {}",
            target.display()
        );
    }
    Ok(())
}

fn claude_code_target_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("skills").join("aka")
}

fn opencode_config_path(home: &Path) -> PathBuf {
    home.join(".config").join("opencode").join("opencode.json")
}

fn opencode_plugin_path(home: &Path) -> PathBuf {
    home.join(".config")
        .join("opencode")
        .join("plugins")
        .join("aka.js")
}

fn opencode_skill_dir(home: &Path) -> PathBuf {
    home.join(".config")
        .join("opencode")
        .join("skills")
        .join("aka-code-graph")
}

fn opencode_mcp_configured(path: &Path) -> (bool, Option<String>) {
    if !path.exists() {
        return (false, None);
    }
    let value = match read_json_file(path) {
        Ok(value) => value,
        Err(e) => return (false, Some(format!("OpenCode config JSON 解析失败: {e:#}"))),
    };
    let Some(aka) = value.get("mcp").and_then(|mcp| mcp.get("aka")) else {
        return (false, None);
    };
    let url_matches = aka
        .get("url")
        .and_then(|v| v.as_str())
        .is_some_and(|url| url == DESKTOP_MCP_URL);
    let enabled = aka
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let remote = aka
        .get("type")
        .and_then(|v| v.as_str())
        .map_or(true, |kind| kind == "remote");
    (url_matches && enabled && remote, None)
}

fn ensure_opencode_mcp_target_is_safe(root: &serde_json::Value, path: &Path) -> anyhow::Result<()> {
    let Some(existing) = root.get("mcp").and_then(|mcp| mcp.get("aka")) else {
        return Ok(());
    };
    let url = existing.get("url").and_then(|v| v.as_str());
    let kind = existing.get("type").and_then(|v| v.as_str());
    if url == Some(DESKTOP_MCP_URL) && kind.map_or(true, |kind| kind == "remote") {
        return Ok(());
    }
    anyhow::bail!(
        "OpenCode config 已有非 AKA mcp.aka，请先手动处理: {}",
        path.display()
    )
}

fn merge_opencode_config(path: &Path) -> anyhow::Result<()> {
    let mut root = if path.exists() {
        read_json_file(path).map_err(|e| {
            anyhow::anyhow!("无法读取 OpenCode config {}: {e:#}", path.display())
        })?
    } else {
        json!({ "$schema": "https://opencode.ai/config.json" })
    };

    if !root.is_object() {
        anyhow::bail!(
            "OpenCode config 必须是 JSON object，当前文件: {}",
            path.display()
        );
    };
    ensure_opencode_mcp_target_is_safe(&root, path)?;
    let Some(root_obj) = root.as_object_mut() else {
        anyhow::bail!("无法创建 OpenCode config object");
    };
    let mcp = root_obj
        .entry("mcp".to_string())
        .or_insert_with(|| json!({}));
    if !mcp.is_object() {
        *mcp = json!({});
    }
    let Some(mcp_obj) = mcp.as_object_mut() else {
        anyhow::bail!("无法创建 OpenCode mcp 配置");
    };
    mcp_obj.insert(
        "aka".to_string(),
        json!({ "type": "remote", "url": DESKTOP_MCP_URL, "enabled": true }),
    );

    let Some(parent) = path.parent() else {
        anyhow::bail!("OpenCode config path has no parent: {}", path.display());
    };
    fs::create_dir_all(parent)?;
    fs::write(path, serde_json::to_string_pretty(&root)? + "\n")?;
    Ok(())
}

fn build_client_integrations_status(
    app: &tauri::AppHandle,
    last_action: Option<String>,
) -> ClientIntegrationsOut {
    let resource_dir = client_integrations_dir(app);
    let resource_dir_display = resource_dir.as_ref().map(|path| path.display().to_string());
    let clients = match home_dir() {
        Ok(home) => vec![
            claude_code_status(resource_dir.as_deref(), &home),
            opencode_status(resource_dir.as_deref(), &home),
        ],
        Err(e) => vec![
            unavailable_client_status(
                "claude-code",
                "Claude Code",
                format!("无法定位用户目录: {e:#}"),
            ),
            unavailable_client_status(
                "opencode",
                "OpenCode",
                format!("无法定位用户目录: {e:#}"),
            ),
        ],
    };

    ClientIntegrationsOut {
        mcp_url: DESKTOP_MCP_URL.to_string(),
        resource_dir: resource_dir_display,
        clients,
        last_action,
    }
}

fn unavailable_client_status(
    client: &str,
    label: &str,
    summary: String,
) -> ClientIntegrationStatus {
    ClientIntegrationStatus {
        client: client.to_string(),
        label: label.to_string(),
        installed: false,
        available: false,
        health: "unavailable".to_string(),
        summary,
        details: vec![],
        version: None,
        bundled_version: None,
        paths: vec![],
    }
}

fn claude_code_status(resource_dir: Option<&Path>, home: &Path) -> ClientIntegrationStatus {
    let source = resource_dir.map(|dir| dir.join("claude-code"));
    let target = claude_code_target_dir(home);
    let manifest = target.join(".claude-plugin").join("plugin.json");
    let mcp = target.join(".mcp.json");
    let skill = target
        .join("skills")
        .join("aka-code-graph")
        .join("SKILL.md");
    let installed = manifest.exists() && mcp.exists() && skill.exists();
    let available = source.as_ref().is_some_and(|path| path.exists());
    let bundled_version = source.as_deref().and_then(read_plugin_version);
    let version = read_plugin_version(&target);
    let mut details = vec![
        "加载名: aka@skills-dir".to_string(),
        format!("MCP endpoint: {DESKTOP_MCP_URL}"),
    ];
    if installed {
        details.push("重启 Claude Code 或运行 /reload-plugins 后生效。".to_string());
    } else {
        details.push("安装到 ~/.claude/skills/aka，作为 Claude Code skills-directory plugin 加载。".to_string());
    }
    if !available {
        details.push("桌面端内置 Claude Code 插件资源缺失。".to_string());
    }

    ClientIntegrationStatus {
        client: "claude-code".to_string(),
        label: "Claude Code".to_string(),
        installed,
        available,
        health: if installed { "ready" } else { "not_installed" }.to_string(),
        summary: if installed {
            "Claude Code 插件已在 personal skills directory 中就绪。".to_string()
        } else {
            "尚未安装 AKA Claude Code 插件。".to_string()
        },
        details,
        version,
        bundled_version,
        paths: vec![
            path_status("Plugin root", target),
            path_status("Manifest", manifest),
            path_status("HTTP MCP config", mcp),
            path_status("Skill", skill),
        ],
    }
}

fn opencode_status(resource_dir: Option<&Path>, home: &Path) -> ClientIntegrationStatus {
    let source = resource_dir.map(|dir| dir.join("opencode"));
    let config = opencode_config_path(home);
    let plugin = opencode_plugin_path(home);
    let skill_dir = opencode_skill_dir(home);
    let skill = skill_dir.join("SKILL.md");
    let (mcp_configured, config_error) = opencode_mcp_configured(&config);
    let plugin_installed = plugin.exists();
    let skill_installed = skill.exists();
    let installed = mcp_configured && plugin_installed && skill_installed;
    let available = source.as_ref().is_some_and(|path| path.exists());
    let mut details = vec![format!("MCP endpoint: {DESKTOP_MCP_URL}")];
    if let Some(config_error) = config_error {
        details.push(config_error);
    }
    if installed {
        details.push("重启 OpenCode 后会加载最新 plugin/skill。".to_string());
    } else {
        details.push("会合并 mcp.aka，并安装 OpenCode plugin 与 aka-code-graph skill。".to_string());
    }
    if !available {
        details.push("桌面端内置 OpenCode 集成资源缺失。".to_string());
    }

    ClientIntegrationStatus {
        client: "opencode".to_string(),
        label: "OpenCode".to_string(),
        installed,
        available,
        health: if installed { "ready" } else { "not_installed" }.to_string(),
        summary: if installed {
            "OpenCode MCP 配置、plugin 与 skill 已就绪。".to_string()
        } else {
            "尚未完整安装 AKA OpenCode 集成。".to_string()
        },
        details,
        version: None,
        bundled_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        paths: vec![
            path_status("Config", config),
            path_status("Plugin", plugin),
            path_status("Skill", skill),
        ],
    }
}

fn install_client_integration_inner(
    app: &tauri::AppHandle,
    request: ClientIntegrationInstallRequest,
) -> anyhow::Result<ClientIntegrationsOut> {
    let resource_dir = client_integrations_dir(app).ok_or_else(|| {
        anyhow::anyhow!("桌面端内置客户端集成资源缺失: {CLIENT_INTEGRATIONS_RESOURCE}")
    })?;
    let home = home_dir()?;
    let client = request.client.trim().to_string();
    let action = match client {
        ref client if client == "claude-code" => {
            install_claude_code_plugin(&resource_dir, &home)?;
            if request.reinstall {
                "已重装 Claude Code 插件"
            } else {
                "已安装/更新 Claude Code 插件"
            }
        }
        ref client if client == "opencode" => {
            install_opencode_integration(&resource_dir, &home)?;
            if request.reinstall {
                "已重装 OpenCode 集成"
            } else {
                "已安装/更新 OpenCode 集成"
            }
        }
        other => anyhow::bail!("unsupported client integration: {other}"),
    };

    Ok(build_client_integrations_status(app, Some(action.to_string())))
}

fn install_claude_code_plugin(resource_dir: &Path, home: &Path) -> anyhow::Result<()> {
    let source = resource_dir.join("claude-code");
    let target = claude_code_target_dir(home);
    require_safe_claude_plugin_target(&target)?;
    copy_dir_recursive(&source, &target)?;
    Ok(())
}

fn install_opencode_integration(resource_dir: &Path, home: &Path) -> anyhow::Result<()> {
    let source = resource_dir.join("opencode");
    require_safe_opencode_plugin_target(&opencode_plugin_path(home))?;
    require_safe_aka_skill_target(&opencode_skill_dir(home))?;
    merge_opencode_config(&opencode_config_path(home))?;
    copy_file_checked(&source.join("plugins").join("aka.js"), &opencode_plugin_path(home))?;
    copy_dir_recursive(
        &source.join("skills").join("aka-code-graph"),
        &opencode_skill_dir(home),
    )?;
    Ok(())
}

#[tauri::command]
async fn client_integrations_status(
    app: tauri::AppHandle,
) -> Result<ClientIntegrationsOut, String> {
    Ok(build_client_integrations_status(&app, None))
}

#[tauri::command]
async fn install_client_integration(
    app: tauri::AppHandle,
    request: ClientIntegrationInstallRequest,
) -> Result<ClientIntegrationsOut, String> {
    tauri::async_runtime::spawn_blocking(move || install_client_integration_inner(&app, request))
        .await
        .map_err(|e| {
            let detail = format!("client integration task failed: {e}");
            log_desktop_event(format!("client integration error: {detail}"));
            detail
        })?
        .map_err(|e| {
            let detail = format!("{e:#}");
            log_desktop_event(format!("client integration error: {detail}"));
            detail
        })
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
    let settings = RepoSettingsPatch {
        embeddings_enabled: settings.embeddings_enabled,
        description: settings.description.map(|description| {
            description.and_then(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
        }),
        render_max_nodes: settings
            .render_max_nodes
            .map(|value| value.map(clamp_render_nodes)),
    };
    run_backend(backend, move |b| {
        b.patch_repo_settings(&name, settings)?;
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
        oss_analyzer_enrichment_enabled: settings
            .oss_analyzer_enrichment_enabled
            .or(settings.lsp_enrichment_enabled)
            .unwrap_or_default(),
        oss_analyzer_enrichment_max_secs: clamp_oss_analyzer_enrichment_max_secs(
            settings
                .oss_analyzer_enrichment_max_secs
                .or(settings.lsp_enrichment_max_secs)
                .unwrap_or(DEFAULT_OSS_ANALYZER_ENRICHMENT_MAX_SECS),
        ),
        scip_index_path: settings.scip_index_path,
        oss_analyzer_facts_path: settings.oss_analyzer_facts_path,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aka-desktop-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn merge_opencode_config_preserves_existing_keys() {
        let dir = temp_test_dir("opencode-merge");
        let path = dir.join("opencode.json");
        fs::write(
            &path,
            r#"{
  "$schema": "https://opencode.ai/config.json",
  "theme": "system",
  "mcp": {
    "other": { "type": "remote", "url": "https://example.test/mcp" }
  }
}
"#,
        )
        .unwrap();

        merge_opencode_config(&path).unwrap();
        merge_opencode_config(&path).unwrap();

        let value = read_json_file(&path).unwrap();
        assert_eq!(value["theme"], "system");
        assert!(value["mcp"]["other"].is_object());
        assert_eq!(value["mcp"]["aka"]["type"], "remote");
        assert_eq!(value["mcp"]["aka"]["url"], DESKTOP_MCP_URL);
        assert_eq!(value["mcp"]["aka"]["enabled"], true);
        assert_eq!(opencode_mcp_configured(&path), (true, None));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn merge_opencode_config_rejects_non_object_root() {
        let dir = temp_test_dir("opencode-invalid");
        let path = dir.join("opencode.json");
        fs::write(&path, "[]").unwrap();

        let err = merge_opencode_config(&path).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn merge_opencode_config_rejects_non_aka_existing_target() {
        let dir = temp_test_dir("opencode-conflict");
        let path = dir.join("opencode.json");
        fs::write(
            &path,
            r#"{
  "mcp": {
    "aka": { "type": "remote", "url": "https://example.test/mcp" }
  }
}
"#,
        )
        .unwrap();

        let err = merge_opencode_config(&path).unwrap_err().to_string();
        assert!(err.contains("已有非 AKA mcp.aka"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn safe_target_checks_reject_unknown_existing_dirs() {
        let dir = temp_test_dir("client-target-safety");
        let claude = dir.join("claude");
        fs::create_dir_all(claude.join(".claude-plugin")).unwrap();
        fs::write(
            claude.join(".claude-plugin").join("plugin.json"),
            r#"{ "name": "aka", "repository": "https://example.test/not-aka" }"#,
        )
        .unwrap();
        let err = require_safe_claude_plugin_target(&claude)
            .unwrap_err()
            .to_string();
        assert!(err.contains("不是 AKA Claude Code 插件"));

        let skill = dir.join("skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: other\n---\n").unwrap();
        let err = require_safe_aka_skill_target(&skill)
            .unwrap_err()
            .to_string();
        assert!(err.contains("不是 AKA aka-code-graph"));

        let plugin = dir.join("aka.js");
        fs::write(&plugin, "export default {};\n").unwrap();
        let err = require_safe_opencode_plugin_target(&plugin)
            .unwrap_err()
            .to_string();
        assert!(err.contains("不是 AKA plugin"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn safe_target_checks_accept_existing_aka_installations() {
        let dir = temp_test_dir("client-target-aka");
        let claude = dir.join("claude");
        fs::create_dir_all(claude.join(".claude-plugin")).unwrap();
        fs::write(
            claude.join(".claude-plugin").join("plugin.json"),
            r#"{ "name": "aka", "repository": "https://github.com/caork/aka" }"#,
        )
        .unwrap();
        require_safe_claude_plugin_target(&claude).unwrap();

        let skill = dir.join("skill");
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: aka-code-graph\n---\naka 代码知识图谱\n",
        )
        .unwrap();
        require_safe_aka_skill_target(&skill).unwrap();

        let plugin = dir.join("aka.js");
        fs::write(&plugin, "console.log('aka OpenCode plugin loaded');\n").unwrap();
        require_safe_opencode_plugin_target(&plugin).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
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
            client_integrations_status,
            install_client_integration,
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
