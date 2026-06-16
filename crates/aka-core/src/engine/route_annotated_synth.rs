use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

use super::route_python_prefix_synth::python_route_prefixes_for_decorators;
use super::{
    first_route_literal, is_ident_start, join_route_paths, literal_occurrences,
    normalize_route_literal, read_repo_text, read_string_literal, semantic_owner_qn,
    source_annotations_before_node, spring_mapping_path, PythonRoutePrefixes, RouteCandidate,
    SynthNode,
};

pub(super) fn extract_annotated_routes(
    text: &str,
    nodes: &[&SynthNode],
    python_prefixes: Option<&PythonRoutePrefixes>,
    java_interface_routes: &HashMap<String, Vec<RouteCandidate>>,
) -> Vec<RouteCandidate> {
    let java_method_route_names: BTreeSet<String> = nodes
        .iter()
        .filter(|node| {
            matches!(node.label.as_str(), "Method")
                && !is_python_route_node(node)
                && (node.route_path.is_some()
                    || spring_mapping_path(&decorators_for_node(text, node)).is_some())
        })
        .map(|node| node.name.clone())
        .collect();
    let mut class_prefixes: BTreeMap<String, String> = BTreeMap::new();
    let mut owner_labels: BTreeMap<String, String> = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        owner_labels.insert(node.aka_id.clone(), node.label.clone());
        owner_labels.insert(node.qn.clone(), node.label.clone());
        let Some(prefix) = spring_mapping_path(&decorators_for_node(text, node)) else {
            continue;
        };
        class_prefixes.insert(node.aka_id.clone(), prefix.clone());
        class_prefixes.insert(node.qn.clone(), prefix);
    }

    let mut routes = Vec::new();
    for node in nodes {
        let decorators = decorators_for_node(text, node);
        let python_decorator_route = is_python_route_node(node)
            .then(|| python_decorator_route(&decorators))
            .flatten();
        let method_path = node
            .route_path
            .clone()
            .or_else(|| {
                python_decorator_route
                    .as_ref()
                    .map(|route| route.path.clone())
            })
            .or_else(|| spring_mapping_path(&decorators));
        let Some(method_path) = method_path else {
            continue;
        };
        let route_methods = node
            .route_method
            .clone()
            .or_else(|| spring_mapping_method(&decorators))
            .map(|method| vec![Some(method)])
            .or_else(|| python_decorator_route.map(|route| route.methods))
            .unwrap_or_else(|| vec![None]);
        if !matches!(node.label.as_str(), "Function" | "Method") {
            if !method_path.is_empty() {
                for route_method in &route_methods {
                    routes.push(RouteCandidate {
                        route: normalize_route_literal(&method_path),
                        method: route_method.clone(),
                        handler_id: None,
                        handler_name: None,
                    });
                }
            }
            continue;
        }
        if !is_python_route_node(node) {
            if node.label == "Function" && java_method_route_names.contains(&node.name) {
                continue;
            }
            if node
                .parent_class
                .as_ref()
                .and_then(|parent| owner_labels.get(parent))
                .is_some_and(|label| label == "Interface")
            {
                continue;
            }
        }
        let prefixes: Vec<String> = if is_python_route_node(node) {
            python_route_prefixes_for_decorators(python_prefixes, &decorators)
        } else {
            vec![node
                .parent_class
                .as_ref()
                .and_then(|parent| class_prefixes.get(parent))
                .map(String::as_str)
                .unwrap_or("")
                .to_string()]
        };
        for prefix in prefixes {
            for route_method in &route_methods {
                routes.push(RouteCandidate {
                    route: join_route_paths(&prefix, &method_path),
                    method: route_method.clone(),
                    handler_id: Some(node.aka_id.clone()),
                    handler_name: Some(node.display_name().to_string()),
                });
            }
        }
    }
    routes.extend(inherited_java_interface_routes(
        text,
        nodes,
        java_interface_routes,
    ));
    routes
}

