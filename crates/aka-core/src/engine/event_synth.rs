use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, find_matching_paren, node_at_offset, pick_handler_node,
    project_code_nodes_by_file, read_repo_text, source_annotations_before_node,
    split_top_level_commas, stable_hash, EdgeRec, NodeRec, ProjectSourceSet, SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct SynthEvent {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) bus: String,
    pub(super) publishers: Vec<SynthEventEndpoint>,
    pub(super) handlers: Vec<SynthEventEndpoint>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct SynthEventEndpoint {
    node_id: String,
    file_path: String,
    strategy: String,
    metadata: BTreeMap<String, String>,
}

impl SynthEvent {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("bus".into(), Value::String(self.bus.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("eventSource".into(), Value::String("source-scan".into()));
        NodeRec {
            id: self.id.clone(),
            label: "Event".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::new();
        for endpoint in &self.publishers {
            out.push(self.edge_rec(endpoint, "PUBLISHES_EVENT", "event-publisher"));
        }
        for endpoint in &self.handlers {
            out.push(self.edge_rec(endpoint, "HANDLES_EVENT", "event-handler"));
        }
        out
    }

    fn edge_rec(&self, endpoint: &SynthEventEndpoint, edge_type: &str, kind: &str) -> EdgeRec {
        EdgeRec {
            id: format!(
                "{}:{}:{:016x}",
                self.id,
                edge_type.to_ascii_lowercase(),
                stable_hash(&format!(
                    "{}|{}|{}",
                    endpoint.node_id, endpoint.strategy, edge_type
                ))
            ),
            source_id: endpoint.node_id.clone(),
            target_id: self.id.clone(),
            edge_type: edge_type.into(),
            confidence: 0.69,
            reason: "aka event synthesis".into(),
            step: None,
            evidence: Some(json!({
                "source": "aka-cbm-synth",
                "kind": kind,
                "bus": self.bus,
                "event": self.name,
                "strategy": endpoint.strategy,
                "filePath": endpoint.file_path,
                "metadata": endpoint.metadata,
            })),
        }
    }
}

pub(super) fn synthesize_events_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> Vec<SynthEvent> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut events: BTreeMap<(String, String), SynthEvent> = BTreeMap::new();
    let mut seen_edges: HashSet<(String, String, String, String)> = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for detection in extract_event_detections(&text, &file_path, &file_nodes) {
            let key = (detection.bus.clone(), detection.name.clone());
            let id = format!(
                "event:heuristic:{:016x}",
                stable_hash(&format!("{}|{}", detection.bus, detection.name))
            );
            let event = events.entry(key).or_insert_with(|| SynthEvent {
                id,
                name: detection.name.clone(),
                bus: detection.bus.clone(),
                publishers: Vec::new(),
                handlers: Vec::new(),
            });
            let edge_key = (
                detection.kind.as_str().to_string(),
                detection.bus,
                detection.name,
                detection.node_id.clone(),
            );
            if !seen_edges.insert(edge_key) {
                continue;
            }
            let endpoint = SynthEventEndpoint {
                node_id: detection.node_id,
                file_path: file_path.clone(),
                strategy: detection.strategy,
                metadata: detection.metadata,
            };
            match detection.kind {
                EventEndpointKind::Publisher => event.publishers.push(endpoint),
                EventEndpointKind::Handler => event.handlers.push(endpoint),
            }
        }
    }
    let mut out: Vec<SynthEvent> = events.into_values().collect();
    for event in &mut out {
        event.publishers.sort();
        event.publishers.dedup();
        event.handlers.sort();
        event.handlers.dedup();
    }
    out.sort_by(|a, b| a.bus.cmp(&b.bus).then_with(|| a.name.cmp(&b.name)));
    out
}

#[derive(Debug, Clone, Copy)]
enum EventEndpointKind {
    Publisher,
    Handler,
}

impl EventEndpointKind {
    fn as_str(self) -> &'static str {
        match self {
            EventEndpointKind::Publisher => "publisher",
            EventEndpointKind::Handler => "handler",
        }
    }
}

#[derive(Debug, Clone)]
struct EventDetection {
    name: String,
    bus: String,
    kind: EventEndpointKind,
    node_id: String,
    strategy: String,
    metadata: BTreeMap<String, String>,
}

fn event_detection(
    name: String,
    bus: &str,
    kind: EventEndpointKind,
    node_id: String,
    strategy: String,
) -> EventDetection {
    EventDetection {
        name,
        bus: bus.into(),
        kind,
        node_id,
        strategy,
        metadata: BTreeMap::new(),
    }
}

