use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, find_matching_paren, project_code_nodes_by_file, read_repo_text,
    source_annotations_before_node, split_top_level_commas, stable_hash, EdgeRec, NodeRec,
    ProjectSourceSet, SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct SynthPolicy {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) policy_type: String,
    pub(super) subjects: Vec<SynthPolicySubject>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct SynthPolicySubject {
    node_id: String,
    file_path: String,
    strategy: String,
}

impl SynthPolicy {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("policyType".into(), Value::String(self.policy_type.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("policySource".into(), Value::String("source-scan".into()));
        NodeRec {
            id: self.id.clone(),
            label: "Policy".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        self.subjects
            .iter()
            .map(|subject| EdgeRec {
                id: format!(
                    "{}:requires:{:016x}",
                    self.id,
                    stable_hash(&format!("{}|{}", subject.node_id, subject.strategy))
                ),
                source_id: subject.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: "REQUIRES_POLICY".into(),
                confidence: 0.7,
                reason: "aka policy synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "policy-requirement",
                    "policy": self.name,
                    "policyType": self.policy_type,
                    "strategy": subject.strategy,
                    "filePath": subject.file_path,
                })),
            })
            .collect()
    }
}

pub(super) fn synthesize_policies_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> Vec<SynthPolicy> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut policies: BTreeMap<(String, String), SynthPolicy> = BTreeMap::new();
    let mut seen_edges: HashSet<(String, String, String)> = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let text = read_repo_text(repo, &file_path).unwrap_or_default();
        for detection in extract_policy_detections(&text, &file_path, &file_nodes) {
            let key = (detection.policy_type.clone(), detection.name.clone());
            let id = format!(
                "policy:heuristic:{:016x}",
                stable_hash(&format!("{}|{}", detection.policy_type, detection.name))
            );
            let policy = policies.entry(key).or_insert_with(|| SynthPolicy {
                id,
                name: detection.name.clone(),
                policy_type: detection.policy_type.clone(),
                subjects: Vec::new(),
            });
            let edge_key = (
                detection.policy_type,
                detection.name,
                detection.node_id.clone(),
            );
            if !seen_edges.insert(edge_key) {
                continue;
            }
            policy.subjects.push(SynthPolicySubject {
                node_id: detection.node_id,
                file_path: file_path.clone(),
                strategy: detection.strategy,
            });
        }
    }
    let mut out: Vec<SynthPolicy> = policies.into_values().collect();
    for policy in &mut out {
        policy.subjects.sort();
        policy.subjects.dedup();
    }
    out.sort_by(|a, b| {
        a.policy_type
            .cmp(&b.policy_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

#[derive(Debug, Clone)]
struct PolicyDetection {
    name: String,
    policy_type: String,
    node_id: String,
    strategy: String,
}

fn extract_policy_detections(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
) -> Vec<PolicyDetection> {
    let mut out = Vec::new();
    let lower = file_path.to_ascii_lowercase();
    if lower.ends_with(".java")
        || nodes.iter().any(|node| {
            matches!(
                node.language.to_ascii_lowercase().as_str(),
                "java" | "kotlin" | "scala" | "groovy"
            )
        })
    {
        out.extend(extract_jvm_policy_detections(text, nodes));
    }
    if lower.ends_with(".py")
        || nodes
            .iter()
            .any(|node| node.language.eq_ignore_ascii_case("python"))
    {
        out.extend(extract_python_policy_detections(text, nodes));
    }
    out.sort_by(|a, b| {
        a.policy_type
            .cmp(&b.policy_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    out
}

fn extract_jvm_policy_detections(text: &str, nodes: &[&SynthNode]) -> Vec<PolicyDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method" | "Class"))
    {
        for decorator in decorators_for_node(text, node) {
            let Some(name) = decorator_name(&decorator) else {
                continue;
            };
            match name {
                "PreAuthorize" | "PostAuthorize" => {
                    for policy in annotation_string_values(&decorator, &["value"]) {
                        out.push(PolicyDetection {
                            name: policy,
                            policy_type: "spring-expression".into(),
                            node_id: node.aka_id.clone(),
                            strategy: format!("java-spring-{}", name.to_ascii_lowercase()),
                        });
                    }
                }
                "Secured" | "RolesAllowed" => {
                    for role in annotation_string_values(&decorator, &["value"]) {
                        out.push(PolicyDetection {
                            name: role,
                            policy_type: "role".into(),
                            node_id: node.aka_id.clone(),
                            strategy: format!("java-security-{}", name.to_ascii_lowercase()),
                        });
                    }
                }
                _ => {}
            }
        }
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

fn extract_python_policy_detections(text: &str, nodes: &[&SynthNode]) -> Vec<PolicyDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        for decorator in decorators_for_node(text, node) {
            let normalized = decorator.trim().trim_start_matches('@');
            let decorator_lower = normalized.to_ascii_lowercase();
            if decorator_lower.contains("permission_required") {
                for name in decorator_literal_values(normalized) {
                    out.push(PolicyDetection {
                        name,
                        policy_type: "permission".into(),
                        node_id: node.aka_id.clone(),
                        strategy: "python-permission-required".into(),
                    });
                }
            } else if decorator_lower.contains("user_passes_test") {
                for name in decorator_callable_values(normalized) {
                    out.push(PolicyDetection {
                        name,
                        policy_type: "predicate".into(),
                        node_id: node.aka_id.clone(),
                        strategy: "python-user-passes-test".into(),
                    });
                }
            }
        }
    }
    out.extend(extract_fastapi_security_dependencies(text, nodes));
    out
}

fn extract_fastapi_security_dependencies(text: &str, nodes: &[&SynthNode]) -> Vec<PolicyDetection> {
    let mut out = Vec::new();
    for callee in ["Depends", "Security"] {
        for call in find_call_args(text, callee) {
            let Some(node) = super::node_at_offset(text, nodes, call.start) else {
                continue;
            };
            let args = split_top_level_commas(call.args);
            let Some(first) = args.first() else {
                continue;
            };
            if let Some(name) = callable_name(first) {
                out.push(PolicyDetection {
                    name,
                    policy_type: "dependency".into(),
                    node_id: node.aka_id.clone(),
                    strategy: format!("python-fastapi-{}", callee.to_ascii_lowercase()),
                });
            }
            for scope in keyword_literal_array(call.args, "scopes") {
                out.push(PolicyDetection {
                    name: scope,
                    policy_type: "scope".into(),
                    node_id: node.aka_id.clone(),
                    strategy: format!("python-fastapi-{}-scope", callee.to_ascii_lowercase()),
                });
            }
        }
    }
    out
}

fn annotation_string_values(annotation: &str, keys: &[&str]) -> Vec<String> {
    let Some(open) = annotation.find('(') else {
        return Vec::new();
    };
    let close = find_matching_paren(annotation, open).unwrap_or(annotation.len());
    let args = &annotation[open + 1..close];
    let mut values = Vec::new();
    for part in split_top_level_commas(args) {
        let part = part.trim();
        let value = if let Some((key, value)) = part.split_once('=') {
            if !keys.iter().any(|expected| key.trim().ends_with(expected)) {
                continue;
            }
            value.trim()
        } else if keys.contains(&"value") {
            part
        } else {
            continue;
        };
        values.extend(string_literals(value));
    }
    normalize_policy_names(values)
}

fn decorator_literal_values(decorator: &str) -> Vec<String> {
    let Some(open) = decorator.find('(') else {
        return Vec::new();
    };
    let close = find_matching_paren(decorator, open).unwrap_or(decorator.len());
    normalize_policy_names(string_literals(&decorator[open + 1..close]))
}

fn decorator_callable_values(decorator: &str) -> Vec<String> {
    let Some(open) = decorator.find('(') else {
        return Vec::new();
    };
    let close = find_matching_paren(decorator, open).unwrap_or(decorator.len());
    let args = split_top_level_commas(&decorator[open + 1..close]);
    args.first()
        .and_then(|arg| callable_name(arg))
        .into_iter()
        .collect()
}

fn keyword_literal_array(args: &str, keyword: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in split_top_level_commas(args) {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key.trim() != keyword {
            continue;
        }
        out.extend(string_literals(value));
    }
    normalize_policy_names(out)
}

fn callable_name(expr: &str) -> Option<String> {
    let expr = expr
        .trim()
        .trim_start_matches("lambda ")
        .split_once('(')
        .map(|(name, _)| name)
        .unwrap_or(expr.trim())
        .trim();
    if expr.is_empty()
        || expr.starts_with('"')
        || expr.starts_with('\'')
        || expr == "None"
        || expr.contains('=')
    {
        return None;
    }
    let name = expr.rsplit('.').next().unwrap_or(expr);
    is_policy_name(name).then(|| name.to_string())
}

fn string_literals(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some(byte) = text.as_bytes().get(idx).copied() else {
            break;
        };
        if matches!(byte, b'\'' | b'"' | b'`') {
            if let Some((literal, end)) = read_raw_string_literal(text, idx) {
                if is_policy_name(&literal) {
                    values.push(literal);
                }
                idx = end;
                continue;
            }
        }
        idx += 1;
    }
    values
}

fn normalize_policy_names(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| is_policy_name(value))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn decorator_name(decorator: &str) -> Option<&str> {
    let text = decorator.trim().trim_start_matches('@');
    let name = text.split_once('(').map(|(name, _)| name).unwrap_or(text);
    Some(name.rsplit('.').next().unwrap_or(name).trim()).filter(|name| !name.is_empty())
}

fn read_raw_string_literal(text: &str, start: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let quote = *bytes.get(start)?;
    if !matches!(quote, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut out = String::new();
    let mut escape = false;
    let mut i = start + 1;
    while i < bytes.len() {
        let b = bytes[i];
        if escape {
            let ch = text[i..].chars().next()?;
            out.push(ch);
            escape = false;
            i += ch.len_utf8();
            continue;
        }
        if b == b'\\' {
            escape = true;
        } else if b == quote {
            return Some((out, i + 1));
        } else {
            let ch = text[i..].chars().next()?;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        i += 1;
    }
    None
}

fn is_policy_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 220
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '_' | '-' | '.' | ':' | '/' | '(' | ')' | '\'' | '"' | ' ' | '#'
                )
        })
        && value.chars().any(|ch| ch.is_ascii_alphabetic())
}
