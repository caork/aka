use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, find_matching_paren, node_at_offset, project_code_nodes_by_file,
    read_repo_text, request_line_path, skip_ws, source_annotations_before_node,
    split_top_level_commas, spring_mapping_path, stable_hash, EdgeRec, NodeRec, ProjectSourceSet,
    SynthNode,
};

mod java_s3;
use java_s3::extract_java_aws_s3_resources;
mod java_azure_blob;
use java_azure_blob::extract_java_azure_blob_resources;
mod python_gcs;
use python_gcs::extract_python_gcs_resources;
mod python_azure_blob;
use python_azure_blob::extract_python_azure_blob_resources;

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
}

fn extract_resource_detections(
    text: &str,
    file_path: &str,
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
        ("aiohttp.request", "python-aiohttp"),
        (".urlopen", "python-urllib"),
        (".getForObject", "java-resttemplate"),
        (".getForEntity", "java-resttemplate"),
        (".postForObject", "java-resttemplate"),
        (".postForEntity", "java-resttemplate"),
        ("URI.create", "java-http-client"),
        (".url", "java-okhttp"),
        (".exchange", "java-http-client"),
        (".uri", "java-webclient"),
    ] {
        out.extend(extract_call_url_detections(text, nodes, callee, strategy));
    }
    out.extend(extract_contextual_http_client_calls(
        text,
        nodes,
        "python-aiohttp",
        text.contains("aiohttp") || text.contains("ClientSession"),
    ));
    out.extend(extract_python_aiohttp_client_relative_calls(text, nodes));
    out.extend(extract_contextual_http_client_calls(
        text,
        nodes,
        "python-httpx-client",
        text.contains("httpx") || text.contains("AsyncClient") || text.contains("Client("),
    ));
    out.extend(extract_python_httpx_client_relative_calls(text, nodes));
    out.extend(extract_python_requests_base_url_session_calls(text, nodes));
    out.extend(extract_python_urllib_calls(text, nodes));
    out.extend(extract_python_boto3_s3_resources(text, nodes));
    out.extend(extract_python_minio_s3_resources(text, nodes));
    out.extend(extract_python_gcs_resources(text, nodes));
    out.extend(extract_python_azure_blob_resources(text, nodes));
    out.extend(extract_java_aws_s3_resources(text, nodes));
    out.extend(extract_java_azure_blob_resources(text, nodes));
    out.extend(extract_contextual_http_client_calls(
        text,
        nodes,
        "java-spring-restclient",
        text.contains("RestClient"),
    ));
    out.extend(extract_spring_restclient_uri_calls(text, nodes));
    out.extend(extract_spring_webclient_uri_calls(text, nodes));
    out.extend(extract_java_feign_client_resources(text, file_path, nodes));
    out.extend(extract_absolute_url_literals(text, nodes));
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

fn extract_python_urllib_calls(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    if !(text.contains("urllib") || text.contains("urlopen")) {
        return Vec::new();
    }
    extract_call_url_detections(text, nodes, "urlopen", "python-urllib")
}

fn extract_python_aiohttp_client_relative_calls(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    if !(text.contains("aiohttp") || text.contains("ClientSession")) {
        return Vec::new();
    }
    let base_urls = aiohttp_base_urls(text);
    if base_urls.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for method in [
        ".get",
        ".post",
        ".put",
        ".patch",
        ".delete",
        ".request",
        ".ws_connect",
    ] {
        for call in find_call_args(text, method) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            for url in relative_urls_from_args(call.args, &base_urls) {
                out.push(ResourceDetection::http(
                    url,
                    node.aka_id.clone(),
                    "python-aiohttp",
                ));
            }
        }
    }
    out
}

fn aiohttp_base_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for callee in ["aiohttp.ClientSession", "ClientSession"] {
        for call in find_call_args(text, callee) {
            out.extend(keyword_url_literals(call.args, "base_url"));
            out.extend(first_arg_url_literals(call.args));
        }
    }
    out.sort();
    out.dedup();
    out.truncate(4);
    out
}

fn extract_python_httpx_client_relative_calls(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    if !(text.contains("httpx") || text.contains("AsyncClient") || text.contains("Client(")) {
        return Vec::new();
    }
    let base_urls = httpx_base_urls(text);
    if base_urls.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for method in [
        ".get", ".post", ".put", ".patch", ".delete", ".request", ".stream",
    ] {
        for call in find_call_args(text, method) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            for url in relative_urls_from_args(call.args, &base_urls) {
                out.push(ResourceDetection::http(
                    url,
                    node.aka_id.clone(),
                    "python-httpx-client",
                ));
            }
        }
    }
    out
}

