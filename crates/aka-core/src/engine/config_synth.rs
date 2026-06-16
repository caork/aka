use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, is_noisy_source_path, node_at_offset, nodes_by_file, pick_handler_node,
    read_repo_text, read_string_literal, source_annotations_before_node, stable_hash, EdgeRec,
    NodeRec, ProjectSourceSet, SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct SynthConfig {
    pub(super) id: String,
    pub(super) key: String,
    pub(super) config_type: String,
    pub(super) file_path: String,
    value_hint: Option<String>,
    sources: BTreeSet<String>,
    consumers: BTreeMap<String, ConfigConsumer>,
}

#[derive(Debug, Clone)]
struct ConfigConsumer {
    node_id: String,
    file_path: String,
    strategies: BTreeSet<String>,
}

impl SynthConfig {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.key.clone()));
        properties.insert("key".into(), Value::String(self.key.clone()));
        properties.insert("configType".into(), Value::String(self.config_type.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("configSource".into(), Value::String("source-scan".into()));
        if let Some(value_hint) = &self.value_hint {
            properties.insert("valueHint".into(), Value::String(value_hint.clone()));
        }
        if !self.sources.is_empty() {
            properties.insert(
                "sources".into(),
                Value::Array(self.sources.iter().cloned().map(Value::String).collect()),
            );
        }
        NodeRec {
            id: self.id.clone(),
            label: "Config".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        self.consumers
            .values()
            .map(|consumer| EdgeRec {
                id: format!(
                    "{}:used-by:{:016x}",
                    self.id,
                    stable_hash(&consumer.node_id)
                ),
                source_id: consumer.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: "USES_CONFIG".into(),
                confidence: 0.68,
                reason: "aka config synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "config-consumer",
                    "key": self.key,
                    "configType": self.config_type,
                    "filePath": consumer.file_path,
                    "strategies": consumer.strategies,
                })),
            })
            .collect()
    }
}

pub(super) fn synthesize_configs_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> Vec<SynthConfig> {
    let mut configs: BTreeMap<String, SynthConfig> = BTreeMap::new();
    let project_sources = ProjectSourceSet::discover(repo);
    for declaration in detect_config_declarations(repo, &project_sources) {
        let id = config_id(&declaration.key);
        let config = configs.entry(id.clone()).or_insert_with(|| SynthConfig {
            id,
            key: declaration.key.clone(),
            config_type: declaration.config_type.clone(),
            file_path: declaration.file_path.clone(),
            value_hint: declaration.value_hint.clone(),
            sources: BTreeSet::new(),
            consumers: BTreeMap::new(),
        });
        config.sources.insert(declaration.strategy);
        if config.value_hint.is_none() {
            config.value_hint = declaration.value_hint;
        }
    }

    for (file_path, file_nodes) in config_nodes_by_file(repo, nodes, &project_sources) {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for usage in detect_config_usages(&text, &file_path, &file_nodes) {
            let id = config_id(&usage.key);
            let config = configs.entry(id.clone()).or_insert_with(|| SynthConfig {
                id,
                key: usage.key.clone(),
                config_type: usage.config_type.clone(),
                file_path: file_path.clone(),
                value_hint: None,
                sources: BTreeSet::new(),
                consumers: BTreeMap::new(),
            });
            config.sources.insert(usage.strategy.clone());
            config
                .consumers
                .entry(usage.node_id.clone())
                .or_insert_with(|| ConfigConsumer {
                    node_id: usage.node_id,
                    file_path: usage.file_path,
                    strategies: BTreeSet::new(),
                })
                .strategies
                .insert(usage.strategy);
        }
    }

    let mut out: Vec<_> = configs.into_values().collect();
    out.sort_by(|a, b| a.key.cmp(&b.key).then_with(|| a.id.cmp(&b.id)));
    out
}

#[derive(Debug, Clone)]
struct ConfigDeclaration {
    key: String,
    config_type: String,
    file_path: String,
    value_hint: Option<String>,
    strategy: String,
}

#[derive(Debug, Clone)]
struct ConfigUsage {
    key: String,
    config_type: String,
    file_path: String,
    node_id: String,
    strategy: String,
}