fn extract_event_detections(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
) -> Vec<EventDetection> {
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
        out.extend(extract_jvm_event_detections(text, nodes));
    }
    if lower.ends_with(".py")
        || nodes
            .iter()
            .any(|node| node.language.eq_ignore_ascii_case("python"))
    {
        out.extend(extract_python_event_detections(text, nodes));
    }
    out.sort_by(|a, b| {
        a.bus
            .cmp(&b.bus)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out
}

fn extract_jvm_event_detections(text: &str, nodes: &[&SynthNode]) -> Vec<EventDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        for decorator in decorators_for_node(text, node) {
            let Some(name) = decorator_name(&decorator) else {
                continue;
            };
            if !matches!(name, "EventListener" | "TransactionalEventListener") {
                continue;
            }
            for event_name in event_names_from_listener(&decorator, node) {
                let mut detection = event_detection(
                    event_name,
                    "spring-application-event",
                    EventEndpointKind::Handler,
                    node.aka_id.clone(),
                    format!("java-spring-{}", name.to_ascii_lowercase()),
                );
                detection.metadata = spring_event_listener_metadata(&decorator);
                out.push(detection);
            }
        }
    }
    out.extend(extract_call_event_literals(
        text,
        nodes,
        ".publishEvent",
        "spring-application-event",
        EventEndpointKind::Publisher,
        "java-spring-publish-event",
        0,
    ));
    out
}

fn extract_python_event_detections(text: &str, nodes: &[&SynthNode]) -> Vec<EventDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        for decorator in decorators_for_node(text, node) {
            let normalized = decorator.trim().trim_start_matches('@');
            if !normalized.starts_with("receiver(") && !normalized.contains(".receiver(") {
                continue;
            }
            for event_name in python_decorator_event_names(normalized) {
                out.push(EventDetection {
                    name: event_name,
                    bus: "python-signal".into(),
                    kind: EventEndpointKind::Handler,
                    node_id: node.aka_id.clone(),
                    strategy: "python-signal-receiver".into(),
                    metadata: BTreeMap::new(),
                });
            }
        }
    }
    out.extend(extract_python_signal_sends(text, nodes));
    out.extend(extract_python_signal_definitions(text, nodes));
    out
}

fn decorators_for_node(text: &str, node: &SynthNode) -> Vec<String> {
    let mut decorators = node.decorators.clone();
    decorators.extend(source_annotations_before_node(text, node));
    decorators.sort();
    decorators.dedup();
    decorators
}

fn event_names_from_listener(decorator: &str, node: &SynthNode) -> Vec<String> {
    let mut out = annotation_string_values(decorator, &["classes", "value"]);
    if out.is_empty() {
        out.extend(method_param_type_names(node));
    }
    if out.is_empty() {
        out.push(node.display_name().to_string());
    }
    normalize_event_names(out)
}

fn method_param_type_names(node: &SynthNode) -> Vec<String> {
    let Some((_, params)) = node.qn.rsplit_once('(') else {
        return Vec::new();
    };
    let params = params.trim_end_matches(')');
    params
        .split(',')
        .map(str::trim)
        .filter(|param| is_event_name(param))
        .map(|param| param.rsplit('.').next().unwrap_or(param).to_string())
        .collect()
}

fn python_decorator_event_names(decorator: &str) -> Vec<String> {
    let Some(open) = decorator.find('(') else {
        return Vec::new();
    };
    let close = find_matching_paren(decorator, open).unwrap_or(decorator.len());
    let args = &decorator[open + 1..close];
    let mut out = Vec::new();
    if let Some(first) = split_top_level_commas(args).first() {
        out.extend(event_name_literals(first));
        if out.is_empty() {
            out.extend(first_type_token(first));
        }
    }
    normalize_event_names(out)
}

fn extract_call_event_literals(
    text: &str,
    nodes: &[&SynthNode],
    callee: &str,
    bus: &str,
    kind: EventEndpointKind,
    strategy: &str,
    arg_index: usize,
) -> Vec<EventDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, callee) {
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        let args = split_top_level_commas(call.args);
        let Some(arg) = args.get(arg_index) else {
            continue;
        };
        for name in event_names_from_expr(arg) {
            out.push(EventDetection {
                name,
                bus: bus.into(),
                kind,
                node_id: node.aka_id.clone(),
                strategy: strategy.into(),
                metadata: BTreeMap::new(),
            });
        }
    }
    out
}

