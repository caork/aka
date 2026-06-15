use std::collections::HashMap;

use super::super::SynthNode;

pub(super) struct NodeLookup<'a> {
    by_simple: HashMap<(String, String), Vec<&'a SynthNode>>,
    by_file_name: HashMap<(String, String), Vec<&'a SynthNode>>,
}

impl<'a> NodeLookup<'a> {
    pub(super) fn new(nodes: impl Iterator<Item = &'a SynthNode>) -> Self {
        let mut by_simple: HashMap<(String, String), Vec<&'a SynthNode>> = HashMap::new();
        let mut by_file_name: HashMap<(String, String), Vec<&'a SynthNode>> = HashMap::new();
        for node in nodes {
            let language = language_key(node);
            let simple = simple_node_name(node);
            by_simple
                .entry((language.clone(), simple.clone()))
                .or_default()
                .push(node);
            by_file_name
                .entry((node.file_path.clone(), simple))
                .or_default()
                .push(node);
        }
        for values in by_simple.values_mut() {
            values.sort_by(|a, b| a.aka_id.cmp(&b.aka_id));
        }
        for values in by_file_name.values_mut() {
            values.sort_by(|a, b| a.aka_id.cmp(&b.aka_id));
        }
        Self {
            by_simple,
            by_file_name,
        }
    }

    pub(super) fn resolve_type(&self, language: &str, type_name: &str) -> Option<&'a SynthNode> {
        let simple = simple_type_name(type_name)?;
        self.by_simple
            .get(&(language.to_string(), simple))
            .and_then(|nodes| (nodes.len() == 1).then_some(nodes[0]))
    }

    pub(super) fn resolve_python_callable(
        &self,
        file_path: &str,
        expr: &str,
    ) -> Option<&'a SynthNode> {
        let name = expr.rsplit('.').next().unwrap_or(expr);
        self.by_file_name
            .get(&(file_path.to_string(), name.to_string()))
            .and_then(|nodes| nodes.first().copied())
            .or_else(|| {
                self.by_simple
                    .get(&("python".to_string(), name.to_string()))
                    .and_then(|nodes| (nodes.len() == 1).then_some(nodes[0]))
            })
    }
}

pub(super) fn simple_type_name(type_name: &str) -> Option<String> {
    let trimmed = type_name
        .trim()
        .trim_matches('?')
        .trim_end_matches("...")
        .trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_generics = trimmed
        .split_once('<')
        .map(|(base, _)| base)
        .unwrap_or(trimmed);
    let simple = without_generics
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '$' && ch != '.')
        .rsplit(['.', '$'])
        .next()
        .unwrap_or(without_generics)
        .trim();
    is_meaningful_java_type(simple).then(|| simple.to_string())
}

pub(super) fn is_meaningful_java_type(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
        && !matches!(
            name,
            "String"
                | "Integer"
                | "Long"
                | "Boolean"
                | "Double"
                | "Float"
                | "Short"
                | "Byte"
                | "Character"
                | "Object"
                | "List"
                | "Set"
                | "Map"
                | "Collection"
                | "Optional"
        )
}

fn language_key(node: &SynthNode) -> String {
    match node.language.to_ascii_lowercase().as_str() {
        "python" => "python".into(),
        "java" | "kotlin" | "scala" | "groovy" => "java".into(),
        other => {
            if node.file_path.ends_with(".py") {
                "python".into()
            } else if is_java_like_extension(&node.file_path) {
                "java".into()
            } else {
                other.to_string()
            }
        }
    }
}

fn is_java_like_extension(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    lower.ends_with(".java")
        || lower.ends_with(".kt")
        || lower.ends_with(".scala")
        || lower.ends_with(".groovy")
}

fn simple_node_name(node: &SynthNode) -> String {
    node.qn
        .rsplit(['.', '$'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(&node.name)
        .to_string()
}
