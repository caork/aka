use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::route_shape_java_builder::{java_builder_response_keys, java_builder_response_types};
use super::{
    find_call_args, find_matching_paren, is_ident_continue, read_repo_text, read_string_literal,
    skip_ws, split_top_level_commas,
};

pub(super) fn extract_java_response_model_keys_for_file(
    repo: &Path,
    file_path: &str,
    text: &str,
) -> Vec<String> {
    let mut models = java_type_fields(text);
    for import in java_imports(text) {
        let Some(import_file) = resolve_java_import_file(repo, &import) else {
            continue;
        };
        let Some(import_text) = read_repo_text(repo, &import_file) else {
            continue;
        };
        models.extend(java_type_fields(&import_text));
    }
    let mut keys = BTreeSet::new();
    for model in java_route_response_types(text, file_path) {
        if let Some(fields) = models.get(&model) {
            keys.extend(fields.iter().cloned());
        }
    }
    keys.extend(java_builder_response_keys(text));
    keys.into_iter().collect()
}

pub(super) fn extract_java_map_response_keys(text: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for call in find_call_args(text, "Map.of") {
        keys.extend(java_map_of_keys(call.args));
    }
    for call in find_call_args(text, "Map.ofEntries") {
        keys.extend(java_map_of_entries_keys(call.args));
    }
    keys.into_iter().collect()
}

fn java_route_response_types(text: &str, file_path: &str) -> Vec<String> {
    if !file_path.ends_with(".java") {
        return Vec::new();
    }
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !(trimmed.contains("public ")
            || trimmed.contains("private ")
            || trimmed.contains("protected "))
        {
            continue;
        }
        if !(trimmed.contains('(') && trimmed.contains(')') && trimmed.contains('{')) {
            continue;
        }
        let Some(prefix) = trimmed.split_once('(').map(|(prefix, _)| prefix.trim()) else {
            continue;
        };
        let Some(return_type) = prefix.split_whitespace().rev().nth(1) else {
            continue;
        };
        if let Some(model) = java_response_model_name(return_type) {
            out.insert(model);
        }
    }
    out.extend(java_constructed_response_types(text));
    out.extend(java_builder_response_types(text));
    out.into_iter().collect()
}

fn java_constructed_response_types(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !(trimmed.starts_with("return ")
            || trimmed.contains("ResponseEntity.")
            || trimmed.contains(".body("))
        {
            continue;
        }
        let mut offset = 0usize;
        while let Some(pos) = trimmed[offset..].find("new ") {
            let start = offset + pos + "new ".len();
            let Some((name, end)) = read_java_identifier(trimmed, skip_ws(trimmed, start)) else {
                offset = start;
                continue;
            };
            let simple = name.rsplit('.').next().unwrap_or(name);
            if is_java_ident(simple) && !is_java_common_constructed_type(simple) {
                out.insert(simple.to_string());
            }
            offset = end;
        }
    }
    out.into_iter().collect()
}

pub(super) fn is_java_common_constructed_type(name: &str) -> bool {
    matches!(
        name,
        "String" | "Object" | "ArrayList" | "HashMap" | "LinkedHashMap" | "ResponseEntity"
    )
}

fn java_response_model_name(return_type: &str) -> Option<String> {
    let ty = return_type.trim();
    if matches!(
        ty,
        "void"
            | "String"
            | "boolean"
            | "Boolean"
            | "int"
            | "Integer"
            | "long"
            | "Long"
            | "double"
            | "Double"
            | "float"
            | "Float"
    ) {
        return None;
    }
    let ty = ty
        .strip_prefix("ResponseEntity<")
        .and_then(|v| v.strip_suffix('>'))
        .unwrap_or(ty);
    let ty = ty
        .strip_prefix("HttpEntity<")
        .and_then(|v| v.strip_suffix('>'))
        .unwrap_or(ty);
    let ty = ty
        .strip_prefix("Mono<")
        .and_then(|v| v.strip_suffix('>'))
        .unwrap_or(ty);
    let ty = ty
        .strip_prefix("Optional<")
        .and_then(|v| v.strip_suffix('>'))
        .unwrap_or(ty);
    let ty = ty
        .strip_prefix("List<")
        .and_then(|v| v.strip_suffix('>'))
        .or_else(|| {
            ty.strip_prefix("Collection<")
                .and_then(|v| v.strip_suffix('>'))
        })
        .unwrap_or(ty);
    let simple = ty.rsplit('.').next().unwrap_or(ty).trim();
    is_java_ident(simple).then(|| simple.to_string())
}

fn java_type_fields(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    out.extend(java_record_fields(text));
    out.extend(java_class_fields(text));
    out
}

fn java_record_fields(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find("record ") {
        let record_pos = offset + pos;
        if !keyword_boundary_ok(text, record_pos, "record") {
            offset = record_pos + "record".len();
            continue;
        }
        let name_start = skip_ws(text, record_pos + "record".len());
        let Some((name, name_end)) = read_java_identifier(text, name_start) else {
            offset = record_pos + "record".len();
            continue;
        };
        let open = skip_ws(text, name_end);
        if text.as_bytes().get(open) != Some(&b'(') {
            offset = name_end;
            continue;
        }
        if let Some(close) = find_matching_paren(text, open) {
            let fields = split_top_level_commas(&text[open + 1..close])
                .into_iter()
                .filter_map(java_record_component_name)
                .collect::<Vec<_>>();
            if !fields.is_empty() {
                out.insert(name.to_string(), fields);
            }
            offset = close + 1;
        } else {
            offset = open + 1;
        }
    }
    out
}

