use std::collections::BTreeSet;

use super::{
    find_matching_paren, is_ident_continue,
    route_shape_java::{
        is_java_common_constructed_type, is_java_ident, is_object_key, read_java_identifier,
    },
    skip_ws,
};

pub(super) fn java_builder_response_types(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for window in java_response_expression_windows(text) {
        for name in java_builder_types_in_window(window) {
            out.insert(name);
        }
    }
    out.into_iter().collect()
}

pub(super) fn java_builder_response_keys(text: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for window in java_response_expression_windows(text) {
        for key in java_builder_keys_in_window(window) {
            keys.insert(key);
        }
    }
    keys.into_iter().collect()
}

fn java_builder_types_in_window(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(".builder(") {
        let builder_pos = offset + pos;
        let Some((name, _)) = java_qualified_name_before(text, builder_pos) else {
            offset = builder_pos + ".builder(".len();
            continue;
        };
        let simple = name.rsplit('.').next().unwrap_or(&name);
        if is_java_ident(simple) && !is_java_common_constructed_type(simple) {
            out.insert(simple.to_string());
        }
        offset = builder_pos + ".builder(".len();
    }
    out.into_iter().collect()
}

fn java_builder_keys_in_window(text: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(".builder(") {
        let builder_pos = offset + pos;
        let Some(chain_end) = text[builder_pos..]
            .find(".build(")
            .map(|rel| builder_pos + rel)
        else {
            offset = builder_pos + ".builder(".len();
            continue;
        };
        for method in java_builder_chain_methods(&text[builder_pos..chain_end]) {
            if is_object_key(&method) && !is_java_common_builder_method(&method) {
                keys.insert(method);
            }
        }
        offset = chain_end + ".build(".len();
    }
    keys.into_iter().collect()
}

fn java_response_expression_windows(text: &str) -> Vec<&str> {
    let mut windows = Vec::new();
    for marker in ["return ", "ResponseEntity.", ".body("] {
        let mut offset = 0usize;
        while let Some(rel) = text[offset..].find(marker) {
            let start = offset + rel;
            let end = java_statement_window_end(text, start);
            windows.push(&text[start..end]);
            offset = start + marker.len();
        }
    }
    windows
}

fn java_statement_window_end(text: &str, start: usize) -> usize {
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut brace = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    for (rel, byte) in text.as_bytes()[start..].iter().copied().enumerate() {
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if byte == b'\\' {
                escape = true;
            } else if byte == q {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'(' => paren += 1,
            b')' => paren -= 1,
            b'[' => bracket += 1,
            b']' => bracket -= 1,
            b'{' => brace += 1,
            b'}' => {
                if paren <= 0 && bracket <= 0 && brace <= 0 {
                    return start + rel;
                }
                brace -= 1;
            }
            b';' if paren <= 0 && bracket <= 0 && brace <= 0 => return start + rel + 1,
            _ => {}
        }
    }
    text.len()
}

fn java_qualified_name_before(text: &str, pos: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let mut start = pos;
    while start > 0 {
        let ch = bytes[start - 1] as char;
        if ch == '.' || is_ident_continue(ch) {
            start -= 1;
        } else {
            break;
        }
    }
    let name = text[start..pos].trim_matches('.');
    (!name.is_empty()
        && name
            .split('.')
            .all(|segment| !segment.is_empty() && is_java_ident(segment)))
    .then(|| (name.to_string(), start))
}

fn java_builder_chain_methods(chain: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = chain.as_bytes();
    let mut idx = 0usize;
    while idx + 1 < chain.len() {
        if bytes[idx] != b'.' {
            idx += 1;
            continue;
        }
        let name_start = idx + 1;
        let Some((name, name_end)) = read_java_identifier(chain, name_start) else {
            idx += 1;
            continue;
        };
        let open = skip_ws(chain, name_end);
        if bytes.get(open) == Some(&b'(') {
            out.push(name.to_string());
            if let Some(close) = find_matching_paren(chain, open) {
                idx = close + 1;
                continue;
            }
        }
        idx = name_end;
    }
    out
}

fn is_java_common_builder_method(name: &str) -> bool {
    matches!(name, "builder" | "build" | "toBuilder")
}
