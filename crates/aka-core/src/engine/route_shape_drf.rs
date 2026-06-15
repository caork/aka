use std::collections::BTreeSet;

use super::route_shape_python::{python_model_name, PythonModelFields};
use super::split_top_level_commas;

pub(super) fn extract_drf_serializer_keys_from_models(
    text: &str,
    models: &PythonModelFields,
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
    let lines: Vec<&str> = text.lines().collect();
    for line in &lines {
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
    out.extend(drf_serializer_map_class_names(&lines));
    out.extend(drf_get_serializer_class_names(&lines));
    out.into_iter().collect()
}

fn drf_serializer_map_class_names(lines: &[&str]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut idx = 0usize;
    while idx < lines.len() {
        let trimmed = lines[idx].trim();
        let Some((left, right)) = trimmed.split_once('=') else {
            idx += 1;
            continue;
        };
        let left = left.trim();
        if !matches!(
            left,
            "serializer_action_classes" | "serializer_classes" | "action_serializers"
        ) {
            idx += 1;
            continue;
        }
        let mut expr = right.trim().to_string();
        while brace_balance(&expr) > 0 && idx + 1 < lines.len() {
            idx += 1;
            expr.push('\n');
            expr.push_str(lines[idx].trim());
        }
        collect_python_class_names(&expr, &mut out);
        idx += 1;
    }
    out
}

fn drf_get_serializer_class_names(lines: &[&str]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_method = false;
    let mut method_indent = 0usize;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = leading_spaces(line);
        if trimmed.starts_with("def get_serializer_class(")
            || trimmed.starts_with("def get_serializer_class (")
        {
            in_method = true;
            method_indent = indent;
            continue;
        }
        if in_method && indent <= method_indent {
            in_method = false;
        }
        if !in_method || !trimmed.starts_with("return ") {
            continue;
        }
        let expr = trimmed
            .trim_start_matches("return ")
            .split('#')
            .next()
            .unwrap_or("")
            .trim();
        collect_python_class_names(expr, &mut out);
    }
    out
}

fn collect_python_class_names(expr: &str, out: &mut BTreeSet<String>) {
    for token in expr
        .split(|ch: char| !(ch == '.' || ch == '_' || ch.is_ascii_alphanumeric()))
        .filter(|token| !token.is_empty())
    {
        if let Some(name) = python_model_name(token) {
            if name.ends_with("Serializer") {
                out.insert(name);
            }
        }
    }
    for arg in split_top_level_commas(expr.trim_matches(['(', ')'])) {
        if let Some(name) = python_model_name(arg.trim()) {
            if name.ends_with("Serializer") {
                out.insert(name);
            }
        }
    }
}

fn brace_balance(text: &str) -> i32 {
    text.chars().fold(0, |balance, ch| match ch {
        '{' | '[' | '(' => balance + 1,
        '}' | ']' | ')' => balance - 1,
        _ => balance,
    })
}

fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}
