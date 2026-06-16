use super::{read_string_literal, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, skip_ws, split_top_level_commas, SynthNode};

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
    for call in find_call_args(text, "Search") {
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        for index in python_index_values(call.args) {
            out.push(ResourceDetection::search_index(
                index,
                node.aka_id.clone(),
                "python-search-index-dsl-search",
            ));
        }
    }
    out.extend(extract_python_dsl_index_objects(text, nodes));
    out.extend(extract_python_document_indices(text, nodes));
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
    for call in find_call_args(text, "SearchRequest.of") {
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        for index in java_index_values(call.args) {
            out.push(ResourceDetection::search_index(
                index,
                node.aka_id.clone(),
                "java-search-index-request",
            ));
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

fn extract_python_dsl_index_objects(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, "Index") {
        let Some(index) = split_top_level_commas(call.args)
            .into_iter()
            .find_map(first_literal_if_index)
        else {
            continue;
        };
        let Some(var_name) = python_assignment_lhs(text, call.start) else {
            continue;
        };
        for access in python_index_object_access_offsets(text, &var_name) {
            let Some(node) = node_at_offset(text, nodes, access) else {
                continue;
            };
            out.push(ResourceDetection::search_index(
                index.clone(),
                node.aka_id.clone(),
                "python-search-index-dsl-index",
            ));
        }
    }
    out
}

fn extract_python_document_indices(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| node.label == "Class")
        .filter(|node| node_contains_document_index(text, node))
    {
        let Some(body) = node_text(text, node) else {
            continue;
        };
        let Some(index) = python_document_index_name(body) else {
            continue;
        };
        out.push(ResourceDetection::search_index(
            index,
            node.aka_id.clone(),
            "python-search-index-dsl-document",
        ));
    }
    out
}

fn first_literal_if_index(arg: &str) -> Option<String> {
    let trimmed = arg.trim();
    let start = trimmed.find(['"', '\''])?;
    let (literal, _) = read_string_literal(trimmed, start)?;
    is_index_literal(&literal).then_some(literal)
}

fn python_assignment_lhs(text: &str, offset: usize) -> Option<String> {
    let line_start = text[..offset].rfind('\n').map_or(0, |idx| idx + 1);
    let prefix = &text[line_start..offset];
    let eq = prefix.rfind('=')?;
    if prefix[eq + 1..].trim().is_empty() {
        let lhs = prefix[..eq].trim();
        if is_python_identifier(lhs) {
            return Some(lhs.to_string());
        }
    }
    None
}

fn python_index_object_access_offsets(text: &str, var_name: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let access_prefix = format!("{var_name}.");
    while let Some(rel) = text[offset..].find(&access_prefix) {
        let start = offset + rel;
        let method_start = start + access_prefix.len();
        let method_end = text[method_start..]
            .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .map_or(text.len(), |rel| method_start + rel);
        let method = &text[method_start..method_end];
        let open = skip_ws(text, method_end);
        if matches!(
            method,
            "create" | "delete" | "exists" | "put_mapping" | "refresh" | "save"
        ) && text.as_bytes().get(open) == Some(&b'(')
        {
            out.push(start);
        }
        offset = method_end.max(start + access_prefix.len());
    }
    out.sort();
    out.dedup();
    out
}

fn is_python_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn node_contains_document_index(text: &str, node: &SynthNode) -> bool {
    let Some(body) = node_text(text, node) else {
        return false;
    };
    body.contains("Document") && body.contains("class Index") && body.contains("name")
}

fn node_text<'a>(text: &'a str, node: &SynthNode) -> Option<&'a str> {
    let start_line = node.start_line_key().max(1) as usize;
    let end_line = node.end_line_key().max(start_line as i64) as usize;
    if start_line > end_line {
        return None;
    }
    let mut line = 1usize;
    let mut start = 0usize;
    let mut end = text.len();
    for (idx, ch) in text.char_indices() {
        if line == start_line {
            start = idx;
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    line = 1;
    for (idx, ch) in text.char_indices() {
        if line > end_line {
            end = idx;
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    Some(&text[start..end])
}

fn python_document_index_name(class_body: &str) -> Option<String> {
    let idx = class_body.find("class Index")?;
    let rest = &class_body[idx..];
    let name_pos = rest.find("name")?;
    let after_name = &rest[name_pos + "name".len()..];
    let eq_pos = after_name.find('=')?;
    let value = &after_name[eq_pos + 1..];
    let literal = first_literal_if_index(value)?;
    Some(literal)
}
