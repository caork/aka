use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, is_project_test_source_path, nodes_by_file, process_ids_for_entry,
    read_repo_text, read_string_literal, split_top_level_commas, stable_hash, EdgeRec, NodeRec,
    ProjectSourceSet, SynthNode, SynthProcess,
};

#[derive(Debug, Clone)]
pub(super) struct SynthCommand {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) command_type: String,
    pub(super) file_path: String,
    pub(super) handler_id: String,
    pub(super) handler_name: String,
    pub(super) strategy: String,
    pub(super) process_ids: Vec<String>,
}

impl SynthCommand {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert(
            "commandType".into(),
            Value::String(self.command_type.clone()),
        );
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("handlerId".into(), Value::String(self.handler_id.clone()));
        properties.insert(
            "handlerName".into(),
            Value::String(self.handler_name.clone()),
        );
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("commandSource".into(), Value::String("source-scan".into()));
        properties.insert("strategy".into(), Value::String(self.strategy.clone()));
        NodeRec {
            id: self.id.clone(),
            label: "Command".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = vec![EdgeRec {
            id: format!("{}:handles:{:016x}", self.id, stable_hash(&self.handler_id)),
            source_id: self.handler_id.clone(),
            target_id: self.id.clone(),
            edge_type: "HANDLES_COMMAND".into(),
            confidence: 0.66,
            reason: "aka command synthesis".into(),
            step: None,
            evidence: Some(json!({
                "source": "aka-cbm-synth",
                "kind": "command-handler",
                "command": self.name,
                "commandType": self.command_type,
                "strategy": self.strategy,
            })),
        }];
        for process_id in &self.process_ids {
            out.push(EdgeRec {
                id: format!("{}:entry-process:{:016x}", self.id, stable_hash(process_id)),
                source_id: self.id.clone(),
                target_id: process_id.clone(),
                edge_type: "ENTRY_POINT_OF".into(),
                confidence: 0.52,
                reason: "aka command process linkage".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "command-entry-process",
                    "command": self.name,
                    "commandType": self.command_type,
                })),
            });
        }
        out
    }
}

pub(super) fn synthesize_commands_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    processes: &[SynthProcess],
) -> Vec<SynthCommand> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = command_nodes_by_file(repo, nodes, &project_sources);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let text = read_repo_text(repo, &file_path);
        for node in file_nodes
            .iter()
            .copied()
            .filter(|node| matches!(node.label.as_str(), "Class" | "Function" | "Method"))
        {
            for detection in detect_node_commands(text.as_deref(), node) {
                let key = format!(
                    "{}|{}|{}|{}",
                    node.aka_id, detection.command_type, detection.name, detection.strategy
                );
                if !seen.insert(key.clone()) {
                    continue;
                }
                out.push(SynthCommand {
                    id: format!("command:heuristic:{:016x}", stable_hash(&key)),
                    name: detection.name,
                    command_type: detection.command_type,
                    file_path: file_path.clone(),
                    handler_id: node.aka_id.clone(),
                    handler_name: node.display_name().to_string(),
                    strategy: detection.strategy,
                    process_ids: process_ids_for_entry(processes, &file_path, Some(&node.aka_id)),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.command_type
            .cmp(&b.command_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.handler_id.cmp(&b.handler_id))
    });
    out
}

#[derive(Debug, Clone)]
struct CommandDetection {
    name: String,
    command_type: String,
    strategy: String,
}

fn command_nodes_by_file<'a>(
    repo: &Path,
    nodes: &'a BTreeMap<String, SynthNode>,
    project_sources: &ProjectSourceSet,
) -> BTreeMap<String, Vec<&'a SynthNode>> {
    nodes_by_file(nodes)
        .into_iter()
        .filter(|(file_path, file_nodes)| {
            is_project_command_source(repo, file_path, file_nodes, project_sources)
        })
        .collect()
}

fn is_project_command_source(
    repo: &Path,
    file_path: &str,
    file_nodes: &[&SynthNode],
    project_sources: &ProjectSourceSet,
) -> bool {
    if is_project_test_source_path(file_path) {
        return false;
    }
    let source_like = is_command_source_path(file_path)
        || file_nodes.iter().any(|node| {
            matches!(
                node.language.to_ascii_lowercase().as_str(),
                "java" | "kotlin" | "scala" | "groovy" | "python"
            )
        });
    if !source_like {
        return false;
    }
    project_sources.contains_project_file(repo, file_path)
}

fn is_command_source_path(file_path: &str) -> bool {
    matches!(
        Path::new(&file_path.to_ascii_lowercase())
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("java" | "kt" | "kts" | "scala" | "groovy" | "py")
    )
}

