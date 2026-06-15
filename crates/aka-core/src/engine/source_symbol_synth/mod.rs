use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Map, Value};

mod java;

use java::java_source_symbols;

use super::{read_repo_text, stable_hash, NodeRec, ProjectSourceSet, SynthNode};

#[derive(Debug, Clone)]
pub(super) struct SynthSourceSymbol {
    node: SynthNode,
}

impl SynthSourceSymbol {
    pub(super) fn node(&self) -> &SynthNode {
        &self.node
    }

    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.node.name.clone()));
        properties.insert("qualifiedName".into(), Value::String(self.node.qn.clone()));
        properties.insert(
            "filePath".into(),
            Value::String(self.node.file_path.clone()),
        );
        properties.insert(
            "startLine".into(),
            Value::from(to_artifact_line(self.node.start_line)),
        );
        properties.insert(
            "endLine".into(),
            Value::from(to_artifact_line(self.node.end_line)),
        );
        properties.insert("language".into(), Value::String(self.node.language.clone()));
        if !self.node.decorators.is_empty() {
            properties.insert(
                "decorators".into(),
                Value::Array(
                    self.node
                        .decorators
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        if let Some(route_path) = &self.node.route_path {
            properties.insert("route_path".into(), Value::String(route_path.clone()));
            properties.insert("routePath".into(), Value::String(route_path.clone()));
        }
        if let Some(route_method) = &self.node.route_method {
            properties.insert("route_method".into(), Value::String(route_method.clone()));
            properties.insert("routeMethod".into(), Value::String(route_method.clone()));
        }
        properties.insert("source".into(), Value::String("aka-source-scan".into()));
        properties.insert(
            "strategy".into(),
            Value::String("java-source-symbol-fallback".into()),
        );
        if let Some(parent_class) = &self.node.parent_class {
            properties.insert("parent_class".into(), Value::String(parent_class.clone()));
            properties.insert("parentClass".into(), Value::String(parent_class.clone()));
        }
        NodeRec {
            id: self.node.aka_id.clone(),
            label: self.node.label.clone(),
            properties,
        }
    }
}

pub(super) fn synthesize_source_symbols_from_sources(
    repo: &Path,
    existing: &BTreeMap<String, SynthNode>,
) -> Vec<SynthSourceSymbol> {
    let project_sources = ProjectSourceSet::discover(repo);
    let mut existing_keys = existing_symbol_keys(existing);
    let mut out = Vec::new();
    for file_path in project_sources.project_files_with_extensions(repo, &["java"]) {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        for node in java_source_symbols(file_path, &text) {
            let key = symbol_key(&node);
            if existing_keys.insert(key) {
                out.push(SynthSourceSymbol { node });
            }
        }
    }
    out.sort_by(|a, b| {
        a.node
            .file_path
            .cmp(&b.node.file_path)
            .then_with(|| a.node.start_line.cmp(&b.node.start_line))
            .then_with(|| a.node.qn.cmp(&b.node.qn))
    });
    out
}

fn existing_symbol_keys(nodes: &BTreeMap<String, SynthNode>) -> BTreeSet<String> {
    nodes.values().map(symbol_key).collect()
}

fn symbol_key(node: &SynthNode) -> String {
    format!("{}|{}|{}", node.label, node.file_path, node.qn)
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

fn normalize_route_literal(route: &str) -> String {
    let trimmed = route.trim();
    if trimmed.is_empty() {
        "/".into()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn to_artifact_line(line_1based: i64) -> u32 {
    if line_1based <= 0 {
        0
    } else {
        (line_1based - 1) as u32
    }
}
