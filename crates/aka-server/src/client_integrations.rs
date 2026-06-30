//! Client integration management shared by HTTP preview and the desktop shell.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

pub const DESKTOP_MCP_URL: &str = "http://127.0.0.1:4112/mcp";
const OPENCODE_PLUGIN_VERSION_PREFIX: &str = "// AKA client integration version = ";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIntegrationInstallRequest {
    pub client: String,
    #[serde(default)]
    pub reinstall: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIntegrationsOut {
    pub mcp_url: String,
    pub resource_dir: Option<String>,
    pub clients: Vec<ClientIntegrationStatus>,
    pub last_action: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIntegrationStatus {
    pub client: String,
    pub label: String,
    pub installed: bool,
    pub available: bool,
    pub health: String,
    pub summary: String,
    pub details: Vec<String>,
    pub version: Option<String>,
    pub bundled_version: Option<String>,
    pub paths: Vec<ClientIntegrationPathStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientIntegrationPathStatus {
    pub label: String,
    pub path: String,
    pub exists: bool,
}

pub fn source_clients_dir() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("clients");
    path.exists().then_some(path)
}

pub fn build_client_integrations_status(
    resource_dir: Option<PathBuf>,
    last_action: Option<String>,
) -> ClientIntegrationsOut {
    let resource_dir_display = resource_dir.as_ref().map(|path| path.display().to_string());
    let clients = match home_dir() {
        Ok(home) => vec![
            claude_code_status(resource_dir.as_deref(), &home),
            codex_status(resource_dir.as_deref(), &home),
            opencode_status(resource_dir.as_deref(), &home),
        ],
        Err(e) => vec![
            unavailable_client_status(
                "claude-code",
                "Claude Code",
                format!("无法定位用户目录: {e:#}"),
            ),
            unavailable_client_status("codex", "Codex", format!("无法定位用户目录: {e:#}")),
            unavailable_client_status("opencode", "OpenCode", format!("无法定位用户目录: {e:#}")),
        ],
    };

    ClientIntegrationsOut {
        mcp_url: DESKTOP_MCP_URL.to_string(),
        resource_dir: resource_dir_display,
        clients,
        last_action,
    }
}

pub fn install_client_integration(
    resource_dir: Option<PathBuf>,
    request: ClientIntegrationInstallRequest,
) -> anyhow::Result<ClientIntegrationsOut> {
    let resource_dir = resource_dir
        .ok_or_else(|| anyhow::anyhow!("内置客户端集成资源缺失：无法找到 clients 目录"))?;
    let home = home_dir()?;
    let client = request.client.trim().to_string();
    let action = match client.as_str() {
        "claude-code" => {
            install_claude_code_plugin(&resource_dir, &home)?;
            if request.reinstall {
                "已重装 Claude Code 插件"
            } else {
                "已安装/更新 Claude Code 插件"
            }
        }
        "codex" => {
            install_codex_integration(&home, request.reinstall)?;
            if request.reinstall {
                "已重装 Codex MCP 配置"
            } else {
                "已安装/更新 Codex MCP 配置"
            }
        }
        "opencode" => {
            install_opencode_integration(&resource_dir, &home)?;
            if request.reinstall {
                "已重装 OpenCode 集成"
            } else {
                "已安装/更新 OpenCode 集成"
            }
        }
        other => anyhow::bail!("unsupported client integration: {other}"),
    };

    Ok(build_client_integrations_status(
        Some(resource_dir),
        Some(action.to_string()),
    ))
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

fn read_opencode_plugin_version(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(OPENCODE_PLUGIN_VERSION_PREFIX)
            .map(str::to_string)
    })
}

fn claude_code_target_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("skills").join("aka")
}