fn detect_config_declarations(
    repo: &Path,
    project_sources: &ProjectSourceSet,
) -> Vec<ConfigDeclaration> {
    let mut out = Vec::new();
    if project_sources.has_git_listing() {
        for file_path in project_sources.iter().filter(|path| {
            is_config_file_path(path) && project_sources.contains_project_file(repo, path)
        }) {
            let Some(text) = read_repo_text(repo, file_path) else {
                continue;
            };
            out.extend(declarations_from_file(file_path, &text));
        }
        return out;
    }

    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let rel = path
                    .strip_prefix(repo)
                    .ok()
                    .map(|path| path.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if !is_noisy_config_dir(&rel) {
                    stack.push(path);
                }
                continue;
            }
            let Ok(rel) = path.strip_prefix(repo) else {
                continue;
            };
            let file_path = rel.to_string_lossy().replace('\\', "/");
            if !is_config_file_path(&file_path)
                || !project_sources.contains_project_file(repo, &file_path)
            {
                continue;
            }
            let Some(text) = std::fs::read_to_string(&path).ok() else {
                continue;
            };
            out.extend(declarations_from_file(&file_path, &text));
        }
    }
    out
}

fn is_noisy_config_dir(path: &str) -> bool {
    let path = path.replace('\\', "/");
    path.split('/').any(|segment| {
        matches!(
            segment,
            ".git" | ".hg" | ".svn" | "node_modules" | "target" | "build" | "dist"
        )
    }) || is_noisy_source_path(&path)
}

fn is_config_file_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    matches!(
        name,
        "application.yml"
            | "application.yaml"
            | "application.properties"
            | "bootstrap.yml"
            | "bootstrap.yaml"
            | "bootstrap.properties"
            | "settings.py"
            | "config.py"
            | ".env"
    ) || name.starts_with("application-") && name.ends_with(".yml")
        || name.starts_with("application-") && name.ends_with(".yaml")
        || name.starts_with("application-") && name.ends_with(".properties")
        || lower.contains("/settings/")
}

fn declarations_from_file(file_path: &str, text: &str) -> Vec<ConfigDeclaration> {
    let lower = file_path.to_ascii_lowercase();
    if lower.ends_with(".properties") || lower.ends_with(".env") {
        return property_declarations(file_path, text);
    }
    if lower.ends_with(".yml") || lower.ends_with(".yaml") {
        return yaml_declarations(file_path, text);
    }
    if lower.ends_with(".py") {
        return python_settings_declarations(file_path, text);
    }
    Vec::new()
}

fn property_declarations(file_path: &str, text: &str) -> Vec<ConfigDeclaration> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next()?.split('!').next()?.trim();
            if line.is_empty() {
                return None;
            }
            let (key, value) = line.split_once('=').or_else(|| line.split_once(':'))?;
            let key = normalize_config_key(key)?;
            Some(ConfigDeclaration {
                config_type: config_type_for_key(&key),
                key,
                file_path: file_path.into(),
                value_hint: value_hint(value),
                strategy: "properties-file".into(),
            })
        })
        .collect()
}

fn yaml_declarations(file_path: &str, text: &str) -> Vec<ConfigDeclaration> {
    let mut out = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            continue;
        }
        let Some((raw_key, raw_value)) = trimmed.split_once(':') else {
            continue;
        };
        let key_part = raw_key.trim().trim_matches(['"', '\'']);
        if key_part.is_empty() || key_part.contains(' ') {
            continue;
        }
        let indent = line.chars().take_while(|ch| *ch == ' ').count();
        while stack.last().is_some_and(|(level, _)| *level >= indent) {
            stack.pop();
        }
        let mut parts: Vec<String> = stack.iter().map(|(_, key)| key.clone()).collect();
        parts.push(key_part.to_ascii_lowercase());
        let full_key = parts.join(".");
        let value = raw_value.trim();
        if !value.is_empty() && !value.starts_with(['{', '[', '&', '*']) {
            if let Some(key) = normalize_config_key(&full_key) {
                out.push(ConfigDeclaration {
                    config_type: config_type_for_key(&key),
                    key,
                    file_path: file_path.into(),
                    value_hint: value_hint(value),
                    strategy: "yaml-file".into(),
                });
            }
        }
        if value.is_empty() {
            stack.push((indent, key_part.to_ascii_lowercase()));
        }
    }
    out
}