fn httpx_base_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for callee in ["httpx.Client", "httpx.AsyncClient", "Client", "AsyncClient"] {
        for call in find_call_args(text, callee) {
            out.extend(keyword_url_literals(call.args, "base_url"));
        }
    }
    out.sort();
    out.dedup();
    out.truncate(4);
    out
}

fn extract_python_requests_base_url_session_calls(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    if !(text.contains("BaseUrlSession") || text.contains("base_url")) {
        return Vec::new();
    }
    let base_urls = requests_base_url_session_urls(text);
    if base_urls.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for method in [".get", ".post", ".put", ".patch", ".delete", ".request"] {
        for call in find_call_args(text, method) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            for url in relative_urls_from_args(call.args, &base_urls) {
                out.push(ResourceDetection::http(
                    url,
                    node.aka_id.clone(),
                    "python-requests-base-url-session",
                ));
            }
        }
    }
    out
}

fn requests_base_url_session_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for callee in [
        "BaseUrlSession",
        "sessions.BaseUrlSession",
        "requests_toolbelt.sessions.BaseUrlSession",
    ] {
        for call in find_call_args(text, callee) {
            out.extend(keyword_url_literals(call.args, "base_url"));
            out.extend(first_arg_url_literals(call.args));
        }
    }
    out.sort();
    out.dedup();
    out.truncate(4);
    out
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

fn extract_spring_restclient_uri_calls(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    if !text.contains("RestClient") {
        return Vec::new();
    }
    extract_spring_client_uri_calls(
        text,
        nodes,
        &spring_client_base_urls(text, "RestClient.create"),
        "java-spring-restclient",
        looks_like_restclient_chain,
    )
}

fn extract_spring_webclient_uri_calls(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    if !text.contains("WebClient") {
        return Vec::new();
    }
    extract_spring_client_uri_calls(
        text,
        nodes,
        &spring_client_base_urls(text, "WebClient.create"),
        "java-spring-webclient",
        looks_like_webclient_chain,
    )
}

fn extract_spring_client_uri_calls(
    text: &str,
    nodes: &[&SynthNode],
    base_urls: &[String],
    strategy: &str,
    looks_like_chain: fn(&str, usize) -> bool,
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find(".uri") {
        let start = offset + rel;
        let open = skip_ws(text, start + ".uri".len());
        if text.as_bytes().get(open) != Some(&b'(') || !looks_like_chain(text, start) {
            offset = start + ".uri".len();
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        let Some(node) = node_at_offset(text, nodes, start) else {
            offset = close + 1;
            continue;
        };
        let args = &text[open + 1..close];
        let mut urls = url_literals(args);
        urls.extend(relative_urls_from_args(args, base_urls));
        urls.sort();
        urls.dedup();
        for url in urls {
            out.push(ResourceDetection::http(url, node.aka_id.clone(), strategy));
        }
        offset = close + 1;
    }
    out
}

fn spring_client_base_urls(text: &str, create_callee: &str) -> Vec<String> {
    let mut out = Vec::new();
    for call in find_call_args(text, create_callee) {
        out.extend(url_literals(call.args));
    }
    out.extend(method_call_url_literals(text, ".baseUrl"));
    out.sort();
    out.dedup();
    out.truncate(4);
    out
}

fn method_call_url_literals(text: &str, method: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find(method) {
        let start = offset + rel;
        let open = skip_ws(text, start + method.len());
        if text.as_bytes().get(open) != Some(&b'(') {
            offset = start + method.len();
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        out.extend(url_literals(&text[open + 1..close]));
        offset = close + 1;
    }
    out
}

fn extract_java_feign_client_resources(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    if !file_path.ends_with(".java") {
        return Vec::new();
    }
    let mut client_urls: BTreeMap<String, String> = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        let decorators = decorators_for_node(text, node);
        let Some(base_url) = feign_client_base_url(&decorators) else {
            continue;
        };
        client_urls.insert(node.aka_id.clone(), base_url.clone());
        client_urls.insert(node.qn.clone(), base_url);
    }

    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        let Some(parent) = node.parent_class.as_ref() else {
            continue;
        };
        let Some(base_url) = client_urls.get(parent) else {
            continue;
        };
        let method_path = feign_method_path(text, node);
        out.push(ResourceDetection::http(
            mask_dynamic_url(&join_url_paths(base_url, &method_path)),
            node.aka_id.clone(),
            "java-spring-feign",
        ));
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then_with(|| a.node_id.cmp(&b.node_id)));
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id);
    out
}