fn codex_config_path(home: &Path) -> PathBuf {
    home.join(".codex").join("config.toml")
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
        details.push(
            "安装到 ~/.claude/skills/aka，作为 Claude Code skills-directory plugin 加载。"
                .to_string(),
        );
    }
    if !available {
        details.push("内置 Claude Code 插件资源缺失。".to_string());
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

fn codex_status(resource_dir: Option<&Path>, home: &Path) -> ClientIntegrationStatus {
    let source = resource_dir.map(|dir| dir.join("codex"));
    let config = codex_config_path(home);
    let configured = codex_mcp_configured(&config);
    let available = source.as_ref().is_some_and(|path| path.exists());
    let version = codex_installed_version(&config);
    let mut details = vec![
        format!("MCP endpoint: {DESKTOP_MCP_URL}"),
        "Codex 使用 ~/.codex/config.toml 的 mcp_servers.aka 配置；无独立 plugin manifest。"
            .to_string(),
    ];
    if configured {
        details.push("重启 Codex 或新开会话后生效。".to_string());
    } else {
        details.push("会写入 mcp_servers.aka，默认连接 AKA 桌面端本地 HTTP MCP。".to_string());
    }
    if !available {
        details.push("内置 Codex 集成资源缺失。".to_string());
    }

    ClientIntegrationStatus {
        client: "codex".to_string(),
        label: "Codex".to_string(),
        installed: configured,
        available,
        health: if configured { "ready" } else { "not_installed" }.to_string(),
        summary: if configured {
            "Codex MCP 配置已就绪。".to_string()
        } else {
            "尚未配置 Codex 使用 AKA MCP。".to_string()
        },
        details,
        version,
        bundled_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        paths: vec![
            path_status("Config", config),
            path_status(
                "Guidance",
                source
                    .map(|path| path.join("AGENTS-aka.md"))
                    .unwrap_or_default(),
            ),
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
        details
            .push("会合并 mcp.aka，并安装 OpenCode plugin 与 aka-code-graph skill。".to_string());
    }
    if !available {
        details.push("内置 OpenCode 集成资源缺失。".to_string());
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
        version: read_opencode_plugin_version(&plugin),
        bundled_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        paths: vec![
            path_status("Config", config),
            path_status("Plugin", plugin),
            path_status("Skill", skill),
        ],
    }
}

fn codex_mcp_configured(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Some(section) = toml_section(&text, "[mcp_servers.aka]") else {
        return false;
    };
    section.contains(&format!("url = \"{DESKTOP_MCP_URL}\""))
}

fn codex_installed_version(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let section_start = text.find("[mcp_servers.aka]")?;
    let before = &text[..section_start];
    before.lines().rev().take(3).find_map(|line| {
        line.trim()
            .strip_prefix("# AKA client integration version = ")
            .map(str::to_string)
    })
}

fn toml_section<'a>(text: &'a str, header: &str) -> Option<&'a str> {
    let start = text.find(header)?;
    let after = &text[start + header.len()..];
    let end = after
        .lines()
        .scan(header.len(), |offset, line| {
            let line_start = *offset;
            *offset += line.len() + 1;
            Some((line_start, line))
        })
        .find_map(|(offset, line)| {
            let trimmed = line.trim();
            (trimmed.starts_with('[') && trimmed.ends_with(']')).then_some(offset)
        })
        .unwrap_or(text.len() - start);
    Some(&text[start..start + end])
}

