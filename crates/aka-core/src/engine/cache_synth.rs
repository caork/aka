use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, find_matching_paren, node_at_offset, pick_handler_node,
    project_code_nodes_by_file, read_repo_text, split_top_level_commas, stable_hash, EdgeRec,
    NodeRec, ProjectSourceSet, SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct SynthCache {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) backend: String,
    pub(super) readers: Vec<SynthCacheEndpoint>,
    pub(super) writers: Vec<SynthCacheEndpoint>,
    pub(super) evictors: Vec<SynthCacheEndpoint>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct SynthCacheEndpoint {
    node_id: String,
    file_path: String,
    strategy: String,
}

impl SynthCache {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("backend".into(), Value::String(self.backend.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("cacheSource".into(), Value::String("source-scan".into()));
        NodeRec {
            id: self.id.clone(),
            label: "Cache".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::new();
        for endpoint in &self.readers {
            out.push(self.edge_rec(endpoint, "READS_CACHE", "cache-reader"));
        }
        for endpoint in &self.writers {
            out.push(self.edge_rec(endpoint, "WRITES_CACHE", "cache-writer"));
        }
        for endpoint in &self.evictors {
            out.push(self.edge_rec(endpoint, "EVICTS_CACHE", "cache-evictor"));
        }
        out
    }

    fn edge_rec(&self, endpoint: &SynthCacheEndpoint, edge_type: &str, kind: &str) -> EdgeRec {
        EdgeRec {
            id: format!(
                "{}:{}:{:016x}",
                self.id,
                edge_type.to_ascii_lowercase(),
                stable_hash(&format!(
                    "{}|{}|{}",
                    endpoint.node_id, endpoint.strategy, edge_type
                ))
            ),
            source_id: endpoint.node_id.clone(),
            target_id: self.id.clone(),
            edge_type: edge_type.into(),
            confidence: 0.67,
            reason: "aka cache synthesis".into(),
            step: None,
            evidence: Some(json!({
                "source": "aka-cbm-synth",
                "kind": kind,
                "backend": self.backend,
                "cache": self.name,
                "strategy": endpoint.strategy,
                "filePath": endpoint.file_path,
            })),
        }
    }
}

pub(super) fn synthesize_caches_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> Vec<SynthCache> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut caches: BTreeMap<(String, String), SynthCache> = BTreeMap::new();
    let mut seen_edges: HashSet<(String, String, String, String)> = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for detection in extract_cache_detections(&text, &file_path, &file_nodes) {
            let key = (detection.backend.clone(), detection.name.clone());
            let id = format!(
                "cache:heuristic:{:016x}",
                stable_hash(&format!("{}|{}", detection.backend, detection.name))
            );
            let cache = caches.entry(key).or_insert_with(|| SynthCache {
                id,
                name: detection.name.clone(),
                backend: detection.backend.clone(),
                readers: Vec::new(),
                writers: Vec::new(),
                evictors: Vec::new(),
            });
            let edge_key = (
                detection.kind.as_str().to_string(),
                detection.backend,
                detection.name,
                detection.node_id.clone(),
            );
            if !seen_edges.insert(edge_key) {
                continue;
            }
            let endpoint = SynthCacheEndpoint {
                node_id: detection.node_id,
                file_path: file_path.clone(),
                strategy: detection.strategy,
            };
            match detection.kind {
                CacheAccessKind::Read => cache.readers.push(endpoint),
                CacheAccessKind::Write => cache.writers.push(endpoint),
                CacheAccessKind::Evict => cache.evictors.push(endpoint),
            }
        }
    }
    let mut out: Vec<SynthCache> = caches.into_values().collect();
    for cache in &mut out {
        cache.readers.sort();
        cache.readers.dedup();
        cache.writers.sort();
        cache.writers.dedup();
        cache.evictors.sort();
        cache.evictors.dedup();
    }
    out.sort_by(|a, b| a.backend.cmp(&b.backend).then_with(|| a.name.cmp(&b.name)));
    out
}

#[derive(Debug, Clone, Copy)]
enum CacheAccessKind {
    Read,
    Write,
    Evict,
}

impl CacheAccessKind {
    fn as_str(self) -> &'static str {
        match self {
            CacheAccessKind::Read => "read",
            CacheAccessKind::Write => "write",
            CacheAccessKind::Evict => "evict",
        }
    }
}

#[derive(Debug, Clone)]
struct CacheDetection {
    name: String,
    backend: String,
    kind: CacheAccessKind,
    node_id: String,
    strategy: String,
}

