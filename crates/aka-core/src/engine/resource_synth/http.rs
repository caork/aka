use std::collections::BTreeMap;

use super::{mask_dynamic_url, read_python_string_literal, read_string_literal, ResourceDetection};
use crate::engine::{
    find_call_args, find_matching_paren, node_at_offset, request_line_path, skip_ws,
    source_annotations_before_node, split_top_level_commas, spring_mapping_path, SynthNode,
};

pub(super) fn extract_http_resources(
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
    out
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
