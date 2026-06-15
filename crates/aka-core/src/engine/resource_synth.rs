use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, node_at_offset, nodes_by_file, read_repo_text, stable_hash, EdgeRec, NodeRec,
    SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct SynthResource {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) url: String,
    pub(super) resource_type: String,
    pub(super) callers: Vec<SynthResourceCaller>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct SynthResourceCaller {
    node_id: String,
    file_path: String,
    strategy: String,
}

impl SynthResource {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("url".into(), Value::String(self.url.clone()));
        properties.insert(
            "resourceType".into(),
            Value::String(self.resource_type.clone()),
        );
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("resourceSource".into(), Value::String("source-scan".into()));
        NodeRec {
            id: self.id.clone(),
            label: "Resource".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        self.callers
            .iter()
            .map(|caller| EdgeRec {
                id: format!(
                    "{}:http-calls:{:016x}",
                    self.id,
                    stable_hash(&format!("{}|{}", caller.node_id, caller.strategy))
                ),
                source_id: caller.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: "HTTP_CALLS".into(),
                confidence: 0.66,
                reason: "aka external resource synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "external-http-resource",
                    "resource": self.name,
                    "url": self.url,
                    "strategy": caller.strategy,
                    "filePath": caller.file_path,
                })),
            })
            .collect()
    }
}

pub(super) fn synthesize_resources_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> Vec<SynthResource> {
    let by_file = resource_nodes_by_file(nodes);
    let mut resources: BTreeMap<String, SynthResource> = BTreeMap::new();
    let mut seen_edges: HashSet<(String, String)> = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for detection in extract_resource_detections(&text, &file_path, &file_nodes) {
            let key = detection.url.clone();
            let id = format!("resource:http:{:016x}", stable_hash(&key));
            let resource = resources
                .entry(key.clone())
                .or_insert_with(|| SynthResource {
                    id,
                    name: resource_name(&key),
                    url: key,
                    resource_type: "http".into(),
                    callers: Vec::new(),
                });
            let edge_key = (resource.id.clone(), detection.node_id.clone());
            if !seen_edges.insert(edge_key) {
                continue;
            }
            resource.callers.push(SynthResourceCaller {
                node_id: detection.node_id,
                file_path: file_path.clone(),
                strategy: detection.strategy,
            });
        }
    }
    let mut out: Vec<SynthResource> = resources.into_values().collect();
    for resource in &mut out {
        resource.callers.sort();
        resource.callers.dedup();
    }
    out.sort_by(|a, b| a.url.cmp(&b.url));
    out
}

#[derive(Debug, Clone)]
struct ResourceDetection {
    url: String,
    node_id: String,
    strategy: String,
}

fn resource_nodes_by_file(
    nodes: &BTreeMap<String, SynthNode>,
) -> BTreeMap<String, Vec<&SynthNode>> {
    nodes_by_file(nodes)
        .into_iter()
        .filter(|(file_path, file_nodes)| {
            is_resource_source_path(file_path)
                || file_nodes.iter().any(|node| {
                    matches!(
                        node.language.to_ascii_lowercase().as_str(),
                        "java" | "kotlin" | "scala" | "groovy" | "python"
                    )
                })
        })
        .collect()
}

fn is_resource_source_path(file_path: &str) -> bool {
    matches!(
        Path::new(&file_path.to_ascii_lowercase())
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("java" | "kt" | "kts" | "scala" | "groovy" | "py")
    )
}

fn extract_resource_detections(
    text: &str,
    _file_path: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in [
        ("requests.get", "python-requests"),
        ("requests.post", "python-requests"),
        ("requests.put", "python-requests"),
        ("requests.patch", "python-requests"),
        ("requests.delete", "python-requests"),
        ("httpx.get", "python-httpx"),
        ("httpx.post", "python-httpx"),
        ("httpx.put", "python-httpx"),
        ("httpx.patch", "python-httpx"),
        ("httpx.delete", "python-httpx"),
        (".getForObject", "java-resttemplate"),
        (".getForEntity", "java-resttemplate"),
        (".postForObject", "java-resttemplate"),
        (".postForEntity", "java-resttemplate"),
        (".exchange", "java-http-client"),
        (".uri", "java-webclient"),
    ] {
        out.extend(extract_call_url_detections(text, nodes, callee, strategy));
    }
    out.extend(extract_absolute_url_literals(text, nodes));
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn extract_call_url_detections(
    text: &str,
    nodes: &[&SynthNode],
    callee: &str,
    strategy: &str,
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, callee) {
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        for url in url_literals(call.args) {
            out.push(ResourceDetection {
                url,
                node_id: node.aka_id.clone(),
                strategy: strategy.into(),
            });
        }
    }
    out
}

fn extract_absolute_url_literals(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some((literal, end)) = read_string_literal(text, idx) else {
            idx += text[idx..].chars().next().map(char::len_utf8).unwrap_or(1);
            continue;
        };
        if let Some(url) = normalize_url_literal(&literal) {
            if let Some(node) = node_at_offset(text, nodes, idx) {
                out.push(ResourceDetection {
                    url,
                    node_id: node.aka_id.clone(),
                    strategy: "literal-http-url".into(),
                });
            }
        }
        idx = end;
    }
    out
}

fn url_literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some((literal, end)) = read_string_literal(text, idx) else {
            idx += text[idx..].chars().next().map(char::len_utf8).unwrap_or(1);
            continue;
        };
        if let Some(url) = normalize_url_literal(&literal) {
            out.push(url);
        }
        idx = end;
    }
    out.sort();
    out.dedup();
    out
}

fn normalize_url_literal(value: &str) -> Option<String> {
    let value = value.trim();
    if value.starts_with("http://") || value.starts_with("https://") {
        return Some(mask_dynamic_url(value));
    }
    if value.starts_with("//") {
        return Some(mask_dynamic_url(&format!("https:{value}")));
    }
    None
}

fn mask_dynamic_url(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' {
            for inner in chars.by_ref() {
                if inner == '}' {
                    break;
                }
            }
            out.push_str("{param}");
        } else {
            out.push(ch);
        }
    }
    out
}

fn resource_name(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    without_scheme
        .split_once('?')
        .map(|(path, _)| path)
        .unwrap_or(without_scheme)
        .trim_end_matches('/')
        .to_string()
}

fn read_string_literal(text: &str, start: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let quote = *bytes.get(start)?;
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut out = String::new();
    let mut escape = false;
    let mut i = start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            let ch = text[i..].chars().next()?;
            out.push(ch);
            escape = false;
            i += ch.len_utf8();
            continue;
        }
        if b == b'\\' {
            escape = true;
        } else if b == quote {
            return Some((out, i + 1));
        } else {
            let ch = text[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
    }
    None
}
