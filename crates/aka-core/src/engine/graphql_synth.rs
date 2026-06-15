use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_matching_paren, merge_strings, nodes_by_file, process_ids_for_entry, read_repo_text,
    read_string_literal, skip_ws, stable_hash, EdgeRec, NodeRec, ProjectSourceSet, SynthNode,
    SynthProcess,
};

#[derive(Debug, Clone)]
pub(super) struct SynthGraphqlOperation {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) operation_type: String,
    pub(super) file_path: String,
    pub(super) handler_id: String,
    pub(super) handler_name: String,
    pub(super) process_ids: Vec<String>,
}

impl SynthGraphqlOperation {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("operationName".into(), Value::String(self.name.clone()));
        properties.insert(
            "operationType".into(),
            Value::String(self.operation_type.clone()),
        );
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("handlerId".into(), Value::String(self.handler_id.clone()));
        properties.insert(
            "handlerName".into(),
            Value::String(self.handler_name.clone()),
        );
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("graphqlSource".into(), Value::String("source-scan".into()));
        NodeRec {
            id: self.id.clone(),
            label: "GraphQL".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = vec![EdgeRec {
            id: format!("{}:handles:{:016x}", self.id, stable_hash(&self.handler_id)),
            source_id: self.handler_id.clone(),
            target_id: self.id.clone(),
            edge_type: "HANDLES_GRAPHQL".into(),
            confidence: 0.64,
            reason: "aka GraphQL operation synthesis".into(),
            step: None,
            evidence: Some(json!({
                "source": "aka-cbm-synth",
                "kind": "graphql-handler",
                "operation": self.name,
                "operationType": self.operation_type,
            })),
        }];
        for process_id in &self.process_ids {
            out.push(EdgeRec {
                id: format!("{}:entry-process:{:016x}", self.id, stable_hash(process_id)),
                source_id: self.id.clone(),
                target_id: process_id.clone(),
                edge_type: "ENTRY_POINT_OF".into(),
                confidence: 0.5,
                reason: "aka GraphQL process linkage".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "graphql-entry-process",
                    "operation": self.name,
                    "operationType": self.operation_type,
                })),
            });
        }
        out
    }
}