fn feign_client_base_url(decorators: &[String]) -> Option<String> {
    for decorator in decorators {
        let Some(args) = annotation_args(decorator, "FeignClient") else {
            continue;
        };
        let Some(url) = keyword_url_literal(args, "url") else {
            continue;
        };
        let path = keyword_relative_path_literal(args, "path");
        return Some(path.map_or(url.clone(), |path| join_url_paths(&url, &path)));
    }
    None
}

fn feign_method_path(text: &str, node: &SynthNode) -> String {
    let decorators = decorators_for_node(text, node);
    for decorator in &decorators {
        if let Some(route) = request_line_path(decorator) {
            return route;
        }
    }
    node.route_path
        .clone()
        .or_else(|| spring_mapping_path(&decorators))
        .unwrap_or_else(|| "/".into())
}

fn decorators_for_node(text: &str, node: &SynthNode) -> Vec<String> {
    let mut decorators = node.decorators.clone();
    decorators.extend(source_annotations_before_node(text, node));
    decorators.sort();
    decorators.dedup();
    decorators
}

fn annotation_args<'a>(annotation: &'a str, expected_simple_name: &str) -> Option<&'a str> {
    let name_end = annotation.find('(')?;
    let name = annotation[..name_end].trim().trim_start_matches('@');
    if name.rsplit('.').next().unwrap_or(name) != expected_simple_name {
        return None;
    }
    let args_start = name_end + 1;
    let args_end = annotation.rfind(')').unwrap_or(annotation.len());
    (args_start <= args_end).then(|| &annotation[args_start..args_end])
}

fn relative_urls_from_args(args: &str, base_urls: &[String]) -> Vec<String> {
    let Some(path) = first_relative_path_literal(args) else {
        return Vec::new();
    };
    base_urls
        .iter()
        .map(|base| join_url_paths(base, &path))
        .collect()
}

fn keyword_url_literals(args: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = format!("{key}=");
    for part in split_top_level_commas(args) {
        let trimmed = part.trim();
        if !trimmed.starts_with(&needle) {
            continue;
        }
        out.extend(url_literals(&trimmed[needle.len()..]));
    }
    out
}

fn keyword_url_literal(args: &str, key: &str) -> Option<String> {
    keyword_literal(args, key).and_then(|literal| normalize_url_literal(&literal))
}

fn keyword_relative_path_literal(args: &str, key: &str) -> Option<String> {
    keyword_literal(args, key).and_then(|literal| {
        (literal.starts_with('/') && !literal.starts_with("//")).then(|| mask_dynamic_url(&literal))
    })
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

fn first_arg_url_literals(args: &str) -> Vec<String> {
    let Some(first) = split_top_level_commas(args).first().copied() else {
        return Vec::new();
    };
    if first.contains('=') {
        return Vec::new();
    }
    url_literals(first)
}

fn first_relative_path_literal(args: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < args.len() {
        let Some((literal, end)) = read_string_literal(args, idx) else {
            idx += args[idx..].chars().next().map(char::len_utf8).unwrap_or(1);
            continue;
        };
        if literal.starts_with('/') && !literal.starts_with("//") {
            return Some(mask_dynamic_url(&literal));
        }
        idx = end;
    }
    None
}

fn join_url_paths(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn looks_like_restclient_chain(text: &str, uri_start: usize) -> bool {
    let start = text[..uri_start].rfind(';').map_or(0, |pos| pos + 1);
    let chain = &text[start..uri_start];
    chain.contains("RestClient")
        || chain.contains("restClient")
        || chain.contains(".get()")
        || chain.contains(".post()")
        || chain.contains(".put()")
        || chain.contains(".patch()")
        || chain.contains(".delete()")
        || chain.contains(".method(")
}

fn looks_like_webclient_chain(text: &str, uri_start: usize) -> bool {
    let start = text[..uri_start].rfind(';').map_or(0, |pos| pos + 1);
    let chain = &text[start..uri_start];
    chain.contains("WebClient")
        || chain.contains("webClient")
        || chain.contains(".get()")
        || chain.contains(".post()")
        || chain.contains(".put()")
        || chain.contains(".patch()")
        || chain.contains(".delete()")
        || chain.contains(".method(")
}

fn extract_contextual_http_client_calls(
    text: &str,
    nodes: &[&SynthNode],
    strategy: &str,
    enabled: bool,
) -> Vec<ResourceDetection> {
    if !enabled {
        return Vec::new();
    }
    let mut out = Vec::new();
    for method in [
        ".get", ".post", ".put", ".patch", ".delete", ".request", ".stream",
    ] {
        out.extend(extract_call_url_detections(text, nodes, method, strategy));
    }
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
            out.push(ResourceDetection::http(url, node.aka_id.clone(), strategy));
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
                out.push(ResourceDetection::http(
                    url,
                    node.aka_id.clone(),
                    "literal-http-url",
                ));
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
