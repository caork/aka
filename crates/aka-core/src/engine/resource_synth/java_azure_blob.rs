use super::{mask_dynamic_url, read_string_literal, resource_strategy_rank, ResourceDetection};
use crate::engine::{
    find_matching_paren, node_at_offset, skip_ws, split_top_level_commas, SynthNode,
};

pub(super) fn extract_java_azure_blob_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    if !(text.contains("BlobClient")
        || text.contains("BlobContainerClient")
        || text.contains("com.azure.storage.blob"))
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.extend(extract_from_get_blob_client(text, nodes));
    out.extend(extract_from_blob_client_builder(text, nodes));
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
    for call in find_dot_call_args(text, ".getBlobClient") {
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        let Some(blob) = java_positional_string_expr(call.args, 0) else {
            continue;
        };
        let Some(container) = container_before_blob_client(text, call.start) else {
            continue;
        };
        let strategy = java_azure_strategy_for_blob_client(text, node, call.start);
        out.push(ResourceDetection::azure_blob(
            azure_blob_url(&container, Some(&blob)),
            node.aka_id.clone(),
            strategy,
        ));
    }
    out
}

fn extract_from_blob_client_builder(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find("BlobClientBuilder") {
        let start = offset + rel;
        let Some(node) = node_at_offset(text, nodes, start) else {
            offset = start + "BlobClientBuilder".len();
            continue;
        };
        let end = builder_statement_end(text, start);
        let window = &text[start..end];
        let Some(container) = java_method_string_expr_arg(window, ".containerName") else {
            offset = end.saturating_add(1);
            continue;
        };
        let Some(blob) = java_method_string_expr_arg(window, ".blobName") else {
            offset = end.saturating_add(1);
            continue;
        };
        let strategy = java_azure_strategy_for_blob_client(text, node, start);
        out.push(ResourceDetection::azure_blob(
            azure_blob_url(&container, Some(&blob)),
            node.aka_id.clone(),
            strategy,
        ));
        offset = end.saturating_add(1);
    }
    out
}

fn container_before_blob_client(text: &str, blob_start: usize) -> Option<String> {
    let line_start = text[..blob_start].rfind('\n').map_or(0, |idx| idx + 1);
    let prefix = &text[line_start..blob_start];
    if let Some(container) = container_literal_from_get_container(prefix) {
        return Some(container);
    }
    let window_start = blob_start.saturating_sub(1600);
    container_literal_from_get_container(&text[window_start..blob_start])
}

fn container_literal_from_get_container(text: &str) -> Option<String> {
    for call in find_dot_call_args(text, ".getBlobContainerClient") {
        if let Some(container) = java_positional_string_expr(call.args, 0) {
            return Some(container);
        }
    }
    None
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

fn java_azure_strategy_for_blob_client(
    text: &str,
    node: &SynthNode,
    call_start: usize,
) -> &'static str {
    let inline = java_azure_strategy_after_blob_client(text, call_start);
    if inline != "java-azure-blob" {
        return inline;
    }
    let Some(var_name) = assigned_var_before_call(text, call_start) else {
        return inline;
    };
    let (body_start, body_end) = source_window_for_node(text, node);
    if call_start < body_start || call_start > body_end {
        return inline;
    }
    java_azure_strategy_for_blob_var(&text[body_start..body_end], &var_name).unwrap_or(inline)
}

fn java_azure_strategy_after_blob_client(text: &str, call_start: usize) -> &'static str {
    let end = text[call_start..]
        .find(';')
        .map(|rel| call_start + rel)
        .unwrap_or(text.len());
    let suffix = &text[call_start..end];
    if suffix.contains(".upload") {
        "java-azure-blob-upload"
    } else if suffix.contains(".download") || suffix.contains(".openInputStream") {
        "java-azure-blob-download"
    } else if suffix.contains(".delete") {
        "java-azure-blob-delete"
    } else if suffix.contains(".exists") || suffix.contains(".getProperties") {
        "java-azure-blob-head"
    } else {
        "java-azure-blob"
    }
}

fn assigned_var_before_call(text: &str, call_start: usize) -> Option<String> {
    let line_start = text[..call_start].rfind('\n').map_or(0, |idx| idx + 1);
    let line_prefix = &text[line_start..call_start];
    let (left, _) = line_prefix.split_once('=')?;
    let var = left.split_whitespace().last()?.trim();
    is_java_ident(var).then(|| var.to_string())
}

fn java_azure_strategy_for_blob_var(text: &str, var_name: &str) -> Option<&'static str> {
    for (method, strategy) in [
        ("upload", "java-azure-blob-upload"),
        ("download", "java-azure-blob-download"),
        ("openInputStream", "java-azure-blob-download"),
        ("delete", "java-azure-blob-delete"),
        ("exists", "java-azure-blob-head"),
        ("getProperties", "java-azure-blob-head"),
    ] {
        let needle = format!("{var_name}.{method}");
        if text.contains(&needle) {
            return Some(strategy);
        }
    }
    None
}

fn builder_statement_end(text: &str, start: usize) -> usize {
    text[start..]
        .find(';')
        .map(|rel| start + rel)
        .unwrap_or(text.len())
}

fn java_method_string_expr_arg(text: &str, method: &str) -> Option<String> {
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find(method) {
        let start = offset + rel;
        let open = skip_ws(text, start + method.len());
        if text.as_bytes().get(open) != Some(&b'(') {
            offset = start + method.len();
            continue;
        }
        let close = find_matching_paren(text, open)?;
        if let Some(value) = java_string_expr_literal(&text[open + 1..close]) {
            return Some(value);
        }
        offset = close + 1;
    }
    None
}

fn java_positional_string_expr(args: &str, index: usize) -> Option<String> {
    split_top_level_commas(args)
        .into_iter()
        .nth(index)
        .and_then(java_string_expr_literal)
}

fn java_string_expr_literal(expr: &str) -> Option<String> {
    let mut out = String::new();
    let mut idx = 0usize;
    let mut last_end = 0usize;
    let mut saw_literal = false;
    while idx < expr.len() {
        let Some((literal, end)) = read_string_literal(expr, idx) else {
            idx += expr[idx..].chars().next().map(char::len_utf8).unwrap_or(1);
            continue;
        };
        let gap = if saw_literal {
            &expr[last_end..idx]
        } else {
            &expr[..idx]
        };
        if java_expr_gap_has_dynamic(gap) {
            out.push_str("{param}");
        }
        out.push_str(&literal);
        saw_literal = true;
        last_end = end;
        idx = end;
    }
    if saw_literal && java_expr_gap_has_dynamic(&expr[last_end..]) {
        out.push_str("{param}");
    }
    saw_literal.then(|| mask_dynamic_url(&out))
}

fn java_expr_gap_has_dynamic(gap: &str) -> bool {
    gap.chars()
        .any(|ch| !ch.is_ascii_whitespace() && ch != '+' && ch != '(' && ch != ')')
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

fn is_java_ident(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}