fn java_record_component_name(component: &str) -> Option<String> {
    let cleaned = component.trim();
    let name = cleaned.split_whitespace().last()?.trim();
    is_object_key(name).then(|| name.to_string())
}

fn java_class_fields(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find("class ") {
        let class_pos = offset + pos;
        if !keyword_boundary_ok(text, class_pos, "class") {
            offset = class_pos + "class".len();
            continue;
        }
        let name_start = skip_ws(text, class_pos + "class".len());
        let Some((name, name_end)) = read_java_identifier(text, name_start) else {
            offset = class_pos + "class".len();
            continue;
        };
        let Some(open_rel) = text[name_end..].find('{') else {
            break;
        };
        let open = name_end + open_rel;
        let Some(close) = matching_brace(text, open) else {
            offset = open + 1;
            continue;
        };
        let fields = java_member_fields(&text[open + 1..close]);
        if !fields.is_empty() {
            out.insert(name.to_string(), fields);
        }
        offset = close + 1;
    }
    out
}

fn java_member_fields(body: &str) -> Vec<String> {
    let mut fields = BTreeSet::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("@JsonProperty") {
            if let Some(name) = java_json_property_name(trimmed) {
                fields.insert(name);
            }
            continue;
        }
        if !trimmed.ends_with(';') || trimmed.contains('(') {
            continue;
        }
        let name = trimmed
            .trim_end_matches(';')
            .split('=')
            .next()
            .unwrap_or("")
            .split_whitespace()
            .last()
            .unwrap_or("")
            .trim();
        if is_object_key(name) && !matches!(name, "serialVersionUID") {
            fields.insert(name.to_string());
        }
    }
    fields.into_iter().collect()
}

fn java_json_property_name(line: &str) -> Option<String> {
    let call = find_call_args(line, "JsonProperty").into_iter().next()?;
    split_top_level_commas(call.args)
        .into_iter()
        .find_map(java_string_arg)
        .filter(|name| is_object_key(name))
}

fn java_imports(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("import "))
        .filter_map(|line| line.trim_end_matches(';').split_whitespace().next())
        .filter(|import| !import.ends_with(".*") && !import.starts_with("static "))
        .map(str::to_string)
        .collect()
}

fn resolve_java_import_file(repo: &Path, import: &str) -> Option<String> {
    let rel = format!("{}.java", import.replace('.', "/"));
    let mut candidates = vec![rel.clone()];
    for root in [
        "src/main/java",
        "src/main/kotlin",
        "src/main/groovy",
        "src/main/scala",
    ] {
        candidates.push(format!("{root}/{rel}"));
    }
    if let Some(stripped) = strip_first_module_segment(&rel) {
        candidates.push(stripped);
    }
    candidates
        .into_iter()
        .find(|candidate| repo.join(candidate).is_file())
}

fn strip_first_module_segment(path: &str) -> Option<String> {
    path.split_once('/').map(|(_, rest)| rest.to_string())
}

fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    for (idx, byte) in bytes.iter().enumerate().skip(open) {
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
            b'\'' | b'"' => quote = Some(*byte),
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn keyword_boundary_ok(text: &str, start: usize, keyword: &str) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|idx| text.as_bytes().get(idx))
        .copied()
        .map(char::from);
    let after = text
        .as_bytes()
        .get(start + keyword.len())
        .copied()
        .map(char::from);
    before.is_none_or(|ch| !is_ident_continue(ch)) && after.is_none_or(|ch| !is_ident_continue(ch))
}

pub(super) fn read_java_identifier(text: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    let first = *bytes.get(start)? as char;
    if !is_java_ident_start(first) {
        return None;
    }
    let mut end = start + 1;
    while end < text.len() && is_ident_continue(bytes[end] as char) {
        end += 1;
    }
    Some((&text[start..end], end))
}

pub(super) fn is_java_ident(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    is_java_ident_start(first)
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn is_java_ident_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphabetic()
}

fn java_map_of_keys(args: &str) -> Vec<String> {
    split_top_level_commas(args)
        .into_iter()
        .step_by(2)
        .filter_map(java_string_arg)
        .filter(|key| is_object_key(key))
        .collect()
}

fn java_map_of_entries_keys(args: &str) -> Vec<String> {
    split_top_level_commas(args)
        .into_iter()
        .filter_map(|entry| {
            let trimmed = entry.trim();
            for callee in ["Map.entry", "entry"] {
                for call in find_call_args(trimmed, callee) {
                    let first = split_top_level_commas(call.args).into_iter().next()?;
                    if let Some(key) = java_string_arg(first) {
                        return Some(key);
                    }
                }
            }
            None
        })
        .filter(|key| is_object_key(key))
        .collect()
}

fn java_string_arg(arg: &str) -> Option<String> {
    let trimmed = arg.trim();
    let start = skip_ws(trimmed, 0);
    read_string_literal(trimmed, start)
        .and_then(|(literal, end)| (skip_ws(trimmed, end) == trimmed.len()).then_some(literal))
}

pub(super) fn is_object_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '$'))
}