fn python_settings_declarations(file_path: &str, text: &str) -> Vec<ConfigDeclaration> {
    text.lines()
        .filter_map(|line| {
            let code = line.split('#').next()?.trim();
            if code.is_empty() || code.starts_with("class ") || code.starts_with("def ") {
                return None;
            }
            let (key, value) = code.split_once('=')?;
            let key = key.trim();
            if !is_upper_setting_name(key) {
                return None;
            }
            let normalized = normalize_config_key(key)?;
            Some(ConfigDeclaration {
                config_type: "python-setting".into(),
                key: normalized,
                file_path: file_path.into(),
                value_hint: value_hint(value),
                strategy: "python-settings-file".into(),
            })
        })
        .collect()
}

fn config_nodes_by_file<'a>(
    repo: &Path,
    nodes: &'a BTreeMap<String, SynthNode>,
    project_sources: &ProjectSourceSet,
) -> BTreeMap<String, Vec<&'a SynthNode>> {
    nodes_by_file(nodes)
        .into_iter()
        .filter(|(file_path, file_nodes)| {
            project_sources.contains_project_file(repo, file_path)
                && (is_code_config_source_path(file_path)
                    || file_nodes.iter().any(|node| {
                        matches!(
                            node.language.to_ascii_lowercase().as_str(),
                            "java" | "kotlin" | "scala" | "groovy" | "python"
                        )
                    }))
        })
        .collect()
}

fn is_code_config_source_path(file_path: &str) -> bool {
    matches!(
        Path::new(&file_path.to_ascii_lowercase())
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("java" | "kt" | "kts" | "scala" | "groovy" | "py")
    )
}

fn detect_config_usages(text: &str, file_path: &str, nodes: &[&SynthNode]) -> Vec<ConfigUsage> {
    let mut out = Vec::new();
    let lower = file_path.to_ascii_lowercase();
    if lower.ends_with(".java")
        || nodes.iter().any(|node| {
            matches!(
                node.language.to_ascii_lowercase().as_str(),
                "java" | "kotlin" | "scala" | "groovy"
            )
        })
    {
        out.extend(detect_jvm_config_usages(text, file_path, nodes));
    }
    if lower.ends_with(".py")
        || nodes
            .iter()
            .any(|node| node.language.eq_ignore_ascii_case("python"))
    {
        out.extend(detect_python_config_usages(text, file_path, nodes));
    }
    out
}

fn detect_jvm_config_usages(text: &str, file_path: &str, nodes: &[&SynthNode]) -> Vec<ConfigUsage> {
    let mut out = Vec::new();
    for node in nodes {
        for decorator in decorators_for_node(text, node) {
            if decorator_name(&decorator) == Some("Value") {
                if let Some(key) = spring_value_key(&decorator) {
                    out.push(config_usage(
                        key,
                        "spring-property",
                        file_path,
                        &node.aka_id,
                        "java-spring-value",
                    ));
                }
            }
            if decorator_name(&decorator) == Some("ConfigurationProperties") {
                if let Some(prefix) = annotation_arg_string(&decorator, &["prefix", "value"]) {
                    out.push(config_usage(
                        prefix,
                        "spring-property-prefix",
                        file_path,
                        &node.aka_id,
                        "java-spring-configuration-properties",
                    ));
                }
            }
        }
    }
    for call in find_call_args(text, "getProperty") {
        let Some(key) = first_string_literal(call.args).and_then(|key| normalize_config_key(&key))
        else {
            continue;
        };
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        out.push(config_usage(
            key,
            "spring-property",
            file_path,
            &node.aka_id,
            "java-environment-get-property",
        ));
    }
    out
}

fn decorators_for_node(text: &str, node: &SynthNode) -> Vec<String> {
    let mut decorators = node.decorators.clone();
    decorators.extend(source_annotations_before_node(text, node));
    decorators.sort();
    decorators.dedup();
    decorators
}

fn detect_python_config_usages(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
) -> Vec<ConfigUsage> {
    let mut out = Vec::new();
    for callee in ["os.getenv", ".getenv"] {
        for call in find_call_args(text, callee) {
            let Some(key) =
                first_string_literal(call.args).and_then(|key| normalize_config_key(&key))
            else {
                continue;
            };
            let Some(node) =
                node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
            else {
                continue;
            };
            out.push(config_usage(
                key,
                "env-var",
                file_path,
                &node.aka_id,
                "python-os-getenv",
            ));
        }
    }
    for (offset, key) in python_environ_keys(text) {
        let Some(node) = node_at_offset(text, nodes, offset).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        out.push(config_usage(
            key,
            "env-var",
            file_path,
            &node.aka_id,
            "python-os-environ",
        ));
    }
    for (offset, key) in django_settings_keys(text) {
        let Some(node) = node_at_offset(text, nodes, offset).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        out.push(config_usage(
            key,
            "python-setting",
            file_path,
            &node.aka_id,
            "python-django-settings",
        ));
    }
    out
}