fn extract_cache_detections(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
) -> Vec<CacheDetection> {
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
        out.extend(extract_jvm_cache_detections(text, nodes));
    }
    if lower.ends_with(".py")
        || nodes
            .iter()
            .any(|node| node.language.eq_ignore_ascii_case("python"))
    {
        out.extend(extract_python_cache_detections(text, nodes));
    }
    out.sort_by(|a, b| {
        a.backend
            .cmp(&b.backend)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out
}

fn extract_jvm_cache_detections(text: &str, nodes: &[&SynthNode]) -> Vec<CacheDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        for decorator in &node.decorators {
            let Some(name) = decorator_name(decorator) else {
                continue;
            };
            let (kind, strategy) = match name {
                "Cacheable" => (CacheAccessKind::Read, "java-spring-cacheable"),
                "CachePut" => (CacheAccessKind::Write, "java-spring-cache-put"),
                "CacheEvict" => (CacheAccessKind::Evict, "java-spring-cache-evict"),
                _ => continue,
            };
            for cache_name in
                annotation_string_values(decorator, &["cacheNames", "value", "cacheName"])
            {
                out.push(CacheDetection {
                    name: cache_name,
                    backend: "spring-cache".into(),
                    kind,
                    node_id: node.aka_id.clone(),
                    strategy: strategy.into(),
                });
            }
        }
    }
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        ".opsForValue().get",
        "redis",
        CacheAccessKind::Read,
        "java-redis-value-get",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        ".opsForValue().set",
        "redis",
        CacheAccessKind::Write,
        "java-redis-value-set",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        ".delete",
        "redis",
        CacheAccessKind::Evict,
        "java-redis-delete",
        0,
    ));
    out
}

fn extract_python_cache_detections(text: &str, nodes: &[&SynthNode]) -> Vec<CacheDetection> {
    let mut out = Vec::new();
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "cache.get",
        "django-cache",
        CacheAccessKind::Read,
        "python-cache-get",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "cache.get_many",
        "django-cache",
        CacheAccessKind::Read,
        "python-cache-get-many",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "cache.set",
        "django-cache",
        CacheAccessKind::Write,
        "python-cache-set",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "cache.set_many",
        "django-cache",
        CacheAccessKind::Write,
        "python-cache-set-many",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "cache.delete",
        "django-cache",
        CacheAccessKind::Evict,
        "python-cache-delete",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "redis.get",
        "redis",
        CacheAccessKind::Read,
        "python-redis-get",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "redis.mget",
        "redis",
        CacheAccessKind::Read,
        "python-redis-mget",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "redis.set",
        "redis",
        CacheAccessKind::Write,
        "python-redis-set",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "redis.mset",
        "redis",
        CacheAccessKind::Write,
        "python-redis-mset",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "redis.delete",
        "redis",
        CacheAccessKind::Evict,
        "python-redis-delete",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "r.get",
        "redis",
        CacheAccessKind::Read,
        "python-redis-get",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "r.mget",
        "redis",
        CacheAccessKind::Read,
        "python-redis-mget",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "r.set",
        "redis",
        CacheAccessKind::Write,
        "python-redis-set",
        0,
    ));
    out.extend(extract_call_cache_literals(
        text,
        nodes,
        "r.mset",
        "redis",
        CacheAccessKind::Write,
        "python-redis-mset",
        0,
    ));
    out
}

fn extract_call_cache_literals(
    text: &str,
    nodes: &[&SynthNode],
    callee: &str,
    backend: &str,
    kind: CacheAccessKind,
    strategy: &str,
    arg_index: usize,
) -> Vec<CacheDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, callee) {
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        let args = split_top_level_commas(call.args);
        let Some(arg) = args.get(arg_index) else {
            continue;
        };
        for name in cache_name_literals(arg) {
            out.push(CacheDetection {
                name,
                backend: backend.into(),
                kind,
                node_id: node.aka_id.clone(),
                strategy: strategy.into(),
            });
        }
    }
    out
}

fn decorator_name(decorator: &str) -> Option<&str> {
    let text = decorator.trim().trim_start_matches('@');
    let name = text.split_once('(').map(|(name, _)| name).unwrap_or(text);
    Some(name.rsplit('.').next().unwrap_or(name).trim()).filter(|name| !name.is_empty())
}

fn annotation_string_values(annotation: &str, keys: &[&str]) -> Vec<String> {
    let Some(open) = annotation.find('(') else {
        return Vec::new();
    };
    let close = find_matching_paren(annotation, open).unwrap_or(annotation.len());
    let args = &annotation[open + 1..close];
    let mut values = Vec::new();
    for part in split_top_level_commas(args) {
        let part = part.trim();
        let value = if let Some((key, value)) = part.split_once('=') {
            if !keys.iter().any(|expected| key.trim().ends_with(expected)) {
                continue;
            }
            value.trim()
        } else if keys.contains(&"value") {
            part
        } else {
            continue;
        };
        values.extend(cache_name_literals(value));
    }
    values.sort();
    values.dedup();
    values
}

fn cache_name_literals(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some(byte) = text.as_bytes().get(idx).copied() else {
            break;
        };
        if matches!(byte, b'\'' | b'"' | b'`') {
            if let Some((literal, end)) = read_raw_string_literal(text, idx) {
                if is_cache_literal(&literal) {
                    values.push(literal);
                }
                idx = end;
                continue;
            }
        }
        idx += 1;
    }
    values.sort();
    values.dedup();
    values
}

fn read_raw_string_literal(text: &str, start: usize) -> Option<(String, usize)> {
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

fn is_cache_literal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 180
        && !value.contains("://")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/' | '*'))
}
