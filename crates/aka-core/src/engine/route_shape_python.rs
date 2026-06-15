use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{find_call_args, read_repo_text, split_top_level_commas};

pub(super) fn extract_python_response_model_keys(text: &str) -> Vec<String> {
    extract_python_response_model_keys_from_models(text, python_class_fields(text))
}

pub(super) fn extract_python_response_model_keys_for_file(
    repo: &Path,
    file_path: &str,
    text: &str,
) -> Vec<String> {
    let mut models = python_class_fields(text);
    for import in python_response_model_imports(text) {
        let Some(import_file) = resolve_python_import_file(repo, file_path, &import.module) else {
            continue;
        };
        let Some(import_text) = read_repo_text(repo, &import_file) else {
            continue;
        };
        let imported_models = python_class_fields(&import_text);
        if let Some(fields) = imported_models.get(&import.name) {
            models
                .entry(import.alias_or_name())
                .or_insert_with(|| fields.clone());
        }
    }
    let mut keys: BTreeSet<String> =
        extract_python_response_model_keys_from_models(text, models.clone())
            .into_iter()
            .collect();
    keys.extend(extract_drf_serializer_keys_from_models(text, &models));
    keys.into_iter().collect()
}

fn extract_python_response_model_keys_from_models(
    text: &str,
    models: BTreeMap<String, Vec<String>>,
) -> Vec<String> {
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

fn extract_drf_serializer_keys_from_models(
    text: &str,
    models: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for serializer in drf_serializer_class_names(text) {
        if let Some(fields) = models.get(&serializer) {
            keys.extend(fields.iter().cloned());
        }
    }
    keys.into_iter().collect()
}

fn drf_serializer_class_names(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some((left, right)) = trimmed.split_once('=') else {
            continue;
        };
        if left.trim() != "serializer_class" {
            continue;
        }
        let name = right.trim().split('#').next().unwrap_or("").trim();
        if let Some(simple) = python_model_name(name) {
            out.insert(simple);
        }
    }
    out.into_iter().collect()
}

#[derive(Debug)]
struct PythonModelImport {
    module: String,
    name: String,
    alias: Option<String>,
}

impl PythonModelImport {
    fn alias_or_name(&self) -> String {
        self.alias.clone().unwrap_or_else(|| self.name.clone())
    }
}

fn python_response_model_imports(text: &str) -> Vec<PythonModelImport> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("from ") else {
            continue;
        };
        let Some((module, names)) = rest.split_once(" import ") else {
            continue;
        };
        for raw in names.split(',') {
            let raw = raw.trim().trim_matches(['(', ')']);
            if raw.is_empty() || raw == "*" {
                continue;
            }
            let (name, alias) = split_python_import_alias(raw);
            if is_python_ident(name) && alias.is_none_or(is_python_ident) {
                out.push(PythonModelImport {
                    module: module.trim().to_string(),
                    name: name.to_string(),
                    alias: alias.map(str::to_string),
                });
            }
        }
    }
    out
}

fn split_python_import_alias(raw: &str) -> (&str, Option<&str>) {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    match parts.as_slice() {
        [name, "as", alias] => (name, Some(alias)),
        [name] => (name, None),
        _ => (raw, None),
    }
}

fn resolve_python_import_file(repo: &Path, file_path: &str, module: &str) -> Option<String> {
    let rel = if module.starts_with('.') {
        relative_python_module_path(file_path, module)
    } else {
        module.replace('.', "/")
    };
    let candidates = [
        format!("{rel}.py"),
        format!("{rel}/__init__.py"),
        strip_first_module_segment(&rel)
            .map(|v| format!("{v}.py"))
            .unwrap_or_default(),
        strip_first_module_segment(&rel)
            .map(|v| format!("{v}/__init__.py"))
            .unwrap_or_default(),
    ];
    candidates
        .into_iter()
        .filter(|candidate| !candidate.is_empty())
        .find(|candidate| repo.join(candidate).is_file())
}

fn relative_python_module_path(file_path: &str, module: &str) -> String {
    let dots = module.chars().take_while(|ch| *ch == '.').count();
    let tail = module.trim_start_matches('.');
    let mut base = PathBuf::from(file_path.replace('\\', "/"));
    base.pop();
    for _ in 1..dots {
        base.pop();
    }
    if !tail.is_empty() {
        for segment in tail.split('.') {
            base.push(segment);
        }
    }
    base.to_string_lossy().replace('\\', "/")
}

fn strip_first_module_segment(path: &str) -> Option<String> {
    path.split_once('/').map(|(_, rest)| rest.to_string())
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
        let body_start = idx;
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
        fields.extend(python_meta_fields(&lines[body_start..idx], class_indent));
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

fn python_meta_fields(lines: &[&str], class_indent: usize) -> Vec<String> {
    let mut out = BTreeSet::new();
    let mut idx = 0usize;
    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim();
        if !(trimmed == "class Meta:" || trimmed.starts_with("class Meta(")) {
            idx += 1;
            continue;
        }
        let meta_indent = leading_spaces(line);
        if meta_indent <= class_indent {
            break;
        }
        idx += 1;
        while idx < lines.len() {
            let current = lines[idx];
            let current_trimmed = current.trim();
            let indent = leading_spaces(current);
            if !current_trimmed.is_empty() && indent <= meta_indent {
                break;
            }
            if indent == meta_indent + 4 && current_trimmed.starts_with("fields") {
                out.extend(parse_python_fields_assignment(current_trimmed));
            }
            idx += 1;
        }
    }
    out.into_iter().collect()
}

fn parse_python_fields_assignment(line: &str) -> Vec<String> {
    let Some((left, right)) = line.split_once('=') else {
        return Vec::new();
    };
    if left.trim() != "fields" {
        return Vec::new();
    }
    let value = right.split('#').next().unwrap_or("").trim();
    if value == "__all__" || value == "'__all__'" || value == "\"__all__\"" {
        return Vec::new();
    }
    split_top_level_commas(value.trim_matches(['[', ']', '(', ')']))
        .into_iter()
        .filter_map(|item| {
            let item = item.trim().trim_end_matches(',');
            let quote = item.as_bytes().first().copied()?;
            if !matches!(quote, b'\'' | b'"') || item.as_bytes().last().copied() != Some(quote) {
                return None;
            }
            let field = &item[1..item.len().saturating_sub(1)];
            is_python_ident(field).then(|| field.to_string())
        })
        .collect()
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
            (rhs.starts_with("Field(") || rhs.starts_with("serializers.") || rhs.contains("Field("))
                .then_some(name)
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
