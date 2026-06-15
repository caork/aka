use std::collections::HashSet;

use serde_json::json;

use super::super::{
    find_call_args, find_matching_paren, is_ident_continue, node_at_offset, skip_ws,
    split_top_level_commas, stable_hash, EdgeRec, SynthNode,
};
use super::dependency_edge;
use super::lookup::{is_meaningful_java_type, simple_type_name, NodeLookup};

pub(super) fn detect_java_dependency_edges(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
    lookup: &NodeLookup<'_>,
    existing_call_pairs: &HashSet<(String, String)>,
) -> Vec<EdgeRec> {
    if !is_java_like_file(file_path, nodes) {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.extend(detect_java_direct_call_edges(
        text,
        nodes,
        existing_call_pairs,
    ));
    for class_node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface" | "Type"))
    {
        let Some(body) = class_source_slice(text, class_node) else {
            continue;
        };
        for dep in detect_java_field_injections(body)
            .into_iter()
            .chain(detect_java_constructor_injections(body, &class_node.name))
        {
            if let Some(target) = lookup.resolve_type("java", &dep.type_name) {
                if target.aka_id != class_node.aka_id {
                    out.push(dependency_edge(
                        &class_node.aka_id,
                        &target.aka_id,
                        "java-spring-dependency-injection",
                        &dep.strategy,
                        &dep.type_name,
                        0.7,
                    ));
                }
            }
        }
    }
    for method in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        for dep in detect_java_bean_method_dependencies(text, method) {
            if let Some(target) = lookup.resolve_type("java", &dep.type_name) {
                if target.aka_id != method.aka_id {
                    out.push(dependency_edge(
                        &method.aka_id,
                        &target.aka_id,
                        "java-spring-bean-dependency",
                        &dep.strategy,
                        &dep.type_name,
                        0.68,
                    ));
                }
            }
        }
    }
    out
}

fn detect_java_direct_call_edges(
    text: &str,
    nodes: &[&SynthNode],
    existing_call_pairs: &HashSet<(String, String)>,
) -> Vec<EdgeRec> {
    let methods = java_file_methods(nodes);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut context = JavaDirectCallContext {
        text,
        nodes,
        existing_call_pairs,
        seen: &mut seen,
        out: &mut out,
    };
    for target in methods {
        for call in find_call_args(text, target.name) {
            let Some(strategy) = java_direct_call_strategy(text, call.start, target.name) else {
                continue;
            };
            context.push(target, call.start, strategy);
        }
        let receiver_callee = format!(".{}", target.name);
        for call in find_call_args(text, &receiver_callee) {
            let Some(strategy) = java_receiver_call_strategy(text, call.start + 1, target.name)
            else {
                continue;
            };
            context.push(target, call.start, strategy);
        }
    }
    out
}

struct JavaDirectCallContext<'a, 'b> {
    text: &'a str,
    nodes: &'a [&'a SynthNode],
    existing_call_pairs: &'a HashSet<(String, String)>,
    seen: &'b mut HashSet<(String, String)>,
    out: &'b mut Vec<EdgeRec>,
}

impl JavaDirectCallContext<'_, '_> {
    fn push(&mut self, target: JavaMethodNode<'_>, call_start: usize, strategy: &'static str) {
        let Some(source) = node_at_offset(self.text, self.nodes, call_start) else {
            return;
        };
        if source.aka_id == target.node.aka_id
            || self
                .existing_call_pairs
                .contains(&(source.aka_id.clone(), target.node.aka_id.clone()))
        {
            return;
        }
        let key = (source.aka_id.clone(), target.node.aka_id.clone());
        if !self.seen.insert(key) {
            return;
        }
        self.out.push(EdgeRec {
            id: format!(
                "java-direct-call:{:016x}",
                stable_hash(&format!(
                    "{}|{}|{}",
                    source.aka_id, target.node.aka_id, target.name
                ))
            ),
            source_id: source.aka_id.clone(),
            target_id: target.node.aka_id.clone(),
            edge_type: "CALLS".into(),
            confidence: 0.68,
            reason: "aka Java direct call synthesis".into(),
            step: None,
            evidence: Some(json!({
                "source": "aka-cbm-synth",
                "kind": "java-direct-call",
                "strategy": strategy,
                "callable": target.name,
            })),
        });
    }
}

#[derive(Debug, Clone, Copy)]
struct JavaMethodNode<'a> {
    name: &'a str,
    node: &'a SynthNode,
}