fn config_usage(
    key: String,
    config_type: &str,
    file_path: &str,
    node_id: &str,
    strategy: &str,
) -> ConfigUsage {
    ConfigUsage {
        key,
        config_type: config_type.into(),
        file_path: file_path.into(),
        node_id: node_id.into(),
        strategy: strategy.into(),
    }
}

fn python_environ_keys(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for marker in ["os.environ[", "environ["] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(marker) {
            let start = offset + pos;
            let literal_start = start + marker.len();
            if let Some((key, _)) = read_string_literal(text, literal_start) {
                if let Some(key) = normalize_config_key(&key) {
                    out.push((start, key));
                }
            }
            offset = literal_start;
        }
    }
    out
}

fn django_settings_keys(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find("settings.") {
        let start = offset + pos;
        let key_start = start + "settings.".len();
        let key: String = text[key_start..]
            .chars()
            .take_while(|ch| *ch == '_' || ch.is_ascii_alphanumeric())
            .collect();
        if is_upper_setting_name(&key) {
            if let Some(key) = normalize_config_key(&key) {
                out.push((start, key));
            }
        }
        offset = key_start + key.len().max(1);
    }
    out
}

fn spring_value_key(annotation: &str) -> Option<String> {
    let raw = first_string_literal(annotation)?;
    let rest = raw
        .strip_prefix("${")
        .or_else(|| raw.find("${").map(|start| &raw[start + 2..]))?;
    let end = rest.find('}')?;
    let key = rest[..end].split(':').next().unwrap_or("").trim();
    normalize_config_key(key)
}

fn annotation_arg_string(annotation: &str, keys: &[&str]) -> Option<String> {
    let open = annotation.find('(')?;
    let close = super::find_matching_paren(annotation, open).unwrap_or(annotation.len());
    let args = &annotation[open + 1..close];
    for part in super::split_top_level_commas(args) {
        let value = if let Some((key, value)) = part.split_once('=') {
            if !keys.iter().any(|expected| key.trim().ends_with(expected)) {
                continue;
            }
            value.trim()
        } else if keys.contains(&"value") {
            part.trim()
        } else {
            continue;
        };
        if let Some(literal) = first_string_literal(value) {
            return Some(literal);
        }
    }
    None
}

fn first_string_literal(text: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < text.len() {
        let byte = text.as_bytes().get(idx).copied()?;
        if matches!(byte, b'\'' | b'"' | b'`') {
            return read_string_literal(text, idx).map(|(literal, _)| literal);
        }
        idx += 1;
    }
    None
}

fn decorator_name(decorator: &str) -> Option<&str> {
    let text = decorator.trim().trim_start_matches('@');
    let name = text.split_once('(').map(|(name, _)| name).unwrap_or(text);
    Some(name.rsplit('.').next().unwrap_or(name).trim()).filter(|name| !name.is_empty())
}

fn normalize_config_key(key: &str) -> Option<String> {
    let normalized = key
        .trim()
        .trim_matches(['"', '\'', '`'])
        .replace("__", ".")
        .replace('_', ".")
        .to_ascii_lowercase();
    let normalized = normalized.trim_matches('.').to_string();
    is_plausible_config_key(&normalized).then_some(normalized)
}

fn is_plausible_config_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 160
        && key.contains('.')
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '[' | ']'))
}

fn is_upper_setting_name(key: &str) -> bool {
    !key.is_empty()
        && key.chars().any(|ch| ch.is_ascii_alphabetic())
        && key
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_uppercase() || ch.is_ascii_digit())
}

fn config_type_for_key(key: &str) -> String {
    if key.starts_with("spring.") || key.starts_with("server.") || key.starts_with("management.") {
        "spring-property".into()
    } else if key.contains('.') {
        "application-property".into()
    } else {
        "env-var".into()
    }
}

fn value_hint(value: &str) -> Option<String> {
    let value = value
        .trim()
        .trim_matches(['"', '\''])
        .split('#')
        .next()
        .unwrap_or("")
        .trim();
    (!value.is_empty() && value.len() <= 120).then(|| value.to_string())
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
