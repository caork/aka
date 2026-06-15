use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::route_shape::{
    extract_accessed_keys_near_route, fetch_literal_windows, is_route_parameter_segment,
    route_occurrences,
};
use super::{
    feign_client_path, join_route_paths, pick_handler_node, read_repo_text, request_line_path,
    route_nodes_by_file, spring_mapping_path, SynthNode, SynthRoute, SynthRouteConsumer,
};

pub(super) fn attach_route_consumers(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    routes: &mut BTreeMap<(String, String), SynthRoute>,
) {
    if routes.is_empty() {
        return;
    }
    let by_file = route_nodes_by_file(nodes);
    let route_names: Vec<String> = routes.keys().map(|(route, _)| route.clone()).collect();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for (route, node_id) in java_feign_route_consumers(&file_nodes) {
            for candidate in routes.values_mut().filter(|r| r.route == route) {
                if candidate.file_path == file_path {
                    continue;
                }
                candidate.consumers.push(SynthRouteConsumer {
                    node_id: node_id.clone(),
                    keys: Vec::new(),
                    fetch_count: 1,
                });
            }
        }
        for (route, consumer) in route_fetch_consumers(&text, &route_names, &file_nodes) {
            for candidate in routes.values_mut().filter(|r| &r.route == route) {
                if candidate.file_path == file_path {
                    continue;
                }
                candidate.consumers.push(consumer.clone());
            }
        }
    }
    for route in routes.values_mut() {
        route.consumers.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        route.consumers.dedup_by(|a, b| {
            if a.node_id == b.node_id {
                b.fetch_count = b.fetch_count.saturating_add(a.fetch_count);
                b.keys.extend(a.keys.clone());
                b.keys.sort();
                b.keys.dedup();
                true
            } else {
                false
            }
        });
    }
    remove_parent_route_consumers(routes);
}

fn remove_parent_route_consumers(routes: &mut BTreeMap<(String, String), SynthRoute>) {
    let route_names: Vec<String> = routes.values().map(|route| route.route.clone()).collect();
    let mut removals: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for route in routes.values() {
        let parent_routes = parent_routes_for(&route.route, &route_names);
        if parent_routes.is_empty() {
            continue;
        }
        for consumer in &route.consumers {
            for parent in &parent_routes {
                removals
                    .entry(parent.clone())
                    .or_default()
                    .insert(consumer.node_id.clone());
            }
        }
    }
    for route in routes.values_mut() {
        let Some(remove_consumers) = removals.get(&route.route) else {
            continue;
        };
        route
            .consumers
            .retain(|consumer| !remove_consumers.contains(&consumer.node_id));
    }
}

fn parent_routes_for(route: &str, all_routes: &[String]) -> Vec<String> {
    if !route
        .split('/')
        .filter(|segment| !segment.is_empty())
        .any(is_route_parameter_segment)
    {
        return Vec::new();
    }
    let mut parents = Vec::new();
    let mut current = route.trim_end_matches('/').to_string();
    while let Some((parent, _)) = current.rsplit_once('/') {
        if parent.is_empty() {
            break;
        }
        if all_routes.iter().any(|route| route == parent) {
            parents.push(parent.to_string());
        }
        current = parent.to_string();
    }
    parents
}

fn java_feign_route_consumers(nodes: &[&SynthNode]) -> Vec<(String, String)> {
    let mut feign_prefixes: BTreeMap<String, String> = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        let Some(prefix) = feign_client_path(&node.decorators) else {
            continue;
        };
        feign_prefixes.insert(node.aka_id.clone(), prefix.clone());
        feign_prefixes.insert(node.qn.clone(), prefix);
    }

    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        let Some(parent) = node.parent_class.as_ref() else {
            continue;
        };
        let Some(prefix) = feign_prefixes.get(parent) else {
            continue;
        };
        if let Some(route) = node
            .decorators
            .iter()
            .find_map(|decorator| request_line_path(decorator))
        {
            out.push((join_route_paths(prefix, &route), node.aka_id.clone()));
            continue;
        }
        if let Some(method_path) = node
            .route_path
            .clone()
            .or_else(|| spring_mapping_path(&node.decorators))
        {
            out.push((join_route_paths(prefix, &method_path), node.aka_id.clone()));
        }
    }
    out.sort();
    out.dedup();
    out
}

fn route_fetch_consumers<'a>(
    text: &str,
    route_names: &'a [String],
    file_nodes: &[&SynthNode],
) -> Vec<(&'a String, SynthRouteConsumer)> {
    if route_names.is_empty() {
        return Vec::new();
    }
    let fetch_windows = fetch_literal_windows(text);
    if fetch_windows.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for route in route_names {
        let mut consumers: BTreeMap<String, SynthRouteConsumer> = BTreeMap::new();
        for (window_start, window) in &fetch_windows {
            for idx in route_occurrences(window, route) {
                let absolute_idx = window_start + idx;
                let Some(node) = super::node_at_offset(text, file_nodes, absolute_idx)
                    .or_else(|| pick_handler_node(file_nodes))
                else {
                    continue;
                };
                let entry =
                    consumers
                        .entry(node.aka_id.clone())
                        .or_insert_with(|| SynthRouteConsumer {
                            node_id: node.aka_id.clone(),
                            keys: Vec::new(),
                            fetch_count: 0,
                        });
                entry.fetch_count = entry.fetch_count.saturating_add(1);
            }
        }
        let keys = extract_accessed_keys_near_route(text, route);
        for consumer in consumers.values_mut() {
            consumer.keys = keys.clone();
            out.push((route, consumer.clone()));
        }
    }
    out
}
