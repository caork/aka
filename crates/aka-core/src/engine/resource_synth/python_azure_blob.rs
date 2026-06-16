use super::{
    mask_dynamic_url, read_python_string_literal, resource_strategy_rank, ResourceDetection,
};
use crate::engine::{
    find_call_args, find_matching_paren, node_at_offset, skip_ws, split_top_level_commas, SynthNode,
};

pub(super) fn extract_python_azure_blob_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    if !(text.contains("azure.storage.blob")
        || text.contains("BlobClient")
        || text.contains("get_blob_client"))
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.extend(extract_from_get_blob_client(text, nodes));
    out.extend(extract_from_blob_client_factory(text, nodes));
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

fn extract_from_get_blob_client(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for call in find_dot_call_args(text, ".get_blob_client") {
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        let Some(blob) =
            python_string_arg(call.args, 0).or_else(|| python_keyword_arg(call.args, "blob"))
        else {
            continue;
        };
        let Some(container) = container_before_blob_client(text, call.start) else {
            continue;
        };
        let strategy = azure_strategy_for_blob_client(text, node, call.start);
        out.push(ResourceDetection::azure_blob(
            azure_blob_url(&container, Some(&blob)),
            node.aka_id.clone(),
            strategy,
        ));
    }
    out
}

fn extract_from_blob_client_factory(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for callee in [
        "BlobClient.from_connection_string",
        "BlobClient.from_blob_url",
        "BlobClient",
    ] {
        for call in find_call_args(text, callee) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            let Some(container) = python_keyword_arg(call.args, "container_name")
                .or_else(|| python_keyword_arg(call.args, "container"))
            else {
                continue;
            };
            let Some(blob) = python_keyword_arg(call.args, "blob_name")
                .or_else(|| python_keyword_arg(call.args, "blob"))
            else {
                continue;
            };
            let strategy = azure_strategy_for_blob_client(text, node, call.start);
            out.push(ResourceDetection::azure_blob(
                azure_blob_url(&container, Some(&blob)),
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out
}

struct DotCallArgs<'a> {
    start: usize,
    args: &'a str,
}

fn find_dot_call_args<'a>(text: &'a str, callee: &str) -> Vec<DotCallArgs<'a>> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find(callee) {
        let start = offset + rel;
        let open = skip_ws(text, start + callee.len());
        if text.as_bytes().get(open) != Some(&b'(') {
            offset = start + callee.len();
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        out.push(DotCallArgs {
            start,
            args: &text[open + 1..close],
        });
        offset = close + 1;
    }
    out
}

fn container_before_blob_client(text: &str, blob_start: usize) -> Option<String> {
    let line_start = text[..blob_start].rfind('\n').map_or(0, |idx| idx + 1);
    let prefix = &text[line_start..blob_start];
    if let Some(container) = container_literal_from_get_container(prefix) {
        return Some(container);
    }
    let window_start = blob_start.saturating_sub(1200);
    container_literal_from_get_container(&text[window_start..blob_start])
}

fn container_literal_from_get_container(text: &str) -> Option<String> {
    for callee in [".get_container_client", "get_container_client"] {
        for call in find_call_args(text, callee) {
            if let Some(container) = python_string_arg(call.args, 0)
                .or_else(|| python_keyword_arg(call.args, "container"))
            {
                return Some(container);
            }
        }
    }
    None
}

fn azure_strategy_for_blob_client(text: &str, node: &SynthNode, call_start: usize) -> &'static str {
    let inline = azure_strategy_after_blob_client(text, call_start);
    if inline != "python-azure-blob" {
        return inline;
    }
    let Some(var_name) = assigned_var_before_call(text, call_start) else {
        return inline;
    };
    let (body_start, body_end) = source_window_for_node(text, node);
    if call_start < body_start || call_start > body_end {
        return inline;
    }
    azure_strategy_for_blob_var(&text[body_start..body_end], &var_name).unwrap_or(inline)
}

fn azure_strategy_after_blob_client(text: &str, call_start: usize) -> &'static str {
    let end = text[call_start..]
        .find('\n')
        .map(|rel| call_start + rel)
        .unwrap_or(text.len());
    let suffix = &text[call_start..end];
    if suffix.contains("upload_blob") {
        "python-azure-blob-upload"
    } else if suffix.contains("download_blob") {
        "python-azure-blob-download"
    } else if suffix.contains("delete_blob") {
        "python-azure-blob-delete"
    } else if suffix.contains("exists(") || suffix.contains("get_blob_properties") {
        "python-azure-blob-head"
    } else {
        "python-azure-blob"
    }
}

fn assigned_var_before_call(text: &str, call_start: usize) -> Option<String> {
    let line_start = text[..call_start].rfind('\n').map_or(0, |idx| idx + 1);
    let line_prefix = &text[line_start..call_start];
    let (left, _) = line_prefix.split_once('=')?;
    let var = left.trim();
    is_python_ident(var).then(|| var.to_string())
}

fn azure_strategy_for_blob_var(text: &str, var_name: &str) -> Option<&'static str> {
    for (method, strategy) in [
        ("upload_blob", "python-azure-blob-upload"),
        ("download_blob", "python-azure-blob-download"),
        ("delete_blob", "python-azure-blob-delete"),
        ("exists", "python-azure-blob-head"),
        ("get_blob_properties", "python-azure-blob-head"),
    ] {
        let needle = format!("{var_name}.{method}");
        if text.contains(&needle) {
            return Some(strategy);
        }
    }
    None
}

fn python_keyword_arg(args: &str, key: &str) -> Option<String> {
    for part in split_top_level_commas(args) {
        let Some((found, value)) = part.split_once('=') else {
            continue;
        };
        if found.trim() != key {
            continue;
        }
        return python_literal(value.trim());
    }
    None
}

fn python_string_arg(args: &str, index: usize) -> Option<String> {
    split_top_level_commas(args)
        .into_iter()
        .filter(|arg| !arg.contains('='))
        .nth(index)
        .and_then(|arg| python_literal(arg.trim()))
}

fn python_literal(expr: &str) -> Option<String> {
    read_python_string_literal(expr.trim_start(), 0).map(|(literal, _)| mask_dynamic_url(&literal))
}

fn azure_blob_url(container: &str, blob: Option<&str>) -> String {
    match blob.filter(|value| !value.is_empty()) {
        Some(blob) => format!(
            "azblob://{}/{}",
            container.trim_matches('/'),
            blob.trim_start_matches('/')
        ),
        None => format!("azblob://{}", container.trim_matches('/')),
    }
}

fn source_window_for_node(text: &str, node: &SynthNode) -> (usize, usize) {
    let start_line = node.start_line_key().max(1);
    let end_line = node.end_line_key().max(start_line);
    let mut line = 1i64;
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
    (start.min(text.len()), end.min(text.len()))
}

fn is_python_ident(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
