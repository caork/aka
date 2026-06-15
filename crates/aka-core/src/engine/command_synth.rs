use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, process_ids_for_entry, project_code_nodes_by_file, read_repo_text,
    read_string_literal, split_top_level_commas, stable_hash, EdgeRec, NodeRec, ProjectSourceSet,
    SynthNode, SynthProcess,
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
    handler_node: Option<SynthCommandHandlerNode>,
}

impl SynthCommand {
    pub(super) fn handler_node_rec(&self) -> Option<NodeRec> {
        self.handler_node
            .as_ref()
            .map(SynthCommandHandlerNode::node_rec)
    }

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
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for file_path in project_sources
        .project_files(repo)
        .filter(|path| is_jvm_source_path(path) || by_file.contains_key(*path))
    {
        if !is_jvm_source_path(file_path) {
            continue;
        }
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        let file_nodes = by_file.get(file_path).map(Vec::as_slice).unwrap_or(&[]);
        for (handler, detection) in detect_spring_runner_commands(file_path, &text, file_nodes) {
            push_command(
                &mut out, &mut seen, file_path, processes, handler, detection,
            );
        }
    }

    for (file_path, file_nodes) in by_file {
        let text = read_repo_text(repo, &file_path);
        for node in file_nodes
            .iter()
            .copied()
            .filter(|node| matches!(node.label.as_str(), "Class" | "Function" | "Method"))
        {
            for detection in detect_node_commands(text.as_deref(), node) {
                push_command(
                    &mut out,
                    &mut seen,
                    file_path.as_str(),
                    processes,
                    CommandHandler::Existing(node),
                    detection,
                );
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

fn push_command(
    out: &mut Vec<SynthCommand>,
    seen: &mut HashSet<String>,
    file_path: &str,
    processes: &[SynthProcess],
    handler: CommandHandler<'_>,
    detection: CommandDetection,
) {
    let handler_id = handler.id().to_string();
    let handler_name = handler.name().to_string();
    let key = format!(
        "{}|{}|{}|{}",
        handler_id, detection.command_type, detection.name, detection.strategy
    );
    if !seen.insert(key.clone()) {
        return;
    }
    out.push(SynthCommand {
        id: format!("command:heuristic:{:016x}", stable_hash(&key)),
        name: detection.name,
        command_type: detection.command_type,
        file_path: file_path.to_string(),
        handler_id: handler_id.clone(),
        handler_name,
        strategy: detection.strategy,
        process_ids: process_ids_for_entry(processes, file_path, Some(&handler_id)),
        handler_node: handler.synthetic_node().cloned(),
    });
}

#[derive(Debug, Clone)]
struct CommandDetection {
    name: String,
    command_type: String,
    strategy: String,
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

fn detect_jvm_commands(_text: Option<&str>, node: &SynthNode) -> Vec<CommandDetection> {
    let mut out = Vec::new();
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

fn detect_spring_runner_commands<'a>(
    file_path: &str,
    text: &str,
    nodes: &[&'a SynthNode],
) -> Vec<(CommandHandler<'a>, CommandDetection)> {
    spring_runner_candidates(text)
        .into_iter()
        .map(|candidate| {
            let handler = candidate
                .pick_handler(nodes)
                .map(CommandHandler::Existing)
                .unwrap_or_else(|| {
                    CommandHandler::Synthetic(candidate.synthetic_handler(file_path, text))
                });
            (
                handler,
                CommandDetection {
                    name: format!("{} runner", candidate.display_name()),
                    command_type: "spring-runner".into(),
                    strategy: candidate.strategy().into(),
                },
            )
        })
        .collect()
}

#[derive(Debug, Clone)]
struct SynthCommandHandlerNode {
    id: String,
    label: String,
    name: String,
    qn: String,
    file_path: String,
    start_line: i64,
    end_line: i64,
    language: String,
    strategy: String,
}

impl SynthCommandHandlerNode {
    fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("qualifiedName".into(), Value::String(self.qn.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert(
            "startLine".into(),
            Value::from(to_artifact_line(self.start_line)),
        );
        properties.insert(
            "endLine".into(),
            Value::from(to_artifact_line(self.end_line)),
        );
        properties.insert("language".into(), Value::String(self.language.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("strategy".into(), Value::String(self.strategy.clone()));
        NodeRec {
            id: self.id.clone(),
            label: self.label.clone(),
            properties,
        }
    }
}

enum CommandHandler<'a> {
    Existing(&'a SynthNode),
    Synthetic(SynthCommandHandlerNode),
}

impl<'a> CommandHandler<'a> {
    fn id(&self) -> &str {
        match self {
            CommandHandler::Existing(node) => &node.aka_id,
            CommandHandler::Synthetic(node) => &node.id,
        }
    }

    fn name(&self) -> &str {
        match self {
            CommandHandler::Existing(node) => node.display_name(),
            CommandHandler::Synthetic(node) => &node.name,
        }
    }

    fn synthetic_node(&self) -> Option<&SynthCommandHandlerNode> {
        match self {
            CommandHandler::Existing(_) => None,
            CommandHandler::Synthetic(node) => Some(node),
        }
    }
}

#[derive(Debug, Clone)]
enum SpringRunnerCandidate {
    Class {
        class_name: String,
        line: i64,
    },
    BeanMethod {
        method_name: String,
        owner_class: Option<String>,
        line: i64,
    },
}

impl SpringRunnerCandidate {
    fn strategy(&self) -> &'static str {
        match self {
            SpringRunnerCandidate::Class { .. } => "java-spring-runner-source-declaration",
            SpringRunnerCandidate::BeanMethod { .. } => {
                "java-spring-runner-bean-source-declaration"
            }
        }
    }

    fn display_name(&self) -> &str {
        match self {
            SpringRunnerCandidate::Class { class_name, .. } => class_name,
            SpringRunnerCandidate::BeanMethod { method_name, .. } => method_name,
        }
    }

    fn pick_handler<'a>(&self, nodes: &[&'a SynthNode]) -> Option<&'a SynthNode> {
        match self {
            SpringRunnerCandidate::Class { class_name, line } => nodes
                .iter()
                .copied()
                .filter(|node| node.label == "Class" && node_simple_name_matches(node, class_name))
                .min_by_key(|node| line_distance(*line, node.start_line_key()))
                .or_else(|| {
                    nodes
                        .iter()
                        .copied()
                        .filter(|node| node.label == "Method" && node.name == "run")
                        .filter(|node| {
                            node.parent_class
                                .as_deref()
                                .is_some_and(|parent| simple_name_matches(parent, class_name))
                        })
                        .min_by_key(|node| line_distance(*line, node.start_line_key()))
                }),
            SpringRunnerCandidate::BeanMethod {
                method_name, line, ..
            } => nodes
                .iter()
                .copied()
                .filter(|node| node.label == "Method" && node.name == *method_name)
                .min_by_key(|node| line_distance(*line, node.start_line_key())),
        }
    }

    fn synthetic_handler(&self, file_path: &str, text: &str) -> SynthCommandHandlerNode {
        let package = java_package_name(text);
        let (label, name, owner, line) = match self {
            SpringRunnerCandidate::Class { class_name, line } => {
                ("Class", class_name.as_str(), None, *line)
            }
            SpringRunnerCandidate::BeanMethod {
                method_name,
                owner_class,
                line,
            } => (
                "Method",
                method_name.as_str(),
                owner_class.as_deref(),
                *line,
            ),
        };
        let qn = java_qualified_name(package.as_deref(), owner, name);
        let strategy = self.strategy().to_string();
        let key = format!("{file_path}|{qn}|{strategy}|{line}");
        SynthCommandHandlerNode {
            id: format!("command-handler:source:{:016x}", stable_hash(&key)),
            label: label.into(),
            name: name.into(),
            qn,
            file_path: file_path.into(),
            start_line: line,
            end_line: line,
            language: "java".into(),
            strategy,
        }
    }
}

fn spring_runner_candidates(text: &str) -> Vec<SpringRunnerCandidate> {
    let mut out = Vec::new();
    out.extend(spring_runner_class_candidates(text));
    out.extend(spring_runner_bean_method_candidates(text));
    out.sort_by(|a, b| candidate_key(a).cmp(&candidate_key(b)));
    out.dedup_by(|a, b| candidate_key(a) == candidate_key(b));
    out
}

fn candidate_key(candidate: &SpringRunnerCandidate) -> (u8, &str) {
    match candidate {
        SpringRunnerCandidate::Class { class_name, .. } => (0, class_name.as_str()),
        SpringRunnerCandidate::BeanMethod { method_name, .. } => (1, method_name.as_str()),
    }
}

fn spring_runner_class_candidates(text: &str) -> Vec<SpringRunnerCandidate> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find("class") {
        let class_pos = offset + rel;
        if !keyword_boundary_ok(text, class_pos, "class") {
            offset = class_pos + "class".len();
            continue;
        }
        let name_start = skip_java_ws_and_modifiers(text, class_pos + "class".len());
        let Some((class_name, _name_end)) = read_java_identifier(text, name_start) else {
            offset = class_pos + "class".len();
            continue;
        };
        let Some(open_brace_rel) = text[class_pos..].find('{') else {
            break;
        };
        let declaration = &text[class_pos..class_pos + open_brace_rel];
        if declaration_mentions_runner(declaration) {
            out.push(SpringRunnerCandidate::Class {
                class_name: class_name.to_string(),
                line: line_number_at_offset(text, class_pos),
            });
        }
        offset = class_pos + open_brace_rel + 1;
    }
    out
}

fn spring_runner_bean_method_candidates(text: &str) -> Vec<SpringRunnerCandidate> {
    let mut out = Vec::new();
    for runner_type in ["CommandLineRunner", "ApplicationRunner"] {
        let mut offset = 0usize;
        while let Some(rel) = text[offset..].find(runner_type) {
            let type_pos = offset + rel;
            let search_end = text[type_pos..]
                .find(['{', ';', '\n'])
                .map(|rel| type_pos + rel)
                .unwrap_or(text.len());
            if let Some((method_name, name_end)) =
                read_java_identifier(text, skip_java_ws(text, type_pos + runner_type.len()))
            {
                let after_name = skip_java_ws(text, name_end);
                if after_name < search_end
                    && text.as_bytes().get(after_name) == Some(&b'(')
                    && bean_annotation_before(text, type_pos)
                {
                    out.push(SpringRunnerCandidate::BeanMethod {
                        method_name: method_name.to_string(),
                        owner_class: enclosing_java_type_name(text, type_pos),
                        line: line_number_at_offset(text, type_pos),
                    });
                }
            }
            offset = type_pos + runner_type.len();
        }
    }
    out
}

fn declaration_mentions_runner(declaration: &str) -> bool {
    declaration.contains("implements")
        && (declaration.contains("CommandLineRunner") || declaration.contains("ApplicationRunner"))
}

fn is_jvm_source_path(path: &str) -> bool {
    matches!(
        Path::new(&path.to_ascii_lowercase())
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("java" | "kt" | "kts" | "scala" | "groovy")
    )
}

fn java_package_name(text: &str) -> Option<String> {
    for line in text.lines().take(80) {
        let trimmed = line.trim();
        if !trimmed.starts_with("package ") {
            continue;
        }
        return trimmed
            .trim_start_matches("package ")
            .trim_end_matches(';')
            .split_whitespace()
            .next()
            .filter(|value| !value.is_empty())
            .map(str::to_string);
    }
    None
}

fn java_qualified_name(package: Option<&str>, owner: Option<&str>, name: &str) -> String {
    let mut parts = Vec::new();
    if let Some(package) = package.filter(|value| !value.is_empty()) {
        parts.push(package);
    }
    if let Some(owner) = owner.filter(|value| !value.is_empty()) {
        parts.push(owner);
    }
    parts.push(name);
    parts.join(".")
}

fn enclosing_java_type_name(text: &str, pos: usize) -> Option<String> {
    let prefix = &text[..pos.min(text.len())];
    let mut offset = 0usize;
    let mut last = None;
    while offset < prefix.len() {
        let next = ["class", "interface", "record"]
            .iter()
            .filter_map(|keyword| {
                prefix[offset..]
                    .find(keyword)
                    .map(|rel| (offset + rel, *keyword))
            })
            .min_by_key(|(idx, _)| *idx);
        let Some((type_pos, keyword)) = next else {
            break;
        };
        if !keyword_boundary_ok(prefix, type_pos, keyword) {
            offset = type_pos + keyword.len();
            continue;
        }
        let name_start = skip_java_ws_and_modifiers(prefix, type_pos + keyword.len());
        if let Some((name, end)) = read_java_identifier(prefix, name_start) {
            last = Some(name.to_string());
            offset = end;
        } else {
            offset = type_pos + keyword.len();
        }
    }
    last
}

fn bean_annotation_before(text: &str, pos: usize) -> bool {
    let window_start = pos.saturating_sub(500);
    text[window_start..pos].contains("@Bean")
}

fn keyword_boundary_ok(text: &str, start: usize, keyword: &str) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|idx| text.as_bytes().get(idx))
        .copied()
        .map(char::from);
    let after = text
        .as_bytes()
        .get(start + keyword.len())
        .copied()
        .map(char::from);
    before.is_none_or(|ch| !is_java_ident_continue(ch))
        && after.is_none_or(|ch| !is_java_ident_continue(ch))
}

fn skip_java_ws_and_modifiers(text: &str, mut idx: usize) -> usize {
    loop {
        idx = skip_java_ws(text, idx);
        let Some((word, end)) = read_java_identifier(text, idx) else {
            return idx;
        };
        if matches!(
            word,
            "public" | "protected" | "private" | "abstract" | "final" | "sealed" | "static"
        ) {
            idx = end;
            continue;
        }
        return idx;
    }
}

fn skip_java_ws(text: &str, mut idx: usize) -> usize {
    let bytes = text.as_bytes();
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    idx
}

fn read_java_identifier(text: &str, start: usize) -> Option<(&str, usize)> {
    let mut chars = text[start..].char_indices();
    let (_, first) = chars.next()?;
    if !is_java_ident_start(first) {
        return None;
    }
    let mut end = start + first.len_utf8();
    for (rel, ch) in chars {
        if !is_java_ident_continue(ch) {
            break;
        }
        end = start + rel + ch.len_utf8();
    }
    Some((&text[start..end], end))
}

fn is_java_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || matches!(ch, '_' | '$')
}

fn is_java_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$')
}

fn node_simple_name_matches(node: &SynthNode, simple_name: &str) -> bool {
    node.name == simple_name || simple_name_matches(&node.qn, simple_name)
}

fn simple_name_matches(qn: &str, simple_name: &str) -> bool {
    qn.rsplit(['.', '$']).next() == Some(simple_name)
}

fn line_distance(a: i64, b: i64) -> i64 {
    a.saturating_sub(b).abs()
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

fn to_artifact_line(line_1based: i64) -> u32 {
    if line_1based <= 0 {
        0
    } else {
        (line_1based - 1) as u32
    }
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
