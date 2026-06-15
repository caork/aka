use std::collections::{BTreeMap, HashSet};

use serde_json::json;

use super::super::{
    find_call_args, node_at_offset, split_top_level_commas, stable_hash, EdgeRec, SynthNode,
};
use super::dependency_edge;
use super::lookup::NodeLookup;

pub(super) fn detect_python_dependency_edges(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
    lookup: &NodeLookup<'_>,
    existing_call_pairs: &HashSet<(String, String)>,
) -> Vec<EdgeRec> {
    if !is_python_file(file_path, nodes) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for call in find_python_dependency_calls(text) {
        let Some(source) = node_at_offset(text, nodes, call.start)
            .or_else(|| next_python_function_node(text, nodes, call.start))
        else {
            continue;
        };
        let Some(target) = lookup.resolve_python_callable(file_path, &call.callable) else {
            continue;
        };
        if target.aka_id == source.aka_id {
            continue;
        }
        out.push(dependency_edge(
            &source.aka_id,
            &target.aka_id,
            "python-fastapi-dependency",
            &call.strategy,
            &call.callable,
            0.74,
        ));
        if !existing_call_pairs.contains(&(source.aka_id.clone(), target.aka_id.clone())) {
            out.push(EdgeRec {
                id: format!(
                    "python-depends-call:{:016x}",
                    stable_hash(&format!(
                        "{}|{}|{}",
                        source.aka_id, target.aka_id, call.callable
                    ))
                ),
                source_id: source.aka_id.clone(),
                target_id: target.aka_id.clone(),
                edge_type: "CALLS".into(),
                confidence: 0.74,
                reason: "aka FastAPI Depends dependency call".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "fastapi-depends",
                    "dependency": call.callable,
                    "strategy": call.strategy,
                })),
            });
        }
    }
    out
}

fn next_python_function_node<'a>(
    text: &str,
    nodes: &[&'a SynthNode],
    offset: usize,
) -> Option<&'a SynthNode> {
    let line = line_number_at_offset(text, offset);
    let node = nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
        .filter(|node| node.start_line_key() >= line)
        .min_by_key(|node| node.start_line_key())?;
    dependency_call_is_in_decorator_window(text, offset, node.start_line_key()).then_some(node)
}

fn dependency_call_is_in_decorator_window(text: &str, offset: usize, node_line: i64) -> bool {
    if node_line <= 1 {
        return false;
    }
    let call_line = line_number_at_offset(text, offset);
    if call_line >= node_line {
        return false;
    }
    let lines: Vec<&str> = text.lines().collect();
    let from = call_line.saturating_sub(1) as usize;
    let to = (node_line.saturating_sub(1) as usize).min(lines.len());
    from < to
        && lines[from..to].iter().all(|line| {
            let trimmed = line.trim();
            trimmed.is_empty() || trimmed.starts_with('@') || trimmed.starts_with("dependencies=")
        })
}

