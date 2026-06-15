use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    clamp_char_boundary, merge_strings, pick_handler_node, process_ids_for_entry,
    property_name_offsets, read_repo_text, read_string_literal, skip_ws, stable_hash,
    web_nodes_by_file, EdgeRec, NativeAppNode, NodeRec, SynthNode, SynthProcess,
};

#[derive(Debug, Clone)]
pub(super) struct SynthTool {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) file_path: String,
    pub(super) emit_node: bool,
    pub(super) description: String,
    pub(super) handler_id: Option<String>,
    pub(super) process_ids: Vec<String>,
}

impl SynthTool {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("toolSource".into(), Value::String("source-scan".into()));
        if !self.description.is_empty() {
            properties.insert(
                "description".into(),
                Value::String(self.description.clone()),
            );
        }
        if let Some(handler_id) = &self.handler_id {
            properties.insert("handlerId".into(), Value::String(handler_id.clone()));
        }
        NodeRec {
            id: self.id.clone(),
            label: "Tool".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::new();
        if let Some(handler_id) = &self.handler_id {
            out.push(EdgeRec {
                id: format!("{}:handles:{:016x}", self.id, stable_hash(handler_id)),
                source_id: handler_id.clone(),
                target_id: self.id.clone(),
                edge_type: "HANDLES_TOOL".into(),
                confidence: 0.6,
                reason: "aka tool synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "tool-handler",
                    "tool": self.name,
                })),
            });
        }
        for process_id in &self.process_ids {
            out.push(EdgeRec {
                id: format!("{}:entry-process:{:016x}", self.id, stable_hash(process_id)),
                source_id: self.id.clone(),
                target_id: process_id.clone(),
                edge_type: "ENTRY_POINT_OF".into(),
                confidence: 0.5,
                reason: "aka tool process linkage".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "tool-entry-process",
                    "tool": self.name,
                })),
            });
        }
        out
    }
}

pub(super) fn synthesize_tools_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    processes: &[SynthProcess],
    native_tools: &[NativeAppNode],
) -> Vec<SynthTool> {
    let mut by_file = web_nodes_by_file(nodes);
    let mut tools: BTreeMap<(String, String), SynthTool> = BTreeMap::new();
    for native in native_tools {
        tools.insert(
            (native.name.clone(), native.file_path.clone()),
            SynthTool {
                id: native.id.clone(),
                name: native.name.clone(),
                file_path: native.file_path.clone(),
                emit_node: false,
                description: String::new(),
                handler_id: None,
                process_ids: process_ids_for_entry(processes, &native.file_path, None),
            },
        );
    }
    for (file_path, file_nodes) in &mut by_file {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        let defs = extract_tool_defs(&text);
        if defs.is_empty() {
            continue;
        }
        let handler = pick_handler_node(file_nodes);
        for def in defs {
            let key = (def.name.clone(), file_path.clone());
            match tools.get_mut(&key) {
                Some(existing) => {
                    if existing.description.is_empty() {
                        existing.description = def.description;
                    }
                    if existing.handler_id.is_none() {
                        existing.handler_id = handler.map(|n| n.aka_id.clone());
                    }
                    merge_strings(
                        &mut existing.process_ids,
                        &process_ids_for_entry(
                            processes,
                            file_path,
                            handler.map(|n| n.aka_id.as_str()),
                        ),
                    );
                }
                None => {
                    tools.insert(
                        key,
                        SynthTool {
                            id: format!(
                                "tool:heuristic:{:016x}",
                                stable_hash(&format!("{}|{file_path}", def.name))
                            ),
                            name: def.name,
                            file_path: file_path.clone(),
                            emit_node: true,
                            description: def.description,
                            handler_id: handler.map(|n| n.aka_id.clone()),
                            process_ids: process_ids_for_entry(
                                processes,
                                file_path,
                                handler.map(|n| n.aka_id.as_str()),
                            ),
                        },
                    );
                }
            }
        }
    }
    let mut out: Vec<SynthTool> = tools.into_values().collect();
    out.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.file_path.cmp(&b.file_path))
    });
    out
}

#[derive(Debug)]
struct ToolDef {
    name: String,
    description: String,
}

fn extract_tool_defs(text: &str) -> Vec<ToolDef> {
    let mut tools: BTreeMap<String, ToolDef> = BTreeMap::new();
    for marker in [".tool(", "server.tool(", "tool("] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(marker) {
            let idx = offset + pos + marker.len();
            let idx = skip_ws(text, idx);
            if let Some((name, end)) = read_string_literal(text, idx) {
                if is_plausible_tool_name(&name) {
                    let desc = extract_description_near(text, end);
                    tools.entry(name.clone()).or_insert(ToolDef {
                        name,
                        description: desc,
                    });
                }
                offset = end;
            } else {
                offset = idx.saturating_add(1);
            }
        }
    }
    for idx in property_name_offsets(text, "name") {
        let window_start = clamp_char_boundary(text, idx.saturating_sub(240));
        let window_end = clamp_char_boundary(text, idx + 400);
        let window = &text[window_start..window_end];
        let lower = window.to_ascii_lowercase();
        if !(lower.contains("tool") || lower.contains("inputschema") || lower.contains("schema")) {
            continue;
        }
        let value_start = skip_ws(text, idx + "name".len());
        let value_start = if text.as_bytes().get(value_start) == Some(&b':') {
            skip_ws(text, value_start + 1)
        } else {
            continue;
        };
        if let Some((name, end)) = read_string_literal(text, value_start) {
            if is_plausible_tool_name(&name) {
                let desc = extract_description_near(text, end);
                tools.entry(name.clone()).or_insert(ToolDef {
                    name,
                    description: desc,
                });
            }
        }
    }
    tools.into_values().collect()
}

fn extract_description_near(text: &str, idx: usize) -> String {
    let start = clamp_char_boundary(text, idx.saturating_sub(120));
    let end = clamp_char_boundary(text, idx + 600);
    let window = &text[start..end];
    for key in ["description", "title"] {
        if let Some(pos) = window.find(key) {
            let colon = skip_ws(window, pos + key.len());
            let value_start = if window.as_bytes().get(colon) == Some(&b':') {
                skip_ws(window, colon + 1)
            } else {
                continue;
            };
            if let Some((desc, _)) = read_string_literal(window, value_start) {
                return desc.chars().take(240).collect();
            }
        }
    }
    String::new()
}

fn is_plausible_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/'))
        && name.chars().any(|ch| ch.is_ascii_alphabetic())
}

#[cfg(test)]
pub(super) fn extract_tool_defs_for_tests(text: &str) -> Vec<(String, String)> {
    extract_tool_defs(text)
        .into_iter()
        .map(|tool| (tool.name, tool.description))
        .collect()
}
