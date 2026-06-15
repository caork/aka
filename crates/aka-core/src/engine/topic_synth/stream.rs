use std::collections::BTreeMap;
use std::path::Path;

use super::{topic_detection, TopicDetection, TopicEndpointKind};
use crate::engine::{
    find_call_args, node_at_offset, pick_handler_node, read_repo_text, split_top_level_commas,
    string_literals, ProjectSourceSet, SynthNode,
};

#[derive(Debug, Clone, Default)]
pub(super) struct StreamBinding {
    destination: Option<String>,
    group: Option<String>,
}

pub(super) fn spring_cloud_stream_bindings(
    repo: &Path,
    project_sources: &ProjectSourceSet,
) -> BTreeMap<String, StreamBinding> {
    let mut out: BTreeMap<String, StreamBinding> = BTreeMap::new();
    for file_path in project_sources
        .project_files(repo)
        .filter(|path| is_spring_config_file(path))
    {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        let lower = file_path.to_ascii_lowercase();
        let pairs = if lower.ends_with(".properties") {
            spring_stream_property_pairs(&text)
        } else if lower.ends_with(".yml") || lower.ends_with(".yaml") {
            spring_stream_yaml_pairs(&text)
        } else {
            Vec::new()
        };
        for (key, value) in pairs {
            let Some((binding, field)) = parse_stream_binding_key(&key) else {
                continue;
            };
            let entry = out.entry(binding.to_string()).or_default();
            match field.as_str() {
                "destination" => entry.destination = Some(value),
                "group" => entry.group = Some(value),
                _ => {}
            }
        }
    }
    out
}

pub(super) fn stream_binding_detections(
    binding: &str,
    stream_bindings: &BTreeMap<String, StreamBinding>,
    kind: TopicEndpointKind,
    node_id: String,
    strategy: &str,
) -> Vec<TopicDetection> {
    let binding = binding.trim();
    if binding.is_empty() {
        return Vec::new();
    }
    let resolved = stream_bindings.get(binding);
    let topic = resolved
        .and_then(|binding| binding.destination.as_deref())
        .unwrap_or(binding)
        .to_string();
    let mut detection = topic_detection(topic, "spring-cloud-stream", kind, node_id, strategy);
    if matches!(kind, TopicEndpointKind::Consumer) {
        if let Some(group) = resolved.and_then(|binding| binding.group.clone()) {
            detection.consumer_groups.push(group);
        }
    }
    vec![detection]
}

pub(super) fn extract_stream_bridge_topics(
    text: &str,
    nodes: &[&SynthNode],
    stream_bindings: &BTreeMap<String, StreamBinding>,
) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    if !text.contains("StreamBridge") {
        return out;
    }
    for call in find_call_args(text, ".send") {
        let args = split_top_level_commas(call.args);
        let Some(binding) = args
            .first()
            .and_then(|arg| string_literals(arg).first().cloned())
        else {
            continue;
        };
        if !stream_bindings.contains_key(&binding) && !binding.contains("-out-") {
            continue;
        }
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        out.extend(stream_binding_detections(
            &binding,
            stream_bindings,
            TopicEndpointKind::Producer,
            node.aka_id.clone(),
            "java-spring-cloud-stream-bridge-send",
        ));
    }
    out
}

pub(super) fn functional_stream_binding_detections(
    text: &str,
    node: &SynthNode,
    stream_bindings: &BTreeMap<String, StreamBinding>,
) -> Vec<TopicDetection> {
    if !node
        .decorators
        .iter()
        .any(|decorator| decorator.contains("Bean"))
    {
        return Vec::new();
    }
    let declaration = java_declaration_window(text, node);
    let mut out = Vec::new();
    if declaration.contains("Supplier<") || declaration.contains("java.util.function.Supplier") {
        out.extend(stream_binding_detections(
            &format!("{}-out-0", node.name),
            stream_bindings,
            TopicEndpointKind::Producer,
            node.aka_id.clone(),
            "java-spring-cloud-stream-function-supplier",
        ));
    }
    if declaration.contains("Consumer<") || declaration.contains("java.util.function.Consumer") {
        out.extend(stream_binding_detections(
            &format!("{}-in-0", node.name),
            stream_bindings,
            TopicEndpointKind::Consumer,
            node.aka_id.clone(),
            "java-spring-cloud-stream-function-consumer",
        ));
    }
    if declaration.contains("Function<") || declaration.contains("java.util.function.Function") {
        out.extend(stream_binding_detections(
            &format!("{}-in-0", node.name),
            stream_bindings,
            TopicEndpointKind::Consumer,
            node.aka_id.clone(),
            "java-spring-cloud-stream-function-input",
        ));
        out.extend(stream_binding_detections(
            &format!("{}-out-0", node.name),
            stream_bindings,
            TopicEndpointKind::Producer,
            node.aka_id.clone(),
            "java-spring-cloud-stream-function-output",
        ));
    }
    out
}

fn is_spring_config_file(file_path: &str) -> bool {
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
    ) || name.starts_with("application-") && name.ends_with(".yml")
        || name.starts_with("application-") && name.ends_with(".yaml")
        || name.starts_with("application-") && name.ends_with(".properties")
}

fn spring_stream_property_pairs(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next()?.split('!').next()?.trim();
            if line.is_empty() {
                return None;
            }
            let (key, value) = line.split_once('=').or_else(|| line.split_once(':'))?;
            let key = key.trim();
            if !key
                .to_ascii_lowercase()
                .starts_with("spring.cloud.stream.bindings.")
            {
                return None;
            }
            clean_config_value(value).map(|value| (key.to_string(), value))
        })
        .collect()
}

fn spring_stream_yaml_pairs(text: &str) -> Vec<(String, String)> {
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
        parts.push(key_part.to_string());
        let full_key = parts.join(".");
        if full_key
            .to_ascii_lowercase()
            .starts_with("spring.cloud.stream.bindings.")
        {
            if let Some(value) = clean_config_value(raw_value) {
                out.push((full_key, value));
            }
        }
        if raw_value.trim().is_empty() {
            stack.push((indent, key_part.to_string()));
        }
    }
    out
}

fn parse_stream_binding_key(key: &str) -> Option<(&str, String)> {
    let prefix = "spring.cloud.stream.bindings.";
    if !key.to_ascii_lowercase().starts_with(prefix) {
        return None;
    }
    let rest = key.get(prefix.len()..)?;
    let (binding, field) = rest.rsplit_once('.')?;
    let binding = binding.trim();
    (!binding.is_empty()).then(|| (binding, field.trim().to_ascii_lowercase()))
}

fn clean_config_value(value: &str) -> Option<String> {
    let value = value
        .split('#')
        .next()
        .unwrap_or(value)
        .trim()
        .trim_matches(['"', '\''])
        .to_string();
    (!value.is_empty()).then_some(value)
}

fn java_declaration_window(text: &str, node: &SynthNode) -> String {
    let line = node.start_line_key().max(1) as usize;
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let idx = line.saturating_sub(1).min(lines.len() - 1);
    let from = idx.saturating_sub(3);
    let to = (idx + 2).min(lines.len());
    lines[from..to].join("\n")
}
