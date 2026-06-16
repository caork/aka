use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, node_at_offset, project_code_nodes_by_file, read_repo_text,
    split_top_level_commas, stable_hash, EdgeRec, NodeRec, ProjectSourceSet, SynthNode,
};

mod http;
use http::extract_http_resources;
mod http_config;
use http_config::extract_http_config_resources;
mod identity;
use identity::extract_identity_resources;
pub(super) mod infra_config;
use infra_config::extract_infra_config_resources;
mod notification;
use notification::extract_notification_resources;
mod payment;
use payment::extract_payment_resources;
mod java_s3;
use java_s3::extract_java_aws_s3_resources;
mod java_azure_blob;
use java_azure_blob::extract_java_azure_blob_resources;
mod feature_flag;
use feature_flag::extract_feature_flag_resources;
mod python_gcs;
use python_gcs::extract_python_gcs_resources;
mod python_azure_blob;
use python_azure_blob::extract_python_azure_blob_resources;
mod search_index;
use search_index::extract_search_index_resources;
mod storage_config;
use storage_config::extract_storage_config_resources;

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
        let (edge_type, evidence_kind, edge_id_segment) = match self.resource_type.as_str() {
            "http" => ("HTTP_CALLS", "external-http-resource", "http-calls"),
            _ => (
                "ACCESSES_RESOURCE",
                "external-resource",
                "accesses-resource",
            ),
        };
        self.callers
            .iter()
            .map(|caller| EdgeRec {
                id: format!(
                    "{}:{}:{:016x}",
                    self.id,
                    edge_id_segment,
                    stable_hash(&format!("{}|{}", caller.node_id, caller.strategy))
                ),
                source_id: caller.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: edge_type.into(),
                confidence: 0.66,
                reason: "aka external resource synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": evidence_kind,
                    "resource": self.name,
                    "resourceType": self.resource_type,
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
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut resources: BTreeMap<String, SynthResource> = BTreeMap::new();
    let mut seen_edges: HashSet<(String, String)> = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for detection in extract_resource_detections(&text, &file_path, &file_nodes) {
            ingest_resource_detection(&mut resources, &mut seen_edges, &file_path, detection);
        }
    }
    for file_path in project_sources
        .project_files(repo)
        .filter(|file_path| is_resource_config_file_path(file_path))
    {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        for detection in extract_config_resource_detections(&text) {
            ingest_resource_detection(&mut resources, &mut seen_edges, file_path, detection);
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

fn ingest_resource_detection(
    resources: &mut BTreeMap<String, SynthResource>,
    seen_edges: &mut HashSet<(String, String)>,
    file_path: &str,
    detection: ResourceDetection,
) {
    let key = detection.url.clone();
    let id = format!(
        "resource:{}:{:016x}",
        detection.resource_type,
        stable_hash(&key)
    );
    let resource = resources
        .entry(key.clone())
        .or_insert_with(|| SynthResource {
            id,
            name: resource_name(&key),
            url: key,
            resource_type: detection.resource_type.clone(),
            callers: Vec::new(),
        });
    let edge_key = (resource.id.clone(), detection.node_id.clone());
    if !seen_edges.insert(edge_key) {
        return;
    }
    resource.callers.push(SynthResourceCaller {
        node_id: detection.node_id,
        file_path: file_path.to_string(),
        strategy: detection.strategy,
    });
}

fn extract_config_resource_detections(text: &str) -> Vec<ResourceDetection> {
    let mut out = identity::extract_identity_config_resources(text);
    out.extend(extract_infra_config_resources(text));
    out.extend(extract_http_config_resources(text));
    out.extend(extract_storage_config_resources(text));
    out.extend(feature_flag::extract_feature_flag_config_resources(text));
    out.extend(payment::extract_payment_config_resources(text));
    out.extend(notification::extract_notification_config_resources(text));
    out
}

fn is_resource_config_file_path(file_path: &str) -> bool {
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
}

#[derive(Debug, Clone)]
struct ResourceDetection {
    url: String,
    resource_type: String,
    node_id: String,
    strategy: String,
}

impl ResourceDetection {
    fn http(url: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url,
            resource_type: "http".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn s3(url: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url,
            resource_type: "s3".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn gcs(url: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url,
            resource_type: "gcs".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn azure_blob(url: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url,
            resource_type: "azure-blob".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn search_index(index: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url: format!("search-index:{index}"),
            resource_type: "search-index".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn feature_flag(flag: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url: format!("feature-flag:{flag}"),
            resource_type: "feature-flag".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn notification(channel: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url: format!("notification:{channel}"),
            resource_type: "notification".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn payment(provider: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url: format!("payment:{provider}"),
            resource_type: "payment".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn identity(provider: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url: format!("identity:{provider}"),
            resource_type: "identity".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn database(url: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url,
            resource_type: "database".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn redis(url: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url,
            resource_type: "redis".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn kafka(url: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url,
            resource_type: "kafka".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn rabbitmq(url: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url,
            resource_type: "rabbitmq".into(),
            node_id,
            strategy: strategy.into(),
        }
    }

    fn mongodb(url: String, node_id: String, strategy: impl Into<String>) -> Self {
        Self {
            url,
            resource_type: "mongodb".into(),
            node_id,
            strategy: strategy.into(),
        }
    }
}

fn extract_resource_detections(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_http_resources(text, file_path, nodes));
    out.extend(extract_python_boto3_s3_resources(text, nodes));
    out.extend(extract_python_minio_s3_resources(text, nodes));
    out.extend(extract_python_gcs_resources(text, nodes));
    out.extend(extract_python_azure_blob_resources(text, nodes));
    out.extend(extract_java_aws_s3_resources(text, nodes));
    out.extend(extract_java_azure_blob_resources(text, nodes));
    out.extend(extract_search_index_resources(text, nodes));
    out.extend(extract_feature_flag_resources(text, nodes));
    out.extend(extract_identity_resources(text, nodes));
    out.extend(extract_notification_resources(text, nodes));
    out.extend(extract_payment_resources(text, nodes));
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| {
                resource_strategy_rank(&a.strategy).cmp(&resource_strategy_rank(&b.strategy))
            })
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

pub(super) fn resource_strategy_rank(strategy: &str) -> u8 {
    if strategy == "literal-http-url" || matches!(strategy, "python-gcs-blob" | "python-azure-blob")
    {
        10
    } else {
        0
    }
}

fn extract_python_boto3_s3_resources(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    if !(text.contains("boto3") || text.contains("s3")) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (callee, strategy, bucket_arg, key_arg) in [
        (".put_object", "python-boto3-s3-put-object", "Bucket", "Key"),
        (".get_object", "python-boto3-s3-get-object", "Bucket", "Key"),
        (
            ".delete_object",
            "python-boto3-s3-delete-object",
            "Bucket",
            "Key",
        ),
        (
            ".upload_file",
            "python-boto3-s3-upload-file",
            "Bucket",
            "Key",
        ),
        (
            ".download_file",
            "python-boto3-s3-download-file",
            "Bucket",
            "Key",
        ),
    ] {
        for call in find_call_args(text, callee) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            let Some((bucket, key)) = s3_bucket_key_from_args(call.args, bucket_arg, key_arg)
            else {
                continue;
            };
            out.push(ResourceDetection::s3(
                s3_url(&bucket, key.as_deref()),
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then_with(|| a.node_id.cmp(&b.node_id)));
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn extract_python_minio_s3_resources(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    if !(text.contains("Minio") || text.contains("minio")) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (callee, strategy) in [
        (".put_object", "python-minio-put-object"),
        (".fput_object", "python-minio-put-object"),
        (".get_object", "python-minio-get-object"),
        (".fget_object", "python-minio-get-object"),
        (".remove_object", "python-minio-delete-object"),
        (".stat_object", "python-minio-head-object"),
    ] {
        for call in find_call_args(text, callee) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            let Some((bucket, key)) = minio_bucket_key_from_args(call.args) else {
                continue;
            };
            out.push(ResourceDetection::s3(
                s3_url(&bucket, key.as_deref()),
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then_with(|| a.node_id.cmp(&b.node_id)));
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn minio_bucket_key_from_args(args: &str) -> Option<(String, Option<String>)> {
    let bucket = keyword_literal(args, "bucket_name").or_else(|| positional_literal(args, 0))?;
    let key = keyword_literal(args, "object_name").or_else(|| positional_literal(args, 1));
    Some((
        mask_dynamic_url(&bucket),
        key.map(|value| mask_dynamic_url(&value)),
    ))
}

fn s3_bucket_key_from_args(
    args: &str,
    bucket_arg: &str,
    key_arg: &str,
) -> Option<(String, Option<String>)> {
    let bucket = keyword_literal(args, bucket_arg).or_else(|| positional_literal(args, 0))?;
    let key = keyword_literal(args, key_arg).or_else(|| positional_literal(args, 1));
    Some((
        mask_dynamic_url(&bucket),
        key.map(|value| mask_dynamic_url(&value)),
    ))
}

fn s3_url(bucket: &str, key: Option<&str>) -> String {
    match key.filter(|value| !value.is_empty()) {
        Some(key) => format!(
            "s3://{}/{}",
            bucket.trim_matches('/'),
            key.trim_start_matches('/')
        ),
        None => format!("s3://{}", bucket.trim_matches('/')),
    }
}

fn keyword_literal(args: &str, key: &str) -> Option<String> {
    for part in split_top_level_commas(args) {
        let trimmed = part.trim();
        let Some(rest) = trimmed.strip_prefix(key) else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let Some((literal, _)) = read_python_string_literal(value, 0) else {
            continue;
        };
        return Some(literal);
    }
    None
}

fn positional_literal(args: &str, index: usize) -> Option<String> {
    split_top_level_commas(args)
        .into_iter()
        .filter(|arg| !arg.contains('='))
        .nth(index)
        .and_then(|arg| read_python_string_literal(arg.trim(), 0).map(|(literal, _)| literal))
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

fn read_python_string_literal(text: &str, start: usize) -> Option<(String, usize)> {
    let mut idx = start;
    let mut saw_f_string = false;
    while let Some(ch) = text[idx..].chars().next() {
        if matches!(ch, 'f' | 'F' | 'r' | 'R' | 'u' | 'U' | 'b' | 'B') {
            saw_f_string |= matches!(ch, 'f' | 'F');
            idx += ch.len_utf8();
            continue;
        }
        break;
    }
    let (literal, end) = read_string_literal(text, idx)?;
    if saw_f_string {
        Some((mask_dynamic_url(&literal), end))
    } else {
        Some((literal, end))
    }
}
