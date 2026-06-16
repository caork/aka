use super::{mask_dynamic_url, read_python_string_literal, ResourceDetection};
use crate::engine::{
    find_call_args, find_matching_paren, node_at_offset, skip_ws, split_top_level_commas, SynthNode,
};

pub(super) fn extract_python_gcs_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    if !(text.contains("google.cloud") || text.contains("storage") || text.contains(".blob(")) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for call in find_dot_call_args(text, ".blob") {
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        let Some(object) =
            python_string_arg(call.args, 0).or_else(|| python_keyword_arg(call.args, "blob_name"))
        else {
            continue;
        };
        let Some(bucket) = gcs_bucket_before_blob(text, call.start) else {
            continue;
        };
        let strategy = gcs_strategy_after_blob(text, call.start);
        out.push(ResourceDetection::gcs(
            gcs_url(&bucket, Some(&object)),
            node.aka_id.clone(),
            strategy,
        ));
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then_with(|| a.node_id.cmp(&b.node_id)));
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
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

fn gcs_bucket_before_blob(text: &str, blob_start: usize) -> Option<String> {
    let line_start = text[..blob_start].rfind('\n').map_or(0, |idx| idx + 1);
    let prefix = &text[line_start..blob_start];
    if let Some(bucket) = bucket_literal_from_get_bucket(prefix) {
        return Some(bucket);
    }
    let window_start = blob_start.saturating_sub(1200);
    bucket_literal_from_get_bucket(&text[window_start..blob_start])
}

fn bucket_literal_from_get_bucket(text: &str) -> Option<String> {
    for callee in [".bucket", ".get_bucket"] {
        for call in find_call_args(text, callee) {
            if let Some(bucket) = python_string_arg(call.args, 0) {
                return Some(bucket);
            }
        }
    }
    None
}

fn gcs_strategy_after_blob(text: &str, blob_start: usize) -> &'static str {
    let end = text[blob_start..]
        .find('\n')
        .map(|rel| blob_start + rel)
        .unwrap_or(text.len());
    let suffix = &text[blob_start..end];
    if suffix.contains("upload_from") {
        "python-gcs-upload"
    } else if suffix.contains("download_") || suffix.contains(".open(") {
        "python-gcs-download"
    } else if suffix.contains(".delete(") {
        "python-gcs-delete"
    } else if suffix.contains(".exists(") {
        "python-gcs-head"
    } else {
        "python-gcs-blob"
    }
}

fn python_keyword_arg(args: &str, key: &str) -> Option<String> {
    for part in split_top_level_commas(args) {
        let (found, value) = part.split_once('=')?;
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
    read_python_string_literal(expr, 0).map(|(literal, _)| mask_dynamic_url(&literal))
}

fn gcs_url(bucket: &str, object: Option<&str>) -> String {
    match object.filter(|value| !value.is_empty()) {
        Some(object) => format!(
            "gs://{}/{}",
            bucket.trim_matches('/'),
            object.trim_start_matches('/')
        ),
        None => format!("gs://{}", bucket.trim_matches('/')),
    }
}