fn remove_toml_section(text: &str, header: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            skipping = true;
            continue;
        }
        if skipping && trimmed.starts_with('[') && trimmed.ends_with(']') {
            skipping = false;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
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
    let enabled = aka.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
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
        read_json_file(path)
            .map_err(|e| anyhow::anyhow!("无法读取 OpenCode config {}: {e:#}", path.display()))?
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

fn install_claude_code_plugin(resource_dir: &Path, home: &Path) -> anyhow::Result<()> {
    let source = resource_dir.join("claude-code");
    let target = claude_code_target_dir(home);
    require_safe_claude_plugin_target(&target)?;
    copy_dir_recursive(&source, &target)?;
    Ok(())
}

fn install_codex_integration(home: &Path, reinstall: bool) -> anyhow::Result<()> {
    let config = codex_config_path(home);
    let existing = fs::read_to_string(&config).unwrap_or_default();
    if existing.contains("[mcp_servers.aka]") && !codex_mcp_configured(&config) && !reinstall {
        anyhow::bail!(
            "Codex config 已有非 AKA mcp_servers.aka，请点击 Reinstall 覆盖或手动处理: {}",
            config.display()
        );
    }
    let mut next = remove_toml_section(&existing, "[mcp_servers.aka]");
    if !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(&format!(
        "\n# AKA client integration version = {}\n[mcp_servers.aka]\nurl = \"{DESKTOP_MCP_URL}\"\n",
        env!("CARGO_PKG_VERSION")
    ));
    let Some(parent) = config.parent() else {
        anyhow::bail!("Codex config path has no parent: {}", config.display());
    };
    fs::create_dir_all(parent)?;
    if reinstall && config.exists() {
        let backup = config.with_extension(format!(
            "toml.aka-backup-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ));
        fs::copy(&config, backup)?;
    }
    fs::write(config, next)?;
    Ok(())
}

fn install_opencode_integration(resource_dir: &Path, home: &Path) -> anyhow::Result<()> {
    let source = resource_dir.join("opencode");
    require_safe_opencode_plugin_target(&opencode_plugin_path(home))?;
    require_safe_aka_skill_target(&opencode_skill_dir(home))?;
    merge_opencode_config(&opencode_config_path(home))?;
    let plugin_source = source.join("plugins").join("aka.js");
    let mut plugin_text = fs::read_to_string(&plugin_source).map_err(|e| {
        anyhow::anyhow!(
            "无法读取 OpenCode plugin {}: {e:#}",
            plugin_source.display()
        )
    })?;
    if plugin_text.starts_with(OPENCODE_PLUGIN_VERSION_PREFIX) {
        plugin_text = plugin_text.lines().skip(1).collect::<Vec<_>>().join("\n");
        if !plugin_text.ends_with('\n') {
            plugin_text.push('\n');
        }
    }
    plugin_text = format!(
        "{OPENCODE_PLUGIN_VERSION_PREFIX}{}\n{plugin_text}",
        env!("CARGO_PKG_VERSION")
    );
    let plugin_target = opencode_plugin_path(home);
    let Some(parent) = plugin_target.parent() else {
        anyhow::bail!(
            "OpenCode plugin path has no parent: {}",
            plugin_target.display()
        );
    };
    fs::create_dir_all(parent)?;
    fs::write(plugin_target, plugin_text)?;
    copy_dir_recursive(
        &source.join("skills").join("aka-code-graph"),
        &opencode_skill_dir(home),
    )?;
    Ok(())
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
            "aka-client-integrations-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn codex_config_install_rewrites_aka_section_only() {
        let dir = temp_test_dir("codex");
        let home = dir.join("home");
        fs::create_dir_all(home.join(".codex")).unwrap();
        let path = codex_config_path(&home);
        fs::write(
            &path,
            r#"[profile.default]
model = "gpt"

[mcp_servers.other]
url = "https://example.test/mcp"

[mcp_servers.aka]
url = "https://old.example/mcp"

[tools]
enabled = true
"#,
        )
        .unwrap();

        install_codex_integration(&home, true).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[profile.default]"));
        assert!(text.contains("[mcp_servers.other]"));
        assert!(text.contains("[tools]"));
        assert!(text.contains(&format!("url = \"{DESKTOP_MCP_URL}\"")));
        assert!(codex_mcp_configured(&path));
        assert_eq!(
            codex_installed_version(&path).as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        fs::remove_dir_all(dir).unwrap();
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
    fn install_opencode_writes_detectable_plugin_version() {
        let dir = temp_test_dir("opencode-version");
        let resource = dir.join("resource");
        let home = dir.join("home");
        fs::create_dir_all(resource.join("opencode").join("plugins")).unwrap();
        fs::create_dir_all(
            resource
                .join("opencode")
                .join("skills")
                .join("aka-code-graph"),
        )
        .unwrap();
        fs::write(
            resource.join("opencode").join("plugins").join("aka.js"),
            "console.log('aka OpenCode plugin loaded');\n",
        )
        .unwrap();
        fs::write(
            resource
                .join("opencode")
                .join("skills")
                .join("aka-code-graph")
                .join("SKILL.md"),
            "---\nname: aka-code-graph\n---\naka 代码知识图谱\n",
        )
        .unwrap();

        install_opencode_integration(&resource, &home).unwrap();

        let plugin = opencode_plugin_path(&home);
        assert_eq!(
            read_opencode_plugin_version(&plugin).as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
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
