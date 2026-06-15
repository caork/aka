use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::{
    find_call_args, node_at_offset, read_string_literal, split_top_level_commas, stable_hash,
    EdgeRec, SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct TableAccessEntity {
    pub(super) entity_id: String,
    pub(super) entity_name: String,
    pub(super) table_id: String,
    pub(super) table_name: String,
}

#[derive(Debug, Clone)]
pub(super) struct TableAccessRef {
    pub(super) table_id: String,
    pub(super) table_name: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum TableAccessKind {
    Read,
    Write,
}

impl TableAccessKind {
    fn edge_type(self) -> &'static str {
        match self {
            TableAccessKind::Read => "READS_TABLE",
            TableAccessKind::Write => "WRITES_TABLE",
        }
    }

    fn operation(self) -> &'static str {
        match self {
            TableAccessKind::Read => "read",
            TableAccessKind::Write => "write",
        }
    }
}

#[derive(Debug, Clone)]
struct TableAccessDetection {
    table: TableAccessRef,
    node_id: String,
    kind: TableAccessKind,
    strategy: String,
}

pub(super) fn detect_table_access_edges(
    text: &str,
    nodes: &[&SynthNode],
    table_lookup: &BTreeMap<String, TableAccessRef>,
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Vec<EdgeRec> {
    let mut out = Vec::new();
    for detection in detect_sql_literal_table_accesses(text, nodes, table_lookup)
        .into_iter()
        .chain(detect_annotation_table_accesses(nodes, table_lookup))
        .chain(detect_orm_table_accesses(text, nodes, entities))
    {
        out.push(table_access_edge(detection));
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

pub(super) fn normalize_table_access_key(name: &str) -> String {
    name.trim()
        .trim_matches(['"', '\'', '`'])
        .trim_matches(['[', ']'])
        .rsplit('.')
        .next()
        .unwrap_or("")
        .trim_matches(['"', '\'', '`'])
        .trim_matches(['[', ']'])
        .to_ascii_lowercase()
}

fn detect_sql_literal_table_accesses(
    text: &str,
    nodes: &[&SynthNode],
    table_lookup: &BTreeMap<String, TableAccessRef>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    for (offset, literal) in string_literal_occurrences(text) {
        let Some(node) = node_at_offset(text, nodes, offset) else {
            continue;
        };
        for (kind, table, strategy) in sql_table_accesses(&literal, table_lookup) {
            out.push(TableAccessDetection {
                table,
                node_id: node.aka_id.clone(),
                kind,
                strategy,
            });
        }
    }
    out
}

fn detect_annotation_table_accesses(
    nodes: &[&SynthNode],
    table_lookup: &BTreeMap<String, TableAccessRef>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    for node in nodes.iter().copied() {
        if !matches!(node.label.as_str(), "Function" | "Method") {
            continue;
        }
        for decorator in &node.decorators {
            let Some(name) = decorator_name(decorator) else {
                continue;
            };
            if !matches!(name, "Query" | "NativeQuery" | "NamedQuery") {
                continue;
            }
            let Some(query) = first_raw_string_literal(decorator) else {
                continue;
            };
            for (kind, table, _) in sql_table_accesses(&query, table_lookup) {
                out.push(TableAccessDetection {
                    table,
                    node_id: node.aka_id.clone(),
                    kind,
                    strategy: format!("java-annotation-{}", name.to_ascii_lowercase()),
                });
            }
        }
    }
    out
}

fn detect_orm_table_accesses(
    text: &str,
    nodes: &[&SynthNode],
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, ".query") {
        let Some(entity) = split_top_level_commas(call.args)
            .first()
            .and_then(|arg| first_type_token(arg))
            .and_then(|name| entities.get(&name).cloned())
        else {
            continue;
        };
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        out.push(TableAccessDetection {
            table: TableAccessRef {
                table_id: entity.table_id,
                table_name: entity.table_name,
            },
            node_id: node.aka_id.clone(),
            kind: TableAccessKind::Read,
            strategy: "orm-query-entity".into(),
        });
    }
    for call in find_call_args(text, "select") {
        let Some(entity) = split_top_level_commas(call.args)
            .first()
            .and_then(|arg| first_type_token(arg))
            .and_then(|name| entities.get(&name).cloned())
        else {
            continue;
        };
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        out.push(TableAccessDetection {
            table: TableAccessRef {
                table_id: entity.table_id,
                table_name: entity.table_name,
            },
            node_id: node.aka_id.clone(),
            kind: TableAccessKind::Read,
            strategy: "orm-select-entity".into(),
        });
    }
    for entity in unique_entities(entities) {
        for marker in [
            format!("{}.objects.", entity.entity_name),
            format!("{}.query.", entity.entity_name),
        ] {
            let mut offset = 0usize;
            while let Some(pos) = text[offset..].find(&marker) {
                let start = offset + pos;
                if let Some(node) = node_at_offset(text, nodes, start) {
                    out.push(TableAccessDetection {
                        table: TableAccessRef {
                            table_id: entity.table_id.clone(),
                            table_name: entity.table_name.clone(),
                        },
                        node_id: node.aka_id.clone(),
                        kind: TableAccessKind::Read,
                        strategy: "orm-entity-manager".into(),
                    });
                }
                offset = start + marker.len();
            }
        }
    }
    out
}

fn table_access_edge(detection: TableAccessDetection) -> EdgeRec {
    EdgeRec {
        id: format!(
            "table-access:heuristic:{:016x}",
            stable_hash(&format!(
                "{}|{}|{}|{}",
                detection.node_id,
                detection.table.table_id,
                detection.kind.edge_type(),
                detection.strategy
            ))
        ),
        source_id: detection.node_id,
        target_id: detection.table.table_id,
        edge_type: detection.kind.edge_type().into(),
        confidence: 0.66,
        reason: "aka table access synthesis".into(),
        step: None,
        evidence: Some(json!({
            "source": "aka-cbm-synth",
            "kind": "table-access",
            "operation": detection.kind.operation(),
            "table": detection.table.table_name,
            "strategy": detection.strategy,
        })),
    }
}

fn sql_table_accesses(
    sql: &str,
    table_lookup: &BTreeMap<String, TableAccessRef>,
) -> Vec<(TableAccessKind, TableAccessRef, String)> {
    let mut out: Vec<(TableAccessKind, TableAccessRef, String)> = Vec::new();
    for name in sql_names_after(sql, " from ") {
        if let Some(table) = table_lookup.get(&normalize_table_access_key(&name)) {
            out.push((TableAccessKind::Read, table.clone(), "sql-select-from".into()));
        }
    }
    for name in sql_names_after(sql, " join ") {
        if let Some(table) = table_lookup.get(&normalize_table_access_key(&name)) {
            out.push((TableAccessKind::Read, table.clone(), "sql-select-join".into()));
        }
    }
    for (keyword, strategy) in [
        ("update ", "sql-update"),
        ("insert into ", "sql-insert"),
        ("delete from ", "sql-delete"),
        ("merge into ", "sql-merge"),
    ] {
        for name in sql_names_after(sql, keyword) {
            if let Some(table) = table_lookup.get(&normalize_table_access_key(&name)) {
                out.push((TableAccessKind::Write, table.clone(), strategy.into()));
            }
        }
    }
    out.sort_by(|a, b| {
        a.0.edge_type()
            .cmp(b.0.edge_type())
            .then_with(|| a.1.table_id.cmp(&b.1.table_id))
            .then_with(|| a.2.cmp(&b.2))
    });
    out.dedup_by(|a, b| a.0 == b.0 && a.1.table_id == b.1.table_id && a.2 == b.2);
    out
}

fn sql_names_after(sql: &str, keyword: &str) -> Vec<String> {
    let lower = sql.to_ascii_lowercase();
    let keyword_lower = keyword.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = lower[offset..].find(&keyword_lower) {
        let start = offset + pos + keyword.len();
        if let Some(name) = read_sql_identifier(sql, start) {
            out.push(name);
        }
        offset = start;
    }
    out
}

fn read_sql_identifier(sql: &str, start: usize) -> Option<String> {
    let mut idx = start;
    let bytes = sql.as_bytes();
    while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    if matches!(bytes.get(idx), Some(b'(')) {
        return None;
    }
    let ident_start = idx;
    while idx < bytes.len() {
        let ch = sql[idx..].chars().next()?;
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '"' | '\'' | '`' | '[' | ']') {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }
    (idx > ident_start).then(|| sql[ident_start..idx].to_string())
}

fn string_literal_occurrences(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some(byte) = text.as_bytes().get(idx).copied() else {
            break;
        };
        if matches!(byte, b'\'' | b'"' | b'`') {
            if let Some((literal, end)) = read_string_literal(text, idx) {
                out.push((idx, literal));
                idx = end;
                continue;
            }
        }
        idx += 1;
    }
    out
}

fn unique_entities(entities: &BTreeMap<String, TableAccessEntity>) -> Vec<TableAccessEntity> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for entity in entities.values() {
        if seen.insert(entity.entity_id.clone()) {
            out.push(entity.clone());
        }
    }
    out
}

fn decorator_name(decorator: &str) -> Option<&str> {
    let text = decorator.trim().trim_start_matches('@');
    let name = text.split_once('(').map(|(name, _)| name).unwrap_or(text);
    Some(name.rsplit('.').next().unwrap_or(name).trim()).filter(|name| !name.is_empty())
}

fn first_raw_string_literal(text: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < text.len() {
        let byte = text.as_bytes().get(idx).copied()?;
        if matches!(byte, b'\'' | b'"' | b'`') {
            return read_string_literal(text, idx).map(|(literal, _)| literal);
        }
        idx += 1;
    }
    None
}

fn first_type_token(text: &str) -> Option<String> {
    let token = text.trim().split(['(', ',', '#']).next()?.trim();
    is_type_name(token).then(|| token.to_string())
}

fn is_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_uppercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