fn java_file_methods<'a>(nodes: &[&'a SynthNode]) -> Vec<JavaMethodNode<'a>> {
    let mut out: Vec<_> = nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
        .filter(|node| is_java_method_name(&node.name))
        .map(|node| JavaMethodNode {
            name: node.name.as_str(),
            node,
        })
        .collect();
    out.sort_by(|a, b| {
        a.name
            .cmp(b.name)
            .then_with(|| a.node.aka_id.cmp(&b.node.aka_id))
    });
    out.dedup_by(|a, b| a.name == b.name && a.node.aka_id == b.node.aka_id);
    out
}

fn java_direct_call_strategy(text: &str, start: usize, name: &str) -> Option<&'static str> {
    let line_start = text[..start].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let prefix = text[line_start..start].trim_start();
    if prefix.starts_with('@') {
        return None;
    }
    if declaration_prefix_before_name(prefix) {
        return None;
    }
    let before = text[..start].chars().rev().find(|ch| !ch.is_whitespace());
    if matches!(before, Some('@')) {
        return None;
    }
    if matches!(before, Some('.')) {
        return None;
    }
    let after = text[start + name.len()..]
        .chars()
        .find(|ch| !ch.is_whitespace());
    (after == Some('(')).then_some("java-same-file-direct-call")
}

fn java_receiver_call_strategy(text: &str, name_start: usize, name: &str) -> Option<&'static str> {
    java_receiver_before_dot(text, name_start)
        .filter(|receiver| *receiver == "this")
        .and_then(|_| {
            let after = text[name_start + name.len()..]
                .chars()
                .find(|ch| !ch.is_whitespace());
            (after == Some('(')).then_some("java-same-file-receiver-call")
        })
}

fn java_receiver_before_dot(text: &str, name_start: usize) -> Option<&str> {
    let dot = text[..name_start].rfind('.')?;
    let before_dot = text[..dot].trim_end();
    let receiver_end = before_dot.len();
    let receiver_start = before_dot
        .rfind(|ch: char| !(ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()))
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let receiver = &before_dot[receiver_start..receiver_end];
    is_java_method_name(receiver).then_some(receiver)
}

fn declaration_prefix_before_name(prefix: &str) -> bool {
    let tokens: Vec<_> = prefix
        .split_whitespace()
        .filter(|token| !token.starts_with('@'))
        .collect();
    tokens.iter().any(|token| {
        matches!(
            token.trim_matches(','),
            "public"
                | "protected"
                | "private"
                | "static"
                | "final"
                | "abstract"
                | "synchronized"
                | "native"
                | "strictfp"
                | "default"
        )
    }) || tokens.len() >= 2
}

fn is_java_method_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

#[derive(Debug, Clone)]
struct JavaDependency {
    type_name: String,
    strategy: String,
}

fn detect_java_field_injections(text: &str) -> Vec<JavaDependency> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if !(trimmed.contains("@Autowired")
            || trimmed.contains("@Inject")
            || trimmed.contains("@Resource"))
        {
            continue;
        }
        let field_line = if looks_like_java_field(trimmed) {
            trimmed
        } else {
            lines
                .iter()
                .skip(idx + 1)
                .map(|line| line.trim())
                .find(|line| !line.is_empty() && !line.starts_with('@'))
                .unwrap_or("")
        };
        if let Some(type_name) = java_field_type(field_line) {
            out.push(JavaDependency {
                type_name,
                strategy: "java-field-injection".into(),
            });
        }
    }
    out
}

fn detect_java_constructor_injections(text: &str, class_name: &str) -> Vec<JavaDependency> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(class_name) {
        let start = offset + pos;
        if !word_boundary_ok(text, start, class_name.len()) {
            offset = start + class_name.len();
            continue;
        }
        let open = skip_ws(text, start + class_name.len());
        if text.as_bytes().get(open) != Some(&b'(') {
            offset = start + class_name.len();
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        if !constructor_prefix_ok(&text[start.saturating_sub(120)..start]) {
            offset = close + 1;
            continue;
        }
        for param in split_top_level_commas(&text[open + 1..close]) {
            if let Some(type_name) = java_parameter_type(param) {
                out.push(JavaDependency {
                    type_name,
                    strategy: "java-constructor-injection".into(),
                });
            }
        }
        offset = close + 1;
    }
    out
}

