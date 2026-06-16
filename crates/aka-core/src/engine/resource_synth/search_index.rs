use super::{read_string_literal, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, split_top_level_commas, SynthNode};

pub(super) fn extract_search_index_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_search_context(text) {
        out.extend(extract_python_search_indices(text, nodes));
    }
    if has_java_search_context(text) {
        out.extend(extract_java_search_indices(text, nodes));
    }
    out
}

fn has_python_search_context(text: &str) -> bool {
    text.contains("elasticsearch")
        || text.contains("opensearchpy")
        || text.contains("OpenSearch")
        || text.contains("Elasticsearch")
}

fn extract_python_search_indices(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in [
        (".search", "python-search-index-search"),
        (".index", "python-search-index-index"),
        (".get", "python-search-index-get"),
        (".delete", "python-search-index-delete"),
        (".update", "python-search-index-update"),
        (".count", "python-search-index-count"),
    ] {
        for call in find_call_args(text, callee) {
            if !python_search_receiver(text, call.start) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            for index in python_index_values(call.args) {
                out.push(ResourceDetection::search_index(
                    index,
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    for call in find_call_args(text, "helpers.bulk") {
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        for index in string_values_for_keys(call.args, &["_index", "index"]) {
            out.push(ResourceDetection::search_index(
                index,
                node.aka_id.clone(),
                "python-search-index-bulk",
            ));
        }
    }
    out
}

fn python_search_receiver(text: &str, dot_start: usize) -> bool {
    let Some(receiver) = receiver_before_dot(text, dot_start) else {
        return false;
    };
    let tail = receiver
        .rsplit('.')
        .next()
        .unwrap_or(receiver)
        .to_ascii_lowercase();
    matches!(tail.as_str(), "es" | "client" | "search" | "opensearch")
        || tail.contains("elastic")
        || tail.contains("opensearch")
}

fn python_index_values(args: &str) -> Vec<String> {
    string_values_for_keys(args, &["index", "_index"])
}

fn has_java_search_context(text: &str) -> bool {
    text.contains("ElasticsearchClient")
        || text.contains("OpenSearchClient")
        || text.contains("RestHighLevelClient")
        || text.contains("co.elastic.clients.elasticsearch")
        || text.contains("org.elasticsearch")
        || text.contains("org.opensearch")
}

fn extract_java_search_indices(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in [
        (".search", "java-search-index-search"),
        (".index", "java-search-index-index"),
        (".get", "java-search-index-get"),
        (".delete", "java-search-index-delete"),
        (".update", "java-search-index-update"),
        (".bulk", "java-search-index-bulk"),
    ] {
        for call in find_call_args(text, callee) {
            if !java_search_receiver(text, call.start) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            for index in java_index_values(call.args) {
                out.push(ResourceDetection::search_index(
                    index,
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    out
}

fn java_search_receiver(text: &str, dot_start: usize) -> bool {
    let Some(receiver) = receiver_before_dot(text, dot_start) else {
        return false;
    };
    let tail = receiver
        .rsplit('.')
        .next()
        .unwrap_or(receiver)
        .to_ascii_lowercase();
    matches!(
        tail.as_str(),
        "client" | "es" | "elasticsearchclient" | "opensearchclient"
    ) || tail.contains("elastic")
        || tail.contains("opensearch")
}

fn java_index_values(args: &str) -> Vec<String> {
    let mut out = string_values_for_keys(args, &["index", "_index"]);
    for part in split_top_level_commas(args) {
        let trimmed = part.trim();
        if !trimmed.starts_with('"') && !trimmed.starts_with('\'') {
            continue;
        }
        if let Some((literal, _)) = read_string_literal(trimmed, 0) {
            if is_index_literal(&literal) {
                out.push(literal);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn string_values_for_keys(args: &str, keys: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for key in keys {
        out.extend(keyword_string_values(args, key));
    }
    out.sort();
    out.dedup();
    out
}

fn keyword_string_values(args: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in split_top_level_commas(args) {
        let trimmed = part.trim();
        if let Some((found, value)) = trimmed.split_once('=') {
            if found.trim() == key {
                out.extend(first_string_values(value));
            }
        }
        let java_prefix = format!(".{key}(");
        if let Some(pos) = trimmed.find(&java_prefix) {
            let value = &trimmed[pos + java_prefix.len()..];
            out.extend(first_string_values(value));
        }
        let json_key = format!("\"{key}\"");
        if let Some(pos) = trimmed.find(&json_key) {
            let rest = &trimmed[pos + json_key.len()..];
            out.extend(first_string_values(rest));
        }
    }
    out.retain(|value| is_index_literal(value));
    out.sort();
    out.dedup();
    out
}

fn first_string_values(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some((literal, end)) = read_string_literal(text, idx) else {
            idx += text[idx..].chars().next().map(char::len_utf8).unwrap_or(1);
            continue;
        };
        out.push(literal);
        idx = end;
    }
    out
}

fn receiver_before_dot(text: &str, dot_start: usize) -> Option<&str> {
    if text.as_bytes().get(dot_start) != Some(&b'.') {
        return None;
    }
    let mut start = dot_start;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if ch == '.' || ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let receiver = text[start..dot_start].trim_matches('.');
    (!receiver.is_empty()).then_some(receiver)
}

fn is_index_literal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 180
        && !value.contains("://")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '*'))
}
