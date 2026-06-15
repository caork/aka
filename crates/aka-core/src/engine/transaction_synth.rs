use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, node_at_offset, pick_handler_node, project_code_nodes_by_file, read_repo_text,
    stable_hash, EdgeRec, NodeRec, ProjectSourceSet, SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct SynthTransaction {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) manager: String,
    pub(super) propagation: Option<String>,
    pub(super) isolation: Option<String>,
    pub(super) read_only: Option<bool>,
    pub(super) endpoints: Vec<SynthTransactionEndpoint>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct SynthTransactionEndpoint {
    pub(super) node_id: String,
    file_path: String,
    strategy: String,
}

impl SynthTransaction {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("manager".into(), Value::String(self.manager.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert(
            "transactionSource".into(),
            Value::String("source-scan".into()),
        );
        if let Some(propagation) = &self.propagation {
            properties.insert("propagation".into(), Value::String(propagation.clone()));
        }
        if let Some(isolation) = &self.isolation {
            properties.insert("isolation".into(), Value::String(isolation.clone()));
        }
        if let Some(read_only) = self.read_only {
            properties.insert("readOnly".into(), Value::Bool(read_only));
        }
        NodeRec {
            id: self.id.clone(),
            label: "Transaction".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        self.endpoints
            .iter()
            .map(|endpoint| EdgeRec {
                id: format!(
                    "{}:boundary:{:016x}",
                    self.id,
                    stable_hash(&format!("{}|{}", endpoint.node_id, endpoint.strategy))
                ),
                source_id: endpoint.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: "HAS_TRANSACTION_BOUNDARY".into(),
                confidence: 0.72,
                reason: "aka transaction synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "transaction-boundary",
                    "manager": self.manager,
                    "transaction": self.name,
                    "strategy": endpoint.strategy,
                    "filePath": endpoint.file_path,
                })),
            })
            .collect()
    }
}

pub(super) fn synthesize_transactions_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> Vec<SynthTransaction> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut transactions: BTreeMap<String, SynthTransaction> = BTreeMap::new();
    let mut seen_edges: BTreeSet<(String, String)> = BTreeSet::new();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for detection in extract_transaction_detections(&text, &file_path, &file_nodes) {
            let id = format!(
                "transaction:heuristic:{:016x}",
                stable_hash(&detection.node_id)
            );
            if !seen_edges.insert((id.clone(), detection.node_id.clone())) {
                continue;
            }
            let tx = transactions
                .entry(id.clone())
                .or_insert_with(|| SynthTransaction {
                    id,
                    name: detection.name.clone(),
                    manager: detection.manager.clone(),
                    propagation: detection.propagation.clone(),
                    isolation: detection.isolation.clone(),
                    read_only: detection.read_only,
                    endpoints: Vec::new(),
                });
            tx.endpoints.push(SynthTransactionEndpoint {
                node_id: detection.node_id,
                file_path: file_path.clone(),
                strategy: detection.strategy,
            });
        }
    }
    let mut out: Vec<SynthTransaction> = transactions.into_values().collect();
    for tx in &mut out {
        tx.endpoints.sort();
        tx.endpoints.dedup();
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    out
}

#[derive(Debug, Clone)]
struct TransactionDetection {
    name: String,
    manager: String,
    propagation: Option<String>,
    isolation: Option<String>,
    read_only: Option<bool>,
    node_id: String,
    strategy: String,
}

fn extract_transaction_detections(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
) -> Vec<TransactionDetection> {
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
        out.extend(extract_jvm_transactions(nodes));
    }
    if lower.ends_with(".py")
        || nodes
            .iter()
            .any(|node| node.language.eq_ignore_ascii_case("python"))
    {
        out.extend(extract_python_transactions(text, nodes));
    }
    out.sort_by(|a, b| {
        a.manager
            .cmp(&b.manager)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out
}

fn extract_jvm_transactions(nodes: &[&SynthNode]) -> Vec<TransactionDetection> {
    let mut out = Vec::new();
    let class_tx = class_transaction_annotations(nodes);
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        let mut own = false;
        for decorator in &node.decorators {
            let Some(name) = decorator_name(decorator) else {
                continue;
            };
            if name != "Transactional" {
                continue;
            }
            own = true;
            out.push(jvm_detection(node, decorator, "java-spring-transactional"));
        }
        if !own {
            if let Some(decorator) = node
                .parent_class
                .as_deref()
                .and_then(|parent_class| class_tx.get(parent_class))
            {
                out.push(jvm_detection(
                    node,
                    decorator,
                    "java-spring-class-transactional",
                ));
            }
        }
    }
    out
}

fn class_transaction_annotations(nodes: &[&SynthNode]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        for decorator in &node.decorators {
            if decorator_name(decorator) == Some("Transactional") {
                out.insert(node.name.clone(), decorator.clone());
                out.insert(node.qn.clone(), decorator.clone());
            }
        }
    }
    out
}

fn jvm_detection(node: &SynthNode, decorator: &str, strategy: &str) -> TransactionDetection {
    TransactionDetection {
        name: format!("{} transaction", node.name),
        manager: "spring-transaction".into(),
        propagation: annotation_enum_value(decorator, "propagation"),
        isolation: annotation_enum_value(decorator, "isolation"),
        read_only: annotation_bool_value(decorator, "readOnly"),
        node_id: node.aka_id.clone(),
        strategy: strategy.into(),
    }
}

fn extract_python_transactions(text: &str, nodes: &[&SynthNode]) -> Vec<TransactionDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        for decorator in &node.decorators {
            let normalized = decorator.trim().trim_start_matches('@');
            if normalized == "transaction.atomic" || normalized.starts_with("transaction.atomic(") {
                out.push(TransactionDetection {
                    name: format!("{} transaction", node.name),
                    manager: "django-transaction".into(),
                    propagation: None,
                    isolation: None,
                    read_only: None,
                    node_id: node.aka_id.clone(),
                    strategy: "python-django-transaction-decorator".into(),
                });
            }
        }
    }
    for call in find_call_args(text, "transaction.atomic") {
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        out.push(TransactionDetection {
            name: format!("{} transaction", node.name),
            manager: "django-transaction".into(),
            propagation: None,
            isolation: None,
            read_only: None,
            node_id: node.aka_id.clone(),
            strategy: "python-django-transaction-context".into(),
        });
    }
    out
}

fn decorator_name(decorator: &str) -> Option<&str> {
    let text = decorator.trim().trim_start_matches('@');
    let name = text.split_once('(').map(|(name, _)| name).unwrap_or(text);
    Some(name.rsplit('.').next().unwrap_or(name).trim()).filter(|name| !name.is_empty())
}

fn annotation_enum_value(annotation: &str, key: &str) -> Option<String> {
    let (_, args) = annotation.split_once('(')?;
    for part in args.trim_end_matches(')').split(',') {
        let (part_key, value) = part.split_once('=')?;
        if !part_key.trim().ends_with(key) {
            continue;
        }
        let value = value.trim().trim_end_matches(')');
        let value = value.rsplit('.').next().unwrap_or(value).trim();
        if !value.is_empty() {
            return Some(value.to_ascii_uppercase());
        }
    }
    None
}

fn annotation_bool_value(annotation: &str, key: &str) -> Option<bool> {
    let (_, args) = annotation.split_once('(')?;
    for part in args.trim_end_matches(')').split(',') {
        let (part_key, value) = part.split_once('=')?;
        if !part_key.trim().ends_with(key) {
            continue;
        }
        return match value.trim().trim_end_matches(')') {
            "true" | "True" => Some(true),
            "false" | "False" => Some(false),
            _ => None,
        };
    }
    None
}
