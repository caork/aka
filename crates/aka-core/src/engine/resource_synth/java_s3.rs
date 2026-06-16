use super::{mask_dynamic_url, read_string_literal, s3_url, ResourceDetection};
use crate::engine::{
    find_call_args, find_matching_paren, node_at_offset, skip_ws, split_top_level_commas, SynthNode,
};

pub(super) fn extract_java_aws_s3_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    if !(text.contains("S3")
        || text.contains("AmazonS3")
        || text.contains("PutObject")
        || text.contains("GetObject"))
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (callee, strategy) in [
        (".putObject", "java-aws-s3-put-object"),
        (".getObject", "java-aws-s3-get-object"),
        (".deleteObject", "java-aws-s3-delete-object"),
        (".headObject", "java-aws-s3-head-object"),
        (".getObjectMetadata", "java-aws-s3-head-object"),
        (".doesObjectExist", "java-aws-s3-head-object"),
    ] {
        for call in find_call_args(text, callee) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            let Some((bucket, key)) = java_s3_bucket_key_from_args(call.args)
                .or_else(|| java_s3_bucket_key_near_call(text, node, call.start))
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

fn java_s3_bucket_key_near_call(
    text: &str,
    node: &SynthNode,
    call_start: usize,
) -> Option<(String, Option<String>)> {
    let (body_start, body_end) = source_window_for_node(text, node);
    if call_start < body_start || call_start > body_end {
        return None;
    }
    java_builder_bucket_keys(&text[body_start..body_end])
        .into_iter()
        .next()
}

fn java_s3_bucket_key_from_args(args: &str) -> Option<(String, Option<String>)> {
    java_builder_bucket_key(args)
        .or_else(|| java_request_ctor_bucket_key(args))
        .or_else(|| {
            let bucket = java_positional_string_expr(args, 0)?;
            let key = java_positional_string_expr(args, 1);
            Some((bucket, key))
        })
}

fn java_builder_bucket_keys(text: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for marker in [
        "PutObjectRequest.builder",
        "GetObjectRequest.builder",
        "DeleteObjectRequest.builder",
        "HeadObjectRequest.builder",
    ] {
        let mut offset = 0usize;
        while let Some(rel) = text[offset..].find(marker) {
            let start = offset + rel;
            let end = text[start..]
                .find(';')
                .map(|rel| start + rel)
                .unwrap_or(text.len());
            if let Some(candidate) = java_builder_bucket_key(&text[start..end]) {
                out.push(candidate);
            }
            offset = end.saturating_add(1);
        }
    }
    out
}

fn java_builder_bucket_key(text: &str) -> Option<(String, Option<String>)> {
    let bucket = java_method_string_expr_arg(text, ".bucket")?;
    let key = java_method_string_expr_arg(text, ".key");
    Some((bucket, key))
}

fn java_request_ctor_bucket_key(text: &str) -> Option<(String, Option<String>)> {
    for ctor in [
        "PutObjectRequest",
        "GetObjectRequest",
        "DeleteObjectRequest",
        "GetObjectMetadataRequest",
        "HeadObjectRequest",
    ] {
        let Some(args) = java_constructor_args(text, ctor) else {
            continue;
        };
        let Some(bucket) = java_positional_string_expr(args, 0) else {
            continue;
        };
        return Some((bucket, java_positional_string_expr(args, 1)));
    }
    None
}

fn java_constructor_args<'a>(text: &'a str, ctor: &str) -> Option<&'a str> {
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find(ctor) {
        let start = offset + rel;
        if !java_identifier_boundary_ok(text, start, ctor) {
            offset = start + ctor.len();
            continue;
        }
        let open = skip_ws(text, start + ctor.len());
        if text.as_bytes().get(open) != Some(&b'(') {
            offset = start + ctor.len();
            continue;
        }
        let close = find_matching_paren(text, open)?;
        return Some(&text[open + 1..close]);
    }
    None
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
        .filter(|arg| !arg.contains('='))
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

fn java_identifier_boundary_ok(text: &str, start: usize, ident: &str) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|idx| text.as_bytes().get(idx))
        .copied()
        .map(char::from);
    let after = text
        .as_bytes()
        .get(start + ident.len())
        .copied()
        .map(char::from);
    before.is_none_or(|ch| !is_java_ident_continue(ch))
        && after.is_none_or(|ch| !is_java_ident_continue(ch))
}

fn is_java_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$')
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