fn extract_python_signal_sends(text: &str, nodes: &[&SynthNode]) -> Vec<EventDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in [
        (".send", "python-signal-send"),
        (".send_robust", "python-signal-send-robust"),
    ] {
        for call in find_call_args(text, callee) {
            let Some(node) =
                node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
            else {
                continue;
            };
            let Some(name) = receiver_ident_before(text, call.start) else {
                continue;
            };
            for name in normalize_event_names(vec![name]) {
                out.push(EventDetection {
                    name,
                    bus: "python-signal".into(),
                    kind: EventEndpointKind::Publisher,
                    node_id: node.aka_id.clone(),
                    strategy: strategy.into(),
                    metadata: BTreeMap::new(),
                });
            }
        }
    }
    out
}

fn extract_python_signal_definitions(text: &str, nodes: &[&SynthNode]) -> Vec<EventDetection> {
    let mut out = Vec::new();
    for marker in ["Signal(", "signal("] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(marker) {
            let start = offset + pos;
            let Some(name) = assigned_name_before(text, start) else {
                offset = start + marker.len();
                continue;
            };
            let Some(node) = node_at_offset(text, nodes, start) else {
                offset = start + marker.len();
                continue;
            };
            out.push(EventDetection {
                name,
                bus: "python-signal".into(),
                kind: EventEndpointKind::Publisher,
                node_id: node.aka_id.clone(),
                strategy: "python-signal-definition".into(),
                metadata: BTreeMap::new(),
            });
            offset = start + marker.len();
        }
    }
    out
}

fn spring_event_listener_metadata(decorator: &str) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    let Some(open) = decorator.find('(') else {
        return metadata;
    };
    let close = find_matching_paren(decorator, open).unwrap_or(decorator.len());
    let args = &decorator[open + 1..close];
    for key in ["condition", "phase"] {
        if let Some(value) = annotation_value(args, key) {
            metadata.insert(key.into(), value);
        }
    }
    metadata
}

fn annotation_value(args: &str, expected: &str) -> Option<String> {
    for part in split_top_level_commas(args) {
        let (key, value) = part.split_once('=')?;
        if !key.trim().ends_with(expected) {
            continue;
        }
        return event_name_literals(value)
            .into_iter()
            .next()
            .or_else(|| Some(value.trim().trim_matches('"').to_string()));
    }
    None
}

fn receiver_ident_before(text: &str, dot_start: usize) -> Option<String> {
    let before = text.get(..dot_start)?;
    let mut end = before.len();
    while end > 0 && before.as_bytes()[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 {
        let ch = before[..start].chars().next_back()?;
        if ch == '_' || ch.is_ascii_alphanumeric() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let ident = before[start..end].trim();
    if is_ident(ident) {
        Some(ident.to_string())
    } else {
        None
    }
}

fn assigned_name_before(text: &str, call_start: usize) -> Option<String> {
    let line_start = text[..call_start].rfind('\n').map_or(0, |idx| idx + 1);
    let before = text[line_start..call_start].trim();
    let lhs = before.split_once('=')?.0.trim();
    if is_ident(lhs) {
        Some(lhs.to_string())
    } else {
        None
    }
}

fn event_names_from_expr(expr: &str) -> Vec<String> {
    let mut out = event_name_literals(expr);
    if out.is_empty() {
        out.extend(first_type_token(expr));
    }
    normalize_event_names(out)
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
        values.extend(event_name_literals(value));
        if values.is_empty() {
            values.extend(first_type_token(value));
        }
    }
    normalize_event_names(values)
}

fn event_name_literals(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some(byte) = text.as_bytes().get(idx).copied() else {
            break;
        };
        if matches!(byte, b'\'' | b'"' | b'`') {
            if let Some((literal, end)) = read_raw_string_literal(text, idx) {
                if is_event_name(&literal) {
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

fn first_type_token(text: &str) -> Vec<String> {
    let cleaned = text
        .trim()
        .trim_start_matches("new ")
        .split(['(', ',', '#', '{'])
        .next()
        .unwrap_or("")
        .trim();
    if is_event_name(cleaned) {
        vec![cleaned.to_string()]
    } else {
        Vec::new()
    }
}

fn normalize_event_names(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = values
        .into_iter()
        .map(|value| {
            value
                .trim()
                .trim_end_matches(".class")
                .rsplit('.')
                .next()
                .unwrap_or("")
                .to_string()
        })
        .filter(|value| is_event_name(value))
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

fn is_event_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':'))
        && value.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn is_ident(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
