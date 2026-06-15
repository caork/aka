use std::collections::BTreeSet;

use super::{clamp_char_boundary, is_ident_continue, is_ident_start, read_string_literal, skip_ws};

pub(super) fn fetch_literal_windows(text: &str) -> Vec<(usize, &str)> {
    let mut windows = Vec::new();
    for marker in [
        "fetch(",
        "axios.",
        ".get(",
        ".post(",
        ".put(",
        ".patch(",
        ".delete(",
        ".request(",
        ".stream(",
        "http.",
        "client.",
        "requests.",
        "httpx.",
        "aiohttp.",
        "AsyncClient",
        "ClientSession",
        "RestTemplate",
        "restTemplate.",
        "getForObject(",
        "getForEntity(",
        "postForObject(",
        "postForEntity(",
        "patchForObject(",
        "exchange(",
        "WebClient",
        "webClient.",
        ".uri(",
        ".url(",
        "HttpRequest.newBuilder",
        "Request.Builder",
    ] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(marker) {
            let start = offset + pos;
            let end = clamp_char_boundary(text, start + 600);
            windows.push((start, &text[start..end]));
            offset = start + marker.len();
        }
    }
    windows
}

pub(super) fn extract_accessed_keys_near_route(text: &str, route: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for idx in route_occurrences(text, route) {
        let end = clamp_char_boundary(text, idx + 2000);
        let window = &text[idx..end];
        for key in dotted_property_names(window) {
            if !is_common_property(&key) {
                keys.insert(key);
            }
        }
        keys.extend(java_getter_property_names(window));
        keys.extend(bracket_string_property_names(window));
    }
    keys.into_iter().take(16).collect()
}

pub(super) fn route_occurrences(text: &str, route: &str) -> Vec<usize> {
    let mut out = literal_occurrences(text, route);
    for variant in route_match_variants(route) {
        out.extend(literal_occurrences(text, &variant));
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub(super) fn literal_occurrences(text: &str, needle: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(needle) {
        let idx = offset + pos;
        out.push(idx);
        offset = idx + needle.len();
    }
    out
}

fn route_match_variants(route: &str) -> Vec<String> {
    let segments: Vec<&str> = route
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let Some(param_idx) = segments
        .iter()
        .position(|segment| is_route_parameter_segment(segment))
    else {
        return Vec::new();
    };
    let mut variants = Vec::new();
    let prefix = format!("/{}", segments[..param_idx].join("/"));
    if prefix != "/" {
        variants.push(format!("{prefix}/"));
    }
    variants
}

pub(super) fn is_route_parameter_segment(segment: &str) -> bool {
    (segment.starts_with('{') && segment.ends_with('}'))
        || segment.starts_with(':')
        || (segment.starts_with('<') && segment.ends_with('>'))
}

fn dotted_property_names(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] != b'.' || !is_ident_start(bytes[i + 1] as char) {
            i += 1;
            continue;
        }
        let start = i + 1;
        i = start + 1;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        out.push(text[start..i].to_string());
    }
    out
}

fn java_getter_property_names(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 2 < bytes.len() {
        if bytes[i] != b'.' || !is_ident_start(bytes[i + 1] as char) {
            i += 1;
            continue;
        }
        let start = i + 1;
        i = start + 1;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        let name = &text[start..i];
        let open = skip_ws(text, i);
        if text.as_bytes().get(open) == Some(&b'(')
            && text.as_bytes().get(skip_ws(text, open + 1)) == Some(&b')')
            && is_plain_response_key(name)
            && !is_java_common_method(name)
        {
            out.push(name.to_string());
        }
    }
    out
}

fn is_java_common_method(name: &str) -> bool {
    matches!(
        name,
        "get"
            | "post"
            | "put"
            | "patch"
            | "delete"
            | "request"
            | "exchange"
            | "retrieve"
            | "bodyToMono"
            | "block"
            | "toString"
            | "hashCode"
            | "equals"
            | "json"
            | "text"
    )
}