fn detect_node_commands(text: Option<&str>, node: &SynthNode) -> Vec<CommandDetection> {
    let mut out = Vec::new();
    let lower_path = node.file_path.to_ascii_lowercase();
    if lower_path.ends_with(".java")
        || matches!(
            node.language.to_ascii_lowercase().as_str(),
            "java" | "kotlin" | "scala" | "groovy"
        )
    {
        out.extend(detect_jvm_commands(text, node));
    }
    if lower_path.ends_with(".py") || node.language.eq_ignore_ascii_case("python") {
        out.extend(detect_python_commands(text, node));
    }
    out.sort_by(|a, b| {
        a.command_type
            .cmp(&b.command_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out
}

fn detect_jvm_commands(text: Option<&str>, node: &SynthNode) -> Vec<CommandDetection> {
    let mut out = Vec::new();
    if text.is_some_and(|text| declares_spring_runner_entrypoint(text, node)) {
        out.push(CommandDetection {
            name: format!("{} runner", node.display_name()),
            command_type: "spring-runner".into(),
            strategy: "java-spring-runner-source-declaration".into(),
        });
    }
    for decorator in &node.decorators {
        if decorator_name(decorator) != Some("Command") {
            continue;
        }
        out.push(CommandDetection {
            name: annotation_string_value(decorator, "name")
                .or_else(|| annotation_string_value(decorator, "value"))
                .or_else(|| annotation_array_first_value(decorator, "aliases"))
                .unwrap_or_else(|| node.display_name().to_string()),
            command_type: "picocli-command".into(),
            strategy: "java-picocli-command".into(),
        });
    }
    out
}

fn declares_spring_runner_entrypoint(text: &str, node: &SynthNode) -> bool {
    if node.label == "Class" {
        return class_declaration_mentions_runner(text, node);
    }
    if node.label == "Method" {
        return bean_method_returns_runner(text, node)
            || (node.name == "run" && parent_class_declaration_mentions_runner(text, node));
    }
    false
}

fn class_declaration_mentions_runner(text: &str, node: &SynthNode) -> bool {
    let Some(class_pos) = find_class_declaration(text, node) else {
        return false;
    };
    let Some(open_brace_rel) = text[class_pos..].find('{') else {
        return false;
    };
    declaration_mentions_runner(&text[class_pos..class_pos + open_brace_rel])
}

fn parent_class_declaration_mentions_runner(text: &str, node: &SynthNode) -> bool {
    let Some(parent) = node.parent_class.as_deref() else {
        return false;
    };
    let class_name = parent.rsplit(['.', '$']).next().unwrap_or(parent);
    if class_name.is_empty() {
        return false;
    }
    let needle = format!("class {class_name}");
    let Some(class_pos) = text.find(&needle) else {
        return false;
    };
    let Some(open_brace_rel) = text[class_pos..].find('{') else {
        return false;
    };
    declaration_mentions_runner(&text[class_pos..class_pos + open_brace_rel])
}

fn bean_method_returns_runner(text: &str, node: &SynthNode) -> bool {
    let Some((start, _end)) = node_body_byte_range(text, node) else {
        return false;
    };
    let window_start = start.saturating_sub(500);
    let Some(open_brace_rel) = text[start.min(text.len())..].find('{') else {
        return false;
    };
    let window_end = (start + open_brace_rel).min(text.len());
    let declaration = &text[window_start..window_end];
    declaration.contains("@Bean")
        && (declaration.contains("CommandLineRunner") || declaration.contains("ApplicationRunner"))
}

fn declaration_mentions_runner(declaration: &str) -> bool {
    declaration.contains("implements")
        && (declaration.contains("CommandLineRunner") || declaration.contains("ApplicationRunner"))
}

fn find_class_declaration(text: &str, node: &SynthNode) -> Option<usize> {
    [
        node.name.as_str(),
        node.qn
            .rsplit(['.', '$'])
            .next()
            .unwrap_or(node.qn.as_str()),
    ]
    .into_iter()
    .find_map(|name| {
        (!name.is_empty())
            .then(|| format!("class {name}"))
            .and_then(|needle| text.find(&needle))
    })
}

fn detect_python_commands(text: Option<&str>, node: &SynthNode) -> Vec<CommandDetection> {
    let mut out = Vec::new();
    for decorator in &node.decorators {
        let normalized = decorator.trim().trim_start_matches('@');
        if is_click_command_decorator(normalized) {
            out.push(CommandDetection {
                name: command_name_from_python_decorator(normalized)
                    .unwrap_or_else(|| node.display_name().replace('_', "-")),
                command_type: "click-command".into(),
                strategy: "python-click-command".into(),
            });
            continue;
        }
        if is_typer_command_decorator(normalized) {
            out.push(CommandDetection {
                name: command_name_from_python_decorator(normalized)
                    .unwrap_or_else(|| node.display_name().replace('_', "-")),
                command_type: "typer-command".into(),
                strategy: "python-typer-command".into(),
            });
        }
    }
    if matches!(node.label.as_str(), "Function" | "Method")
        && node.name == "handle"
        && is_django_management_command(node)
    {
        out.push(CommandDetection {
            name: django_command_name(node),
            command_type: "django-management-command".into(),
            strategy: "python-django-management-command".into(),
        });
    }
    if node.label == "Function" {
        if let Some(text) = text {
            out.extend(detect_python_argparse_entrypoints(text, node));
        }
    }
    out
}

fn is_click_command_decorator(text: &str) -> bool {
    text == "click.command"
        || text.starts_with("click.command(")
        || text.ends_with(".command")
        || text.contains(".command(") && text.contains("click")
}

fn is_typer_command_decorator(text: &str) -> bool {
    text.ends_with(".command")
        || text.contains(".command(")
        || text.ends_with(".callback")
        || text.contains(".callback(")
}

fn command_name_from_python_decorator(text: &str) -> Option<String> {
    let args = python_call_args(text)?;
    keyword_string_literal(args, "name")
        .or_else(|| first_string_literal(args))
        .map(|name| name.replace('_', "-"))
}

fn is_django_management_command(node: &SynthNode) -> bool {
    let path = node.file_path.replace('\\', "/").to_ascii_lowercase();
    path.contains("/management/commands/")
        || node
            .parent_class
            .as_ref()
            .is_some_and(|parent| parent.ends_with("Command") || parent.contains("BaseCommand"))
        || node.qn.contains(".Command.handle")
}

fn django_command_name(node: &SynthNode) -> String {
    let path = node.file_path.replace('\\', "/");
    Path::new(&path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty() && *stem != "__init__")
        .unwrap_or_else(|| node.display_name())
        .to_string()
}

fn detect_python_argparse_entrypoints(text: &str, node: &SynthNode) -> Vec<CommandDetection> {
    let Some((body_start, body_end)) = node_body_byte_range(text, node) else {
        return Vec::new();
    };
    let body = &text[body_start..body_end];
    if !(body.contains("ArgumentParser") || body.contains(".add_parser(")) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut names = Vec::new();
    for callee in ["ArgumentParser", ".ArgumentParser"] {
        for call in find_call_args(body, callee) {
            if let Some(name) = keyword_string_literal(call.args, "prog") {
                names.push(name);
            }
        }
    }
    for call in find_call_args(body, ".add_parser") {
        if let Some(name) = first_string_literal(call.args) {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    if names.is_empty() {
        names.push(node.display_name().replace('_', "-"));
    }
    out.extend(names.into_iter().map(|name| CommandDetection {
        name,
        command_type: "argparse-command".into(),
        strategy: "python-argparse-command".into(),
    }));
    out
}

fn node_body_byte_range(text: &str, node: &SynthNode) -> Option<(usize, usize)> {
    let start_line = node.start_line.max(1);
    let end_line = node.end_line.max(start_line);
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

fn decorator_name(decorator: &str) -> Option<&str> {
    let text = decorator.trim().trim_start_matches('@');
    let name = text.split_once('(').map(|(name, _)| name).unwrap_or(text);
    Some(name.rsplit('.').next().unwrap_or(name).trim()).filter(|name| !name.is_empty())
}

fn annotation_string_value(annotation: &str, key: &str) -> Option<String> {
    let args = annotation_args(annotation)?;
    keyword_string_literal(args, key)
}

fn annotation_array_first_value(annotation: &str, key: &str) -> Option<String> {
    let args = annotation_args(annotation)?;
    for part in split_top_level_commas(args) {
        let (found, value) = part.split_once('=')?;
        if found.trim().ends_with(key) {
            return first_string_literal(value);
        }
    }
    None
}

fn annotation_args(annotation: &str) -> Option<&str> {
    python_call_args(annotation)
}

fn python_call_args(text: &str) -> Option<&str> {
    let open = text.find('(')?;
    let close = super::find_matching_paren(text, open).unwrap_or(text.len());
    Some(&text[open + 1..close])
}

fn keyword_string_literal(args: &str, key: &str) -> Option<String> {
    let compact = args.replace(' ', "");
    let needle = format!("{key}=");
    if let Some(pos) = compact.find(&needle) {
        return first_string_literal(&compact[pos + needle.len()..]);
    }
    for part in split_top_level_commas(args) {
        let (found, value) = part.split_once('=')?;
        if found.trim().ends_with(key) {
            return first_string_literal(value);
        }
    }
    None
}

fn first_string_literal(args: &str) -> Option<String> {
    split_top_level_commas(args)
        .first()
        .and_then(|arg| first_raw_string_literal(arg))
}

fn first_raw_string_literal(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let start = trimmed
        .char_indices()
        .find_map(|(idx, ch)| matches!(ch, '\'' | '"' | '`').then_some(idx))?;
    read_string_literal(trimmed, start).map(|(literal, _)| literal)
}
