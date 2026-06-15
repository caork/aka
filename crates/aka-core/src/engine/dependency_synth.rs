use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde_json::json;

use super::{
    find_call_args, find_matching_paren, is_ident_continue, node_at_offset,
    project_code_nodes_by_file, read_repo_text, skip_ws, split_top_level_commas, stable_hash,
    EdgeRec, ProjectSourceSet, SynthNode,
};

pub(super) fn synthesize_dependency_edges_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    existing_call_pairs: &HashSet<(String, String)>,
) -> Vec<EdgeRec> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let lookup = NodeLookup::new(by_file.values().flatten().copied());
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        out.extend(detect_java_dependency_edges(
            &text,
            &file_path,
            &file_nodes,
            &lookup,
        ));
        out.extend(detect_python_dependency_edges(
            &text,
            &file_path,
            &file_nodes,
            &lookup,
            existing_call_pairs,
        ));
    }
    out.retain(|edge| {
        seen.insert((
            edge.source_id.clone(),
            edge.target_id.clone(),
            edge.edge_type.clone(),
            edge.reason.clone(),
        ))
    });
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

struct NodeLookup<'a> {
    by_simple: HashMap<(String, String), Vec<&'a SynthNode>>,
    by_file_name: HashMap<(String, String), Vec<&'a SynthNode>>,
}

impl<'a> NodeLookup<'a> {
    fn new(nodes: impl Iterator<Item = &'a SynthNode>) -> Self {
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

    fn resolve_type(&self, language: &str, type_name: &str) -> Option<&'a SynthNode> {
        let simple = simple_type_name(type_name)?;
        self.by_simple
            .get(&(language.to_string(), simple))
            .and_then(|nodes| (nodes.len() == 1).then_some(nodes[0]))
    }

    fn resolve_python_callable(&self, file_path: &str, expr: &str) -> Option<&'a SynthNode> {
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

fn detect_java_dependency_edges(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
    lookup: &NodeLookup<'_>,
) -> Vec<EdgeRec> {
    if !is_java_like_file(file_path, nodes) {
        return Vec::new();
    }
    let mut out = Vec::new();
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

fn detect_python_dependency_edges(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
    lookup: &NodeLookup<'_>,
    existing_call_pairs: &HashSet<(String, String)>,
) -> Vec<EdgeRec> {
    if !is_python_file(file_path, nodes) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for call in find_python_dependency_calls(text) {
        let Some(source) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        let Some(target) = lookup.resolve_python_callable(file_path, &call.callable) else {
            continue;
        };
        if target.aka_id == source.aka_id {
            continue;
        }
        out.push(dependency_edge(
            &source.aka_id,
            &target.aka_id,
            "python-fastapi-dependency",
            &call.strategy,
            &call.callable,
            0.74,
        ));
        if !existing_call_pairs.contains(&(source.aka_id.clone(), target.aka_id.clone())) {
            out.push(EdgeRec {
                id: format!(
                    "python-depends-call:{:016x}",
                    stable_hash(&format!(
                        "{}|{}|{}",
                        source.aka_id, target.aka_id, call.callable
                    ))
                ),
                source_id: source.aka_id.clone(),
                target_id: target.aka_id.clone(),
                edge_type: "CALLS".into(),
                confidence: 0.74,
                reason: "aka FastAPI Depends dependency call".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "fastapi-depends",
                    "dependency": call.callable,
                    "strategy": call.strategy,
                })),
            });
        }
    }
    out
}

#[derive(Debug, Clone)]
struct JavaDependency {
    type_name: String,
    strategy: String,
}

#[derive(Debug, Clone)]
struct PythonDependencyCall {
    start: usize,
    callable: String,
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

fn find_python_dependency_calls(text: &str) -> Vec<PythonDependencyCall> {
    let mut out = Vec::new();
    for callee in ["Depends", "Security"] {
        for call in find_call_args(text, callee) {
            if let Some(callable) = first_depends_callable(call.args) {
                out.push(PythonDependencyCall {
                    start: call.start,
                    callable,
                    strategy: format!("python-fastapi-{}", callee.to_ascii_lowercase()),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then_with(|| a.callable.cmp(&b.callable))
    });
    out.dedup_by(|a, b| a.start == b.start && a.callable == b.callable);
    out
}

fn first_depends_callable(args: &str) -> Option<String> {
    for arg in split_top_level_commas(args) {
        let arg = arg.trim();
        if arg.is_empty() {
            continue;
        }
        let value = if let Some((key, value)) = arg.split_once('=') {
            if !matches!(key.trim(), "dependency" | "call" | "callable") {
                continue;
            }
            value.trim()
        } else {
            arg
        };
        let value = value.trim_start_matches("lambda ").trim();
        if value.starts_with('"') || value.starts_with('\'') || value.starts_with("None") {
            continue;
        }
        let name = value
            .split_once('(')
            .map(|(name, _)| name)
            .unwrap_or(value)
            .trim();
        if is_python_callable_expr(name) {
            return Some(name.to_string());
        }
    }
    None
}

fn dependency_edge(
    source_id: &str,
    target_id: &str,
    kind: &str,
    strategy: &str,
    dependency: &str,
    confidence: f64,
) -> EdgeRec {
    EdgeRec {
        id: format!(
            "dependency:heuristic:{:016x}",
            stable_hash(&format!(
                "{source_id}|{target_id}|{kind}|{strategy}|{dependency}"
            ))
        ),
        source_id: source_id.to_string(),
        target_id: target_id.to_string(),
        edge_type: "DEPENDS_ON".into(),
        confidence,
        reason: "aka dependency synthesis".into(),
        step: None,
        evidence: Some(json!({
            "source": "aka-cbm-synth",
            "kind": kind,
            "strategy": strategy,
            "dependency": dependency,
        })),
    }
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

fn is_python_file(file_path: &str, nodes: &[&SynthNode]) -> bool {
    file_path.to_ascii_lowercase().ends_with(".py")
        || nodes
            .iter()
            .any(|node| node.language.eq_ignore_ascii_case("python"))
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

fn simple_type_name(type_name: &str) -> Option<String> {
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

fn is_meaningful_java_type(name: &str) -> bool {
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

fn is_python_callable_expr(expr: &str) -> bool {
    let mut parts = expr.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    is_python_identifier(first) && parts.all(is_python_identifier)
}

fn is_python_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
