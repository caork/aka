use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use super::{
    find_call_args, find_matching_paren, read_repo_text, read_string_literal, ProjectSourceSet,
    RouteCandidate, SynthNode,
};

pub(super) fn spring_functional_routes_from_repo(
    repo: &Path,
    project_sources: &ProjectSourceSet,
    nodes: &BTreeMap<String, SynthNode>,
) -> BTreeMap<String, Vec<RouteCandidate>> {
    let mut out = BTreeMap::new();
    let methods = java_methods_by_name(nodes);
    for file_path in spring_source_files(repo, project_sources) {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        let routes = spring_functional_routes(&text, &methods, java_package_name(&text).as_deref());
        if !routes.is_empty() {
            out.insert(file_path, routes);
        }
    }
    out
}

fn spring_functional_routes(
    text: &str,
    methods: &HashMap<String, &SynthNode>,
    package_name: Option<&str>,
) -> Vec<RouteCandidate> {
    if !(text.contains("RouterFunctions")
        || text.contains("RequestPredicates")
        || text.contains("andRoute")
        || text.contains("route("))
    {
        return Vec::new();
    }

    let mut out = Vec::new();
    let nest_prefixes = spring_nest_prefixes(text);
    for predicate in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
        for call in find_call_args(text, predicate) {
            let Some(route) = first_string_literal(call.args) else {
                continue;
            };
            let Some(handler_name) = handler_method_after(text, call.start) else {
                continue;
            };
            let handler = method_in_package(methods, &handler_name, package_name)
                .or_else(|| methods.get(&handler_name).copied());
            let route = nest_prefix_for(&nest_prefixes, call.start)
                .map(|prefix| join_route_paths(prefix, &route))
                .unwrap_or_else(|| normalize_spring_functional_route(&route));
            out.push(RouteCandidate {
                route,
                method: Some(predicate.to_string()),
                handler_id: handler.map(|node| node.aka_id.clone()),
                handler_name: handler.map(|node| node.display_name().to_string()),
            });
        }
    }
    out
}

#[derive(Debug)]
struct NestPrefix {
    start: usize,
    end: usize,
    prefix: String,
}

fn spring_nest_prefixes(text: &str) -> Vec<NestPrefix> {
    let mut out = Vec::new();
    for call in find_call_args(text, "nest") {
        let Some(prefix) = nest_path_prefix(call.args) else {
            continue;
        };
        let Some(open) = text[call.start..].find('(').map(|idx| call.start + idx) else {
            continue;
        };
        let Some(end) = find_matching_paren(text, open) else {
            continue;
        };
        out.push(NestPrefix {
            start: call.start,
            end,
            prefix: normalize_spring_functional_route(&prefix),
        });
    }
    out
}

fn nest_path_prefix(args: &str) -> Option<String> {
    for callee in ["path", "RequestPredicates.path"] {
        for call in find_call_args(args, callee) {
            if let Some(prefix) = first_string_literal(call.args) {
                return Some(prefix);
            }
        }
    }
    None
}

fn nest_prefix_for(prefixes: &[NestPrefix], offset: usize) -> Option<&str> {
    prefixes
        .iter()
        .filter(|prefix| offset >= prefix.start && offset <= prefix.end)
        .max_by_key(|prefix| prefix.start)
        .map(|prefix| prefix.prefix.as_str())
}

fn method_in_package<'a>(
    methods: &HashMap<String, &'a SynthNode>,
    method_name: &str,
    package_name: Option<&str>,
) -> Option<&'a SynthNode> {
    let package_name = package_name?;
    methods
        .get(method_name)
        .copied()
        .filter(|node| strip_node_prefix(&node.qn).starts_with(package_name))
}

fn java_package_name(text: &str) -> Option<String> {
    for line in text.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("package ") else {
            continue;
        };
        return Some(rest.trim_end_matches(';').trim().to_string());
    }
    None
}

fn spring_source_files(repo: &Path, project_sources: &ProjectSourceSet) -> Vec<String> {
    let mut out = if project_sources.has_git_listing() {
        project_sources
            .iter()
            .filter(|path| is_spring_functional_source_path(path))
            .map(str::to_string)
            .collect()
    } else {
        let mut files = Vec::new();
        collect_spring_source_files(repo, repo, project_sources, &mut files);
        files
    };
    out.sort();
    out.dedup();
    out
}

fn collect_spring_source_files(
    repo: &Path,
    dir: &Path,
    project_sources: &ProjectSourceSet,
    out: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Some(name) = path.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        if file_type.is_dir() {
            if is_source_discovery_skip_dir(name) {
                continue;
            }
            collect_spring_source_files(repo, &path, project_sources, out);
        } else if file_type.is_file() {
            let Ok(rel) = path.strip_prefix(repo) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if is_spring_functional_source_path(&rel)
                && project_sources.contains_project_file(repo, &rel)
            {
                out.push(rel);
            }
        }
    }
}

fn is_spring_functional_source_path(path: &str) -> bool {
    matches!(
        Path::new(&path.to_ascii_lowercase())
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("java" | "kt" | "kts" | "scala" | "groovy")
    )
}

fn is_source_discovery_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | "vendor"
            | "vendors"
            | "target"
            | "build"
            | "dist"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".idea"
            | ".vscode"
    )
}

fn java_methods_by_name(nodes: &BTreeMap<String, SynthNode>) -> HashMap<String, &SynthNode> {
    let mut out = HashMap::new();
    for node in nodes.values() {
        if !matches!(node.label.as_str(), "Method" | "Function")
            || node.language.eq_ignore_ascii_case("python")
            || node.file_path.to_ascii_lowercase().ends_with(".py")
        {
            continue;
        }
        out.entry(node.name.clone()).or_insert(node);
    }
    out
}

fn first_string_literal(args: &str) -> Option<String> {
    let idx = args.find(['"', '\''])?;
    read_string_literal(args, idx).map(|(literal, _)| literal)
}

fn handler_method_after(text: &str, predicate_start: usize) -> Option<String> {
    let window_end = (predicate_start + 400).min(text.len());
    let window = &text[predicate_start..window_end];
    let marker = window.find("::")?;
    let after = &window[marker + 2..];
    let name: String = after
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn normalize_spring_functional_route(route: &str) -> String {
    let route = route.trim();
    if route.starts_with('/') {
        route.to_string()
    } else {
        format!("/{route}")
    }
}

fn join_route_paths(prefix: &str, route: &str) -> String {
    let prefix = normalize_spring_functional_route(prefix);
    let route = normalize_spring_functional_route(route);
    if prefix == "/" {
        route
    } else if route == "/" {
        prefix
    } else {
        format!(
            "{}/{}",
            prefix.trim_end_matches('/'),
            route.trim_start_matches('/')
        )
    }
}

fn strip_node_prefix(id_or_qn: &str) -> &str {
    if let Some(rest) = id_or_qn.strip_prefix("cbm:") {
        let mut parts = rest.splitn(2, ':');
        if let (Some(_id), Some(qn)) = (parts.next(), parts.next()) {
            return qn;
        }
    }
    id_or_qn
}
