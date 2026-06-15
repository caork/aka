use std::collections::HashSet;

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
        let Some(source) = node_at_offset(text, nodes, call.start) else {
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
    out.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| a.callable.cmp(&b.callable))
    });
    out.dedup_by(|a, b| a.start == b.start && a.callable == b.callable);
    out
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
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