fn line_number_at_offset(text: &str, offset: usize) -> i64 {
    let bounded = offset.min(text.len());
    let mut line = 1i64;
    for (idx, ch) in text.char_indices() {
        if idx >= bounded {
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    line
}

#[derive(Debug, Clone)]
struct PythonDependencyCall {
    start: usize,
    callable: String,
    strategy: String,
}

fn find_python_dependency_calls(text: &str) -> Vec<PythonDependencyCall> {
    let mut out = Vec::new();
    for callee in ["Depends", "Security"] {
        for call in find_call_args(text, callee) {
            if let Some(callable) = first_depends_callable(call.args) {
                out.push(PythonDependencyCall {
                    start: call.start,
                    callable,
                    strategy: format!("python-fastapi-{}", callee.to_ascii_lowercase()),
                });
            }
        }
    }
    let aliases = python_annotated_dependency_aliases(text, &out);
    for (alias, calls) in aliases {
        for usage in python_annotation_alias_usages(text, &alias) {
            for call in &calls {
                out.push(PythonDependencyCall {
                    start: usage,
                    callable: call.callable.clone(),
                    strategy: "python-fastapi-annotated-alias".into(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| a.callable.cmp(&b.callable))
    });
    out.dedup_by(|a, b| a.start == b.start && a.callable == b.callable);
    out
}

fn python_annotated_dependency_aliases(
    text: &str,
    calls: &[PythonDependencyCall],
) -> BTreeMap<String, Vec<PythonDependencyCall>> {
    let mut aliases: BTreeMap<String, Vec<PythonDependencyCall>> = BTreeMap::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find("Annotated[") {
        let annotated_pos = offset + rel;
        let line_start = text[..annotated_pos]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let prefix = &text[line_start..annotated_pos];
        let Some(alias) = python_annotated_alias_name(prefix) else {
            offset = annotated_pos + "Annotated[".len();
            continue;
        };
        let open = annotated_pos + "Annotated".len();
        let Some(close) = find_matching_square_bracket(text, open) else {
            offset = open + 1;
            continue;
        };
        for call in calls
            .iter()
            .filter(|call| call.start > open && call.start < close)
        {
            aliases.entry(alias.clone()).or_default().push(call.clone());
        }
        offset = close + 1;
    }
    aliases
}

fn python_annotated_alias_name(prefix: &str) -> Option<String> {
    let (left, _) = prefix.split_once('=')?;
    let alias = left
        .trim()
        .split_once(':')
        .map(|(alias, _)| alias)
        .unwrap_or(left)
        .trim();
    is_python_identifier(alias).then(|| alias.to_string())
}

fn find_matching_square_bracket(text: &str, open: usize) -> Option<usize> {
    if text.as_bytes().get(open) != Some(&b'[') {
        return None;
    }
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut escape = false;
    for (idx, byte) in text.bytes().enumerate().skip(open) {
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
            b'[' => depth += 1,
            b']' => {
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

fn python_annotation_alias_usages(text: &str, alias: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for call in find_python_function_defs(text) {
        let args = call.args;
        let mut offset = 0usize;
        while let Some(rel) = args[offset..].find(alias) {
            let start = offset + rel;
            if python_alias_annotation_boundary_ok(args, start, alias) {
                out.push(call.start + start);
            }
            offset = start + alias.len();
        }
    }
    out.sort();
    out.dedup();
    out
}

#[derive(Debug)]
struct PythonFunctionDef<'a> {
    start: usize,
    args: &'a str,
}

fn find_python_function_defs(text: &str) -> Vec<PythonFunctionDef<'_>> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find("def ") {
        let def_pos = offset + rel;
        if !python_keyword_boundary_ok(text, def_pos, "def") {
            offset = def_pos + "def".len();
            continue;
        }
        let Some(open_rel) = text[def_pos..].find('(') else {
            break;
        };
        let open = def_pos + open_rel;
        let Some(close) = super::super::find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        out.push(PythonFunctionDef {
            start: open + 1,
            args: &text[open + 1..close],
        });
        offset = close + 1;
    }
    out
}

fn python_alias_annotation_boundary_ok(args: &str, start: usize, alias: &str) -> bool {
    let before = args[..start].chars().rev().find(|ch| !ch.is_whitespace());
    let after = args[start + alias.len()..]
        .chars()
        .find(|ch| !ch.is_whitespace());
    before == Some(':') && after.is_none_or(|ch| matches!(ch, ',' | '=' | ')' | ']'))
}

fn python_keyword_boundary_ok(text: &str, start: usize, keyword: &str) -> bool {
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
    before.is_none_or(|ch| !is_python_ident_continue(ch))
        && after.is_none_or(|ch| !is_python_ident_continue(ch))
}

fn first_depends_callable(args: &str) -> Option<String> {
    for arg in split_top_level_commas(args) {
        let arg = arg.trim();
        if arg.is_empty() {
            continue;
        }
        let value = if let Some((key, value)) = arg.split_once('=') {
            if !matches!(key.trim(), "dependency" | "call" | "callable") {
                continue;
            }
            value.trim()
        } else {
            arg
        };
        let value = value.trim_start_matches("lambda ").trim();
        if value.starts_with('"') || value.starts_with('\'') || value.starts_with("None") {
            continue;
        }
        let name = value
            .split_once('(')
            .map(|(name, _)| name)
            .unwrap_or(value)
            .trim();
        if is_python_callable_expr(name) {
            return Some(name.to_string());
        }
    }
    None
}

fn is_python_file(file_path: &str, nodes: &[&SynthNode]) -> bool {
    file_path.to_ascii_lowercase().ends_with(".py")
        || nodes
            .iter()
            .any(|node| node.language.eq_ignore_ascii_case("python"))
}

fn is_python_callable_expr(expr: &str) -> bool {
    let mut parts = expr.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    is_python_identifier(first) && parts.all(is_python_identifier)
}

fn is_python_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic()) && chars.all(is_python_ident_continue)
}

fn is_python_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}
