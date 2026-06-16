use std::{collections::BTreeMap, path::Path};

use super::route_python_prefix_synth::python_route_prefixes_for_decorators;
use super::{
    join_route_paths, normalize_route_literal, read_repo_text, source_annotations_before_node,
    PythonRoutePrefixes, RouteCandidate, SynthNode,
};

pub(super) fn realtime_routes_by_file(
    repo: &Path,
    nodes_by_file: &BTreeMap<String, Vec<&SynthNode>>,
    python_prefixes_by_file: &BTreeMap<String, PythonRoutePrefixes>,
) -> BTreeMap<String, Vec<RouteCandidate>> {
    let mut out = BTreeMap::new();
    for (file_path, nodes) in nodes_by_file {
        let text = read_repo_text(repo, file_path).unwrap_or_default();
        let routes = realtime_routes(&text, nodes, python_prefixes_by_file.get(file_path));
        if !routes.is_empty() {
            out.insert(file_path.clone(), routes);
        }
    }
    out
}

fn realtime_routes(
    text: &str,
    nodes: &[&SynthNode],
    python_prefixes: Option<&PythonRoutePrefixes>,
) -> Vec<RouteCandidate> {
    let mut class_prefixes: BTreeMap<String, String> = BTreeMap::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        let Some(prefix) = realtime_mapping_path(&decorators_for_node(text, node)) else {
            continue;
        };
        class_prefixes.insert(node.aka_id.clone(), prefix.clone());
        class_prefixes.insert(node.qn.clone(), prefix);
    }

    let mut routes = Vec::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        let decorators = decorators_for_node(text, node);
        for mapping in realtime_method_mappings(&decorators) {
            let prefixes =
                realtime_prefixes_for_node(node, &decorators, &class_prefixes, python_prefixes);
            for prefix in prefixes {
                routes.push(RouteCandidate {
                    route: join_realtime_paths(&prefix, &mapping.path),
                    method: Some(mapping.method.clone()),
                    handler_id: Some(node.aka_id.clone()),
                    handler_name: Some(node.display_name().to_string()),
                });
            }
        }
    }
    routes.sort_by(|a, b| a.route.cmp(&b.route).then_with(|| a.method.cmp(&b.method)));
    routes.dedup_by(|a, b| a.route == b.route && a.method == b.method);
    routes
}

fn decorators_for_node(text: &str, node: &SynthNode) -> Vec<String> {
    let mut decorators = node.decorators.clone();
    decorators.extend(source_annotations_before_node(text, node));
    decorators.sort();
    decorators.dedup();
    decorators
}

fn realtime_prefixes_for_node(
    node: &SynthNode,
    decorators: &[String],
    class_prefixes: &BTreeMap<String, String>,
    python_prefixes: Option<&PythonRoutePrefixes>,
) -> Vec<String> {
    if is_python_node(node) {
        python_route_prefixes_for_decorators(python_prefixes, decorators)
    } else {
        vec![node
            .parent_class
            .as_ref()
            .and_then(|parent| class_prefixes.get(parent))
            .cloned()
            .unwrap_or_default()]
    }
}

struct RealtimeMapping {
    path: String,
    method: String,
}

fn realtime_method_mappings(decorators: &[String]) -> Vec<RealtimeMapping> {
    let mut out = Vec::new();
    for decorator in decorators {
        let Some(name) = annotation_simple_name(decorator) else {
            continue;
        };
        let method = match name.to_ascii_lowercase().as_str() {
            "messagemapping" => "STOMP",
            "subscribemapping" => "STOMP_SUBSCRIBE",
            "serverendpoint" | "websocket" | "websocket_route" => "WEBSOCKET",
            _ => continue,
        };
        out.push(RealtimeMapping {
            path: annotation_path(decorator).unwrap_or_else(|| "/".into()),
            method: method.into(),
        });
    }
    out
}

fn realtime_mapping_path(decorators: &[String]) -> Option<String> {
    decorators.iter().find_map(|decorator| {
        let name = annotation_simple_name(decorator)?;
        matches!(
            name.to_ascii_lowercase().as_str(),
            "messagemapping" | "serverendpoint"
        )
        .then(|| annotation_path(decorator).unwrap_or_else(|| "/".into()))
    })
}

fn annotation_simple_name(annotation: &str) -> Option<&str> {
    let text = annotation.trim().trim_start_matches('@');
    let name = text.split_once('(').map(|(name, _)| name).unwrap_or(text);
    Some(name.rsplit('.').next().unwrap_or(name).trim()).filter(|name| !name.is_empty())
}

fn annotation_path(annotation: &str) -> Option<String> {
    let open = annotation.find('(')?;
    let close = annotation.rfind(')').unwrap_or(annotation.len());
    if open + 1 >= close {
        return None;
    }
    first_string_literal(&annotation[open + 1..close]).map(|path| {
        if path.starts_with('/') {
            path
        } else {
            format!("/{path}")
        }
    })
}

fn first_string_literal(text: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < text.len() {
        let ch = text[idx..].chars().next()?;
        if matches!(ch, '"' | '\'') {
            return read_string_literal(text, idx);
        }
        idx += ch.len_utf8();
    }
    None
}

fn read_string_literal(text: &str, start: usize) -> Option<String> {
    let quote = *text.as_bytes().get(start)?;
    if !matches!(quote, b'"' | b'\'') {
        return None;
    }
    let mut out = String::new();
    let mut escape = false;
    let mut idx = start + 1;
    while idx < text.len() {
        let byte = *text.as_bytes().get(idx)?;
        if escape {
            let ch = text[idx..].chars().next()?;
            out.push(ch);
            escape = false;
            idx += ch.len_utf8();
            continue;
        }
        if byte == b'\\' {
            escape = true;
            idx += 1;
            continue;
        }
        if byte == quote {
            return Some(out);
        }
        let ch = text[idx..].chars().next()?;
        out.push(ch);
        idx += ch.len_utf8();
    }
    None
}

fn join_realtime_paths(prefix: &str, suffix: &str) -> String {
    if prefix.is_empty() {
        normalize_route_literal(suffix)
    } else {
        join_route_paths(prefix, suffix)
    }
}

fn is_python_node(node: &SynthNode) -> bool {
    node.language.eq_ignore_ascii_case("python")
        || node.file_path.to_ascii_lowercase().ends_with(".py")
}