pub(super) fn synthesize_graphql_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    processes: &[SynthProcess],
) -> Vec<SynthGraphqlOperation> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = nodes_by_file(nodes);
    let mut operations: BTreeMap<(String, String, String), SynthGraphqlOperation> = BTreeMap::new();
    for (file_path, file_nodes) in by_file {
        if !project_sources.contains_project_file(repo, &file_path)
            || !is_graphql_candidate_file(&file_path, &file_nodes)
        {
            continue;
        }
        let text = read_repo_text(repo, &file_path).unwrap_or_default();
        for node in file_nodes
            .into_iter()
            .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
        {
            for detection in detect_graphql_operations(&text, node) {
                let key = (
                    detection.operation_type.clone(),
                    detection.name.clone(),
                    file_path.clone(),
                );
                match operations.get_mut(&key) {
                    Some(existing) => {
                        merge_strings(
                            &mut existing.process_ids,
                            &process_ids_for_entry(processes, &file_path, Some(&node.aka_id)),
                        );
                    }
                    None => {
                        operations.insert(
                            key,
                            SynthGraphqlOperation {
                                id: format!(
                                    "graphql:heuristic:{:016x}",
                                    stable_hash(&format!(
                                        "{}|{}|{}",
                                        detection.operation_type, detection.name, file_path
                                    ))
                                ),
                                name: detection.name,
                                operation_type: detection.operation_type,
                                file_path: file_path.clone(),
                                handler_id: node.aka_id.clone(),
                                handler_name: node.name.clone(),
                                process_ids: process_ids_for_entry(
                                    processes,
                                    &file_path,
                                    Some(&node.aka_id),
                                ),
                            },
                        );
                    }
                }
            }
        }
    }
    let mut out: Vec<_> = operations.into_values().collect();
    out.sort_by(|a, b| {
        a.operation_type
            .cmp(&b.operation_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    out
}

#[derive(Debug, Clone)]
struct GraphqlDetection {
    name: String,
    operation_type: String,
}

fn is_graphql_candidate_file(file_path: &str, nodes: &[&SynthNode]) -> bool {
    let lower = file_path.to_ascii_lowercase();
    lower.ends_with(".graphql")
        || lower.ends_with(".gql")
        || nodes.iter().any(|node| {
            matches!(
                node.language.to_ascii_lowercase().as_str(),
                "java" | "kotlin" | "scala" | "groovy" | "python"
            )
        })
}

fn detect_graphql_operations(text: &str, node: &SynthNode) -> Vec<GraphqlDetection> {
    let mut out = Vec::new();
    for decorator in &node.decorators {
        if let Some(detection) = detect_jvm_graphql_annotation(decorator, &node.name) {
            out.push(detection);
        }
        if let Some(detection) = detect_python_graphql_decorator(decorator, &node.name) {
            out.push(detection);
        }
    }
    out.extend(detect_python_graphql_resolver(text, node));
    out.sort_by(|a, b| {
        a.operation_type
            .cmp(&b.operation_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    out.dedup_by(|a, b| a.operation_type == b.operation_type && a.name == b.name);
    out
}

fn detect_jvm_graphql_annotation(annotation: &str, fallback: &str) -> Option<GraphqlDetection> {
    let name = decorator_name(annotation)?;
    let operation_type = match name {
        "QueryMapping" => "query",
        "MutationMapping" => "mutation",
        "SubscriptionMapping" => "subscription",
        "SchemaMapping" | "BatchMapping" => "field",
        _ => return None,
    };
    let operation = annotation_string_value(annotation, &["name", "value", "field"])
        .unwrap_or_else(|| fallback.to_string());
    Some(GraphqlDetection {
        name: operation,
        operation_type: operation_type.into(),
    })
}

fn detect_python_graphql_decorator(decorator: &str, fallback: &str) -> Option<GraphqlDetection> {
    let normalized = decorator.trim().trim_start_matches('@');
    let lower = normalized.to_ascii_lowercase();
    let operation_type = if lower.contains("mutation") {
        "mutation"
    } else if lower.contains("subscription") {
        "subscription"
    } else if lower.contains("field")
        || lower.contains("query")
        || lower.contains("strawberry.")
        || lower.contains("graphene.")
    {
        "query"
    } else {
        return None;
    };
    let name = annotation_string_value(normalized, &["name", "field_name"]).unwrap_or_else(|| {
        fallback
            .strip_prefix("resolve_")
            .unwrap_or(fallback)
            .to_string()
    });
    Some(GraphqlDetection {
        name,
        operation_type: operation_type.into(),
    })
}

fn detect_python_graphql_resolver(text: &str, node: &SynthNode) -> Vec<GraphqlDetection> {
    if !node.file_path.ends_with(".py") && !node.language.eq_ignore_ascii_case("python") {
        return Vec::new();
    }
    let lower = text.to_ascii_lowercase();
    if !(lower.contains("graphql")
        || lower.contains("graphene")
        || lower.contains("strawberry")
        || lower.contains("ariadne"))
    {
        return Vec::new();
    }
    if let Some(name) = node.name.strip_prefix("resolve_") {
        return vec![GraphqlDetection {
            name: name.to_string(),
            operation_type: "query".into(),
        }];
    }
    if node.name.starts_with("mutate") || node.name.ends_with("_mutation") {
        return vec![GraphqlDetection {
            name: node.name.clone(),
            operation_type: "mutation".into(),
        }];
    }
    Vec::new()
}

fn decorator_name(decorator: &str) -> Option<&str> {
    let text = decorator.trim().trim_start_matches('@');
    let end = text.find('(').unwrap_or(text.len());
    text[..end].rsplit('.').next().map(str::trim)
}

fn annotation_string_value(annotation: &str, keys: &[&str]) -> Option<String> {
    let open = annotation.find('(')?;
    let close = find_matching_paren(annotation, open).unwrap_or(annotation.len());
    let args = &annotation[open + 1..close];
    for key in keys {
        if let Some(value) = keyed_string_arg(args, key) {
            return Some(value);
        }
    }
    first_string_literal(args)
}

fn keyed_string_arg(args: &str, key: &str) -> Option<String> {
    let mut offset = 0usize;
    while let Some(pos) = args[offset..].find(key) {
        let idx = offset + pos;
        let before = idx
            .checked_sub(1)
            .and_then(|i| args.as_bytes().get(i))
            .copied()
            .map(char::from);
        let after = args
            .as_bytes()
            .get(idx + key.len())
            .copied()
            .map(char::from);
        if before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            offset = idx + key.len();
            continue;
        }
        let sep = skip_ws(args, idx + key.len());
        let value_start = if matches!(args.as_bytes().get(sep), Some(b'=') | Some(b':')) {
            skip_ws(args, sep + 1)
        } else {
            offset = idx + key.len();
            continue;
        };
        if let Some((value, _)) = read_string_literal(args, value_start) {
            return Some(value);
        }
        offset = idx + key.len();
    }
    None
}

fn first_string_literal(args: &str) -> Option<String> {
    let mut offset = 0usize;
    while offset < args.len() {
        if let Some((value, _)) = read_string_literal(args, offset) {
            return Some(value);
        }
        offset += args[offset..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
    }
    None
}
