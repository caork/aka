use std::collections::{BTreeMap, BTreeSet};

use super::{find_call_args, split_top_level_commas};

pub(super) fn extract_python_response_model_keys(text: &str) -> Vec<String> {
    let models = python_class_fields(text);
    if models.is_empty() {
        return Vec::new();
    }
    let mut keys = BTreeSet::new();
    for call in find_call_args(text, ".get")
        .into_iter()
        .chain(find_call_args(text, ".post"))
        .chain(find_call_args(text, ".put"))
        .chain(find_call_args(text, ".patch"))
        .chain(find_call_args(text, ".delete"))
    {
        if let Some(model) = python_response_model_arg(call.args) {
            if let Some(fields) = models.get(&model) {
                keys.extend(fields.iter().cloned());
            }
        }
    }
    keys.into_iter().collect()
}

fn python_response_model_arg(args: &str) -> Option<String> {
    for arg in split_top_level_commas(args) {
        let Some((name, value)) = arg.split_once('=') else {
            continue;
        };
        if name.trim() != "response_model" {
            continue;
        }
        return python_model_name(value.trim());
    }
    None
}

fn python_model_name(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value
        .strip_prefix("list[")
        .and_then(|v| v.strip_suffix(']'))
        .or_else(|| {
            value
                .strip_prefix("List[")
                .and_then(|v| v.strip_suffix(']'))
        })
        .unwrap_or(value);
    let value = value
        .strip_prefix("Optional[")
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    let simple = value.rsplit('.').next().unwrap_or(value).trim();
    is_python_ident(simple).then(|| simple.to_string())
}

fn python_class_fields(text: &str) -> BTreeMap<String, Vec<String>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = BTreeMap::new();
    let mut idx = 0usize;
    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim_start();
        if !trimmed.starts_with("class ") {
            idx += 1;
            continue;
        }
        let Some((name, bases)) = python_class_decl(trimmed) else {
            idx += 1;
            continue;
        };
        let class_indent = leading_spaces(line);
        let mut fields = Vec::new();
        idx += 1;
        while idx < lines.len() {
            let current = lines[idx];
            let current_trimmed = current.trim();
            if current_trimmed.is_empty() || current_trimmed.starts_with('#') {
                idx += 1;
                continue;
            }
            let indent = leading_spaces(current);
            if indent <= class_indent {
                break;
            }
            if indent == class_indent + 4 {
                if let Some(field) = python_field_name(current_trimmed) {
                    fields.push(field);
                }
            }
            idx += 1;
        }
        if !fields.is_empty()
            && (bases.contains("BaseModel")
                || bases.contains("Schema")
                || bases.contains("Serializer"))
        {
            fields.sort();
            fields.dedup();
            out.insert(name, fields);
        }
    }
    out
}

fn python_class_decl(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("class ")?.trim_start();
    let name_end = rest
        .find(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric()))
        .unwrap_or(rest.len());
    let name = &rest[..name_end];
    if !is_python_ident(name) {
        return None;
    }
    let bases = rest[name_end..]
        .split_once('(')
        .and_then(|(_, v)| v.split_once(')'))
        .map(|(v, _)| v.to_string())
        .unwrap_or_default();
    Some((name.to_string(), bases))
}

fn python_field_name(line: &str) -> Option<String> {
    if line.starts_with("def ")
        || line.starts_with("async ")
        || line.starts_with("class ")
        || line.starts_with('@')
    {
        return None;
    }
    let code = line.split('#').next()?.trim();
    let name = code
        .split_once(':')
        .map(|(name, _)| name)
        .or_else(|| {
            let (name, rhs) = code.split_once('=')?;
            let rhs = rhs.trim_start();
            (rhs.starts_with("Field(")).then_some(name)
        })?
        .trim();
    (is_python_ident(name) && !name.starts_with("__")).then(|| name.to_string())
}

fn is_python_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}