pub(super) fn java_interface_routes_by_method(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> HashMap<String, Vec<RouteCandidate>> {
    let mut owner_labels: BTreeMap<String, String> = BTreeMap::new();
    let mut class_prefixes: BTreeMap<String, String> = BTreeMap::new();
    let mut type_aliases: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in nodes
        .values()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        let aliases = java_type_aliases(node);
        for alias in &aliases {
            owner_labels.insert(alias.clone(), node.label.clone());
        }
        type_aliases.insert(node.aka_id.clone(), aliases);
        let text = read_repo_text(repo, &node.file_path).unwrap_or_default();
        if let Some(prefix) = spring_mapping_path(&decorators_for_node(&text, node)) {
            for alias in java_type_aliases(node) {
                class_prefixes.insert(alias, prefix.clone());
            }
        }
    }

    let mut out: HashMap<String, Vec<RouteCandidate>> = HashMap::new();
    for node in nodes.values().filter(|node| {
        matches!(node.label.as_str(), "Function" | "Method")
            && !is_python_route_node(node)
            && node
                .parent_class
                .as_ref()
                .and_then(|parent| owner_labels.get(parent))
                .is_some_and(|label| label == "Interface")
    }) {
        let text = read_repo_text(repo, &node.file_path).unwrap_or_default();
        let decorators = decorators_for_node(&text, node);
        let Some(method_path) = node
            .route_path
            .clone()
            .or_else(|| spring_mapping_path(&decorators))
        else {
            continue;
        };
        let prefix = node
            .parent_class
            .as_ref()
            .and_then(|parent| class_prefixes.get(parent))
            .map(String::as_str)
            .unwrap_or("");
        let candidate = RouteCandidate {
            route: join_route_paths(prefix, &method_path),
            method: node
                .route_method
                .clone()
                .or_else(|| spring_mapping_method(&decorators)),
            handler_id: Some(node.aka_id.clone()),
            handler_name: Some(node.display_name().to_string()),
        };
        for key in java_method_route_keys(node) {
            out.entry(key).or_default().push(candidate.clone());
        }
    }
    let interface_routes = out.clone();
    for class_node in nodes
        .values()
        .filter(|node| matches!(node.label.as_str(), "Class"))
    {
        let Some(text) = read_repo_text(repo, &class_node.file_path) else {
            continue;
        };
        let implemented = java_implemented_interfaces(&text, &class_node.name);
        if implemented.is_empty() {
            continue;
        }
        let class_aliases = type_aliases
            .get(&class_node.aka_id)
            .cloned()
            .unwrap_or_else(|| java_type_aliases(class_node));
        for method in nodes.values().filter(|node| {
            let method_text = read_repo_text(repo, &node.file_path).unwrap_or_default();
            matches!(node.label.as_str(), "Function" | "Method")
                && node
                    .parent_class
                    .as_ref()
                    .is_some_and(|parent| class_aliases.iter().any(|alias| alias == parent))
                && node.route_path.is_none()
                && spring_mapping_path(&decorators_for_node(&method_text, node)).is_none()
        }) {
            for iface in &implemented {
                let possible = possible_interface_method_ids_from_name(
                    &class_node.qn,
                    &class_node.name,
                    iface,
                    &method.name,
                );
                for iface_method in possible {
                    if let Some(routes) = interface_routes.get(&iface_method) {
                        out.entry(method.aka_id.clone())
                            .or_default()
                            .extend(routes.clone());
                        out.entry(method.qn.clone())
                            .or_default()
                            .extend(routes.clone());
                    }
                }
            }
        }
    }
    for routes in out.values_mut() {
        dedup_route_candidates(routes);
    }
    out
}

fn decorators_for_node(text: &str, node: &SynthNode) -> Vec<String> {
    let mut decorators = node.decorators.clone();
    decorators.extend(source_annotations_before_node(text, node));
    decorators.sort();
    decorators.dedup();
    decorators
}

fn spring_mapping_method(decorators: &[String]) -> Option<String> {
    for decorator in decorators {
        let name = decorator
            .trim()
            .trim_start_matches('@')
            .split_once('(')
            .map(|(name, _)| name)
            .unwrap_or_else(|| decorator.trim().trim_start_matches('@'))
            .rsplit('.')
            .next()
            .unwrap_or("")
            .trim();
        let method = match name {
            "GetMapping" => "GET",
            "PostMapping" => "POST",
            "PutMapping" => "PUT",
            "DeleteMapping" => "DELETE",
            "PatchMapping" => "PATCH",
            "RequestMapping" => {
                let args = decorator_args(decorator).unwrap_or("");
                if let Some(method) = spring_request_method_arg(args) {
                    return Some(method);
                }
                continue;
            }
            _ => continue,
        };
        return Some(method.to_string());
    }
    None
}

fn spring_request_method_arg(args: &str) -> Option<String> {
    for method in ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"] {
        if args.contains(&format!("RequestMethod.{method}")) {
            return Some(method.to_string());
        }
    }
    None
}