fn detect_java_bean_method_dependencies(text: &str, node: &SynthNode) -> Vec<JavaDependency> {
    if !node.decorators.iter().any(|decorator| {
        let normalized = decorator.trim().trim_start_matches('@');
        normalized == "Bean" || normalized.starts_with("Bean(") || normalized.ends_with(".Bean")
    }) {
        return Vec::new();
    }
    let Some((decl_start, decl_end)) = node_declaration_range(text, node) else {
        return Vec::new();
    };
    let declaration = &text[decl_start..decl_end];
    let Some(name_pos) = declaration.find(&node.name) else {
        return Vec::new();
    };
    let open = skip_ws(declaration, name_pos + node.name.len());
    if declaration.as_bytes().get(open) != Some(&b'(') {
        return Vec::new();
    }
    let Some(close) = find_matching_paren(declaration, open) else {
        return Vec::new();
    };
    split_top_level_commas(&declaration[open + 1..close])
        .into_iter()
        .filter_map(java_parameter_type)
        .map(|type_name| JavaDependency {
            type_name,
            strategy: "java-bean-method-parameter".into(),
        })
        .collect()
}

fn is_java_like_file(file_path: &str, nodes: &[&SynthNode]) -> bool {
    let lower = file_path.to_ascii_lowercase();
    lower.ends_with(".java")
        || lower.ends_with(".kt")
        || lower.ends_with(".scala")
        || lower.ends_with(".groovy")
        || nodes.iter().any(|node| {
            matches!(
                node.language.to_ascii_lowercase().as_str(),
                "java" | "kotlin" | "scala" | "groovy"
            )
        })
}

fn class_source_slice<'a>(text: &'a str, node: &SynthNode) -> Option<&'a str> {
    let start_line = node.start_line.max(1);
    let end_line = node.end_line.max(start_line);
    let (start, end) = line_range(text, start_line, end_line)?;
    Some(&text[start..end])
}

fn node_declaration_range(text: &str, node: &SynthNode) -> Option<(usize, usize)> {
    let start_line = node.start_line.max(1);
    let (start, mut end) = line_range(text, start_line, node.end_line.max(start_line))?;
    if let Some(open_rel) = text[start..end].find('{') {
        end = start + open_rel;
    }
    Some((start.saturating_sub(500), end))
}

fn line_range(text: &str, start_line: i64, end_line: i64) -> Option<(usize, usize)> {
    let mut line = 1i64;
    let mut start = None;
    let mut end = text.len();
    for (idx, ch) in text.char_indices() {
        if line == start_line && start.is_none() {
            start = Some(idx);
        }
        if line > end_line {
            end = idx;
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    if start.is_none() && line == start_line {
        start = Some(text.len());
    }
    start.map(|start| (start.min(text.len()), end.min(text.len())))
}

fn looks_like_java_field(line: &str) -> bool {
    line.ends_with(';') && !line.contains('(') && java_field_type(line).is_some()
}

fn java_field_type(line: &str) -> Option<String> {
    let clean = line
        .split_once('=')
        .map(|(lhs, _)| lhs)
        .unwrap_or(line)
        .trim_end_matches(';')
        .trim();
    let tokens = java_type_tokens(clean);
    tokens
        .get(tokens.len().saturating_sub(2))
        .cloned()
        .filter(|token| is_meaningful_java_type(token))
}

fn java_parameter_type(param: &str) -> Option<String> {
    let clean = param
        .split_once('=')
        .map(|(lhs, _)| lhs)
        .unwrap_or(param)
        .trim();
    if clean.is_empty() {
        return None;
    }
    let tokens = java_type_tokens(clean);
    tokens
        .get(tokens.len().saturating_sub(2))
        .cloned()
        .filter(|token| is_meaningful_java_type(token))
}

fn java_type_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter(|token| !token.starts_with('@'))
        .filter(|token| {
            !matches!(
                token.trim_matches(','),
                "private"
                    | "protected"
                    | "public"
                    | "final"
                    | "static"
                    | "volatile"
                    | "transient"
                    | "var"
            )
        })
        .map(|token| {
            token
                .trim_matches(',')
                .trim_matches(';')
                .trim_matches(|ch: char| ch == '[' || ch == ']')
                .to_string()
        })
        .filter_map(|token| simple_type_name(&token))
        .collect()
}

fn constructor_prefix_ok(prefix: &str) -> bool {
    let trimmed = prefix
        .rsplit_once('\n')
        .map(|(_, line)| line)
        .unwrap_or(prefix)
        .trim_end();
    trimmed.is_empty()
        || trimmed.ends_with("public")
        || trimmed.ends_with("private")
        || trimmed.ends_with("protected")
        || trimmed.ends_with("@Autowired")
        || trimmed.ends_with("@Inject")
        || trimmed.contains("@Autowired")
        || trimmed.contains("@Inject")
}

fn word_boundary_ok(text: &str, start: usize, len: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[start + len..].chars().next();
    before.is_none_or(|ch| !is_ident_continue(ch)) && after.is_none_or(|ch| !is_ident_continue(ch))
}