fn bracket_string_property_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find('[') {
        let open = offset + pos;
        let start = skip_ws(text, open + 1);
        let Some((literal, end)) = read_string_literal(text, start) else {
            offset = open + 1;
            continue;
        };
        let close = skip_ws(text, end);
        if text.as_bytes().get(close) == Some(&b']') && is_plain_response_key(&literal) {
            out.push(literal);
        }
        offset = open + 1;
    }
    out
}

fn is_plain_response_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|ch| ch == '_' || ch == '-' || ch.is_ascii_alphanumeric())
}

fn is_common_property(key: &str) -> bool {
    matches!(
        key,
        "then"
            | "catch"
            | "finally"
            | "json"
            | "text"
            | "ok"
            | "status"
            | "headers"
            | "get"
            | "post"
            | "put"
            | "patch"
            | "delete"
            | "request"
            | "internal"
            | "AsyncClient"
            | "class"
            | "map"
            | "filter"
            | "reduce"
            | "length"
            | "push"
            | "slice"
            | "data"
    )
}

pub(super) fn extract_response_keys(text: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for marker in [".json(", "json(", "return "] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(marker) {
            let idx = offset + pos + marker.len();
            let idx = skip_ws(text, idx);
            if text.as_bytes().get(idx) == Some(&b'{') {
                if let Some(body) = balanced_brace_body(text, idx) {
                    keys.extend(top_level_object_keys(body));
                }
            }
            offset = idx.saturating_add(1);
        }
    }
    keys.into_iter().take(32).collect()
}

pub(super) fn extract_error_keys(response_keys: &[String], text: &str) -> Vec<String> {
    let mut keys: BTreeSet<String> = response_keys
        .iter()
        .filter(|key| matches!(key.as_str(), "error" | "errors" | "message" | "code"))
        .cloned()
        .collect();
    let lower = text.to_ascii_lowercase();
    for key in ["error", "errors", "message", "code"] {
        if lower.contains(key) {
            keys.insert(key.to_string());
        }
    }
    keys.into_iter().collect()
}

pub(super) fn extract_middleware(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for word in ident_words(text) {
        if word.starts_with("with") && word.len() > 4 {
            out.insert(word);
        }
    }
    for name in ["auth", "requireAuth", "rateLimit", "cors", "csrf"] {
        if text.contains(name) {
            out.insert(name.to_string());
        }
    }
    out.into_iter().take(12).collect()
}

fn balanced_brace_body(text: &str, open_idx: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.get(open_idx) != Some(&b'{') {
        return None;
    }
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    for (idx, byte) in bytes.iter().enumerate().skip(open_idx) {
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if *byte == b'\\' {
                escape = true;
            } else if *byte == q {
                quote = None;
            }
            continue;
        }
        match *byte {
            b'\'' | b'"' | b'`' => quote = Some(*byte),
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open_idx + 1..idx]);
                }
            }
            _ => {}
        }
    }
    None
}

fn top_level_object_keys(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let mut keys = Vec::new();
    let mut i = 0usize;
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' | b'`' if depth > 0 => quote = Some(b),
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => depth -= 1,
            b'\'' | b'"' if depth == 0 => {
                if let Some((key, end)) = read_string_literal(body, i) {
                    let after = skip_ws(body, end);
                    if body.as_bytes().get(after) == Some(&b':') && is_object_key(&key) {
                        keys.push(key);
                    }
                    i = after.saturating_add(1);
                    continue;
                }
            }
            _ if depth == 0 && is_ident_start(b as char) => {
                let start = i;
                i += 1;
                while i < bytes.len() && is_ident_continue(bytes[i] as char) {
                    i += 1;
                }
                let key = &body[start..i];
                let after = skip_ws(body, i);
                if body.as_bytes().get(after) == Some(&b':') && is_object_key(key) {
                    keys.push(key.to_string());
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    keys.sort();
    keys.dedup();
    keys
}

fn is_object_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '$'))
}

fn ident_words(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if !is_ident_start(bytes[i] as char) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && is_ident_continue(bytes[i] as char) {
            i += 1;
        }
        out.push(text[start..i].to_string());
    }
    out
}