fn java_type_aliases(node: &SynthNode) -> Vec<String> {
    let mut aliases = vec![node.aka_id.clone(), node.qn.clone()];
    let qn = strip_aka_node_prefix(&node.qn);
    aliases.extend(possible_java_type_qns(qn));
    aliases.push(format!("{}.{name}", qn, name = node.name));
    if let Some((_, simple)) = qn.rsplit_once('.') {
        aliases.push(simple.to_string());
        aliases.push(format!("{}.{}", simple, node.name));
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn java_method_route_keys(node: &SynthNode) -> Vec<String> {
    let mut keys = vec![node.aka_id.clone(), node.qn.clone()];
    let qn = strip_aka_node_prefix(&node.qn);
    keys.push(qn.to_string());
    if let Some(parent) = node.parent_class.as_ref() {
        for owner in possible_java_type_qns(parent) {
            keys.push(format!("{owner}.{method}", method = node.name));
        }
    }
    if let Some(owner) = semantic_owner_qn(qn, &node.name) {
        keys.push(format!("{owner}.{method}", method = node.name));
    }
    keys.sort();
    keys.dedup();
    keys
}

fn possible_java_type_qns(type_id: &str) -> Vec<String> {
    let mut out = Vec::new();
    let qn = strip_aka_node_prefix(type_id);
    out.push(qn.to_string());
    if let Some((pkg, simple)) = qn.rsplit_once('.') {
        if let Some((base, duplicate)) = pkg.rsplit_once('.') {
            if duplicate == simple {
                out.push(pkg.to_string());
                out.push(base.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn java_implemented_interfaces(text: &str, class_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for class_pos in literal_occurrences(text, class_name) {
        let tail = &text[class_pos + class_name.len()..];
        let Some(implements_pos) = tail.find("implements") else {
            continue;
        };
        if tail[..implements_pos].contains('{') || tail[..implements_pos].contains(';') {
            continue;
        }
        let after = &tail[implements_pos + "implements".len()..];
        let end = after
            .find('{')
            .or_else(|| after.find("extends"))
            .unwrap_or(after.len());
        for raw in after[..end].split(',') {
            let name = raw
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim()
                .trim_end_matches('{');
            let name = name
                .split('<')
                .next()
                .unwrap_or(name)
                .rsplit('.')
                .next()
                .unwrap_or(name);
            if name.chars().next().is_some_and(is_ident_start) {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn inherited_java_interface_routes(
    text: &str,
    nodes: &[&SynthNode],
    interface_method_routes: &HashMap<String, Vec<RouteCandidate>>,
) -> Vec<RouteCandidate> {
    if interface_method_routes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for node in nodes.iter().filter(|node| {
        matches!(node.label.as_str(), "Function" | "Method")
            && !is_python_route_node(node)
            && node.route_path.is_none()
            && spring_mapping_path(&decorators_for_node(text, node)).is_none()
    }) {
        let mut inherited = Vec::new();
        for iface in possible_interface_method_ids(node) {
            if let Some(routes) = interface_method_routes.get(&iface) {
                inherited.extend(routes.clone());
            }
        }
        inherited.sort_by(|a, b| a.route.cmp(&b.route).then_with(|| a.method.cmp(&b.method)));
        inherited.dedup_by(|a, b| a.route == b.route && a.method == b.method);
        for route in inherited {
            out.push(RouteCandidate {
                route: route.route,
                method: route.method,
                handler_id: Some(node.aka_id.clone()),
                handler_name: Some(node.display_name().to_string()),
            });
        }
    }
    out
}

fn possible_interface_method_ids(node: &SynthNode) -> Vec<String> {
    let mut out = Vec::new();
    let Some(owner) = node.parent_class.as_ref() else {
        return out;
    };
    for iface_owner in possible_interface_owner_ids(owner) {
        out.push(format!("{iface_owner}.{}", node.name));
        if let Some(stripped) = iface_owner.strip_prefix("cbm:") {
            let mut parts = stripped.splitn(2, ':');
            if let (Some(id), Some(qn)) = (parts.next(), parts.next()) {
                out.push(format!("cbm:{id}:{qn}.{}", node.name));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn possible_interface_method_ids_from_name(
    class_owner_qn: &str,
    class_name: &str,
    interface_name: &str,
    method_name: &str,
) -> Vec<String> {
    let owner_qn = strip_aka_node_prefix(class_owner_qn);
    let pkg = owner_qn
        .strip_suffix(class_name)
        .and_then(|prefix| prefix.strip_suffix('.'))
        .unwrap_or_else(|| owner_qn.rsplit_once('.').map(|(pkg, _)| pkg).unwrap_or(""));
    let iface_qn = if pkg.is_empty() {
        interface_name.to_string()
    } else {
        format!("{pkg}.{interface_name}")
    };
    let mut out = vec![format!("{iface_qn}.{method_name}")];
    for class_owner in possible_java_type_qns(class_owner_qn) {
        if let Some((pkg, _)) = class_owner.rsplit_once('.') {
            out.push(format!("{pkg}.{interface_name}.{method_name}"));
        }
    }
    out.extend(
        possible_interface_owner_ids(class_owner_qn)
            .into_iter()
            .map(|owner| format!("{owner}.{method_name}")),
    );
    out.sort();
    out.dedup();
    out
}

fn possible_interface_owner_ids(owner: &str) -> Vec<String> {
    let mut out = Vec::new();
    let owner_qn = strip_aka_node_prefix(owner);
    let Some((pkg, class_name)) = owner_qn.rsplit_once('.') else {
        return out;
    };
    for suffix in [
        "Controller",
        "ControllerImpl",
        "Resource",
        "ResourceImpl",
        "Endpoint",
    ] {
        if let Some(base) = class_name.strip_suffix(suffix) {
            if !base.is_empty() {
                out.push(format!("{pkg}.{base}Api"));
                out.push(format!("{pkg}.{base}Controller"));
                out.push(format!("{pkg}.{base}Resource"));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn strip_aka_node_prefix(id_or_qn: &str) -> &str {
    if let Some(rest) = id_or_qn.strip_prefix("cbm:") {
        let mut parts = rest.splitn(2, ':');
        if let (Some(_id), Some(qn)) = (parts.next(), parts.next()) {
            return qn;
        }
    }
    id_or_qn
}

fn decorator_args(text: &str) -> Option<&str> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    (close > open).then_some(&text[open + 1..close])
}

fn is_python_route_node(node: &SynthNode) -> bool {
    node.language.eq_ignore_ascii_case("python")
        || node.file_path.to_ascii_lowercase().ends_with(".py")
}

#[derive(Debug, Clone)]
struct PythonDecoratorRoute {
    path: String,
    methods: Vec<Option<String>>,
}

fn python_decorator_route(decorators: &[String]) -> Option<PythonDecoratorRoute> {
    decorators
        .iter()
        .find_map(|decorator| python_decorator_route_one(decorator))
}

fn python_decorator_route_one(decorator: &str) -> Option<PythonDecoratorRoute> {
    let text = decorator.trim().trim_start_matches('@');
    let (_receiver, method_part) = text.split_once('.')?;
    let method = method_part
        .split_once('(')
        .map(|(name, _)| name)
        .unwrap_or(method_part);
    let method = match method {
        "get" => Some(vec![Some("GET".to_string())]),
        "post" => Some(vec![Some("POST".to_string())]),
        "put" => Some(vec![Some("PUT".to_string())]),
        "patch" => Some(vec![Some("PATCH".to_string())]),
        "delete" => Some(vec![Some("DELETE".to_string())]),
        "head" => Some(vec![Some("HEAD".to_string())]),
        "options" => Some(vec![Some("OPTIONS".to_string())]),
        "websocket" | "websocket_route" => Some(vec![Some("WEBSOCKET".to_string())]),
        "api_route" | "route" => None,
        _ => return None,
    };
    let args = decorator_args(text).unwrap_or("");
    let path = first_route_literal(args).map(|path| normalize_route_literal(&path))?;
    let methods = method
        .or_else(|| python_methods_arg(args).map(|methods| methods.into_iter().map(Some).collect()))
        .unwrap_or_else(|| vec![None]);
    Some(PythonDecoratorRoute { path, methods })
}

fn python_methods_arg(args: &str) -> Option<Vec<String>> {
    let needle = "methods=";
    let pos = args.find(needle)?;
    let mut rest = args[pos + needle.len()..].trim_start();
    if let Some(list) = rest.strip_prefix('[') {
        let end = list.find(']')?;
        rest = &list[..end];
    }
    let mut methods = Vec::new();
    let mut offset = 0usize;
    while let Some(rel_start) = rest[offset..].find(['"', '\'']) {
        let start = offset + rel_start;
        let Some((literal, end)) = read_string_literal(rest, start) else {
            break;
        };
        methods.push(literal.to_ascii_uppercase());
        offset = end;
    }
    (!methods.is_empty()).then_some(methods)
}

pub(super) fn dedup_route_candidates(candidates: &mut Vec<RouteCandidate>) {
    candidates.sort_by(|a, b| {
        a.route
            .cmp(&b.route)
            .then_with(|| a.method.cmp(&b.method))
            .then_with(|| a.handler_id.cmp(&b.handler_id))
    });
    candidates.dedup_by(|a, b| {
        if a.route == b.route && (a.method == b.method || a.method.is_none() || b.method.is_none())
        {
            if b.method.is_none() {
                b.method = a.method.clone();
            }
            if b.handler_id.is_none() {
                b.handler_id = a.handler_id.clone();
                b.handler_name = a.handler_name.clone();
            }
            true
        } else {
            false
        }
    });
}
