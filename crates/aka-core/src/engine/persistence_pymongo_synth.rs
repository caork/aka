use std::collections::{BTreeMap, BTreeSet};

use super::persistence_access_synth::{
    normalize_table_access_key, TableAccessDetection, TableAccessKind, TableAccessRef,
};
use super::{node_at_offset, read_string_literal, stable_hash, SynthNode};

#[derive(Debug, Clone)]
pub(super) struct PyMongoCollection {
    pub(super) table_id: String,
    pub(super) collection_name: String,
    pub(super) owner_id: String,
    pub(super) owner_name: String,
    pub(super) file_path: String,
}

pub(super) fn detect_pymongo_collections(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
) -> Vec<PyMongoCollection> {
    let mut names = pymongo_collection_names(text);
    names.sort();
    names.dedup();
    let Some(owner) = nodes
        .iter()
        .copied()
        .find(|node| matches!(node.label.as_str(), "Function" | "Method" | "Class"))
    else {
        return Vec::new();
    };
    names
        .into_iter()
        .map(|name| PyMongoCollection {
            table_id: pymongo_collection_id(file_path, &name),
            collection_name: name,
            owner_id: owner.aka_id.clone(),
            owner_name: owner.display_name().to_string(),
            file_path: file_path.to_string(),
        })
        .collect()
}

pub(super) fn detect_pymongo_table_accesses(
    text: &str,
    nodes: &[&SynthNode],
    table_lookup: &BTreeMap<String, TableAccessRef>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    for access in pymongo_accesses(text) {
        let Some(table) = table_lookup
            .get(&normalize_table_access_key(&access.collection_name))
            .cloned()
        else {
            continue;
        };
        let Some(node) = node_at_offset(text, nodes, access.offset) else {
            continue;
        };
        out.push(TableAccessDetection {
            table,
            node_id: node.aka_id.clone(),
            kind: access.kind,
            strategy: access.strategy.into(),
        });
    }
    out.sort_by(|a, b| {
        a.node_id
            .cmp(&b.node_id)
            .then_with(|| a.table.table_id.cmp(&b.table.table_id))
            .then_with(|| a.kind.edge_type().cmp(b.kind.edge_type()))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| {
        a.node_id == b.node_id
            && a.table.table_id == b.table.table_id
            && a.kind == b.kind
            && a.strategy == b.strategy
    });
    out
}

fn pymongo_collection_names(text: &str) -> Vec<String> {
    pymongo_accesses(text)
        .into_iter()
        .map(|access| access.collection_name)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Debug, Clone)]
struct PyMongoAccess {
    offset: usize,
    collection_name: String,
    kind: TableAccessKind,
    strategy: &'static str,
}

fn pymongo_accesses(text: &str) -> Vec<PyMongoAccess> {
    let mut out = Vec::new();
    out.extend(pymongo_index_accesses(text));
    out.extend(pymongo_attr_accesses(text));
    out.extend(pymongo_get_collection_accesses(text));
    out
}

fn pymongo_index_accesses(text: &str) -> Vec<PyMongoAccess> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0usize;
    while idx < text.len() {
        if bytes.get(idx) != Some(&b'[') {
            idx += 1;
            continue;
        }
        let Some(receiver_start) = receiver_start_before(text, idx) else {
            idx += 1;
            continue;
        };
        if !is_pymongo_db_receiver(&text[receiver_start..idx]) {
            idx += 1;
            continue;
        }
        let literal_start = skip_ws(text, idx + 1);
        if !matches!(bytes.get(literal_start), Some(b'"' | b'\'')) {
            idx += 1;
            continue;
        }
        let Some((collection_name, literal_end)) = read_string_literal(text, literal_start) else {
            idx += 1;
            continue;
        };
        let close = skip_ws(text, literal_end);
        if bytes.get(close) != Some(&b']') {
            idx += 1;
            continue;
        }
        if let Some((kind, strategy)) = method_access_after_collection(text, close + 1) {
            out.push(PyMongoAccess {
                offset: receiver_start,
                collection_name,
                kind,
                strategy,
            });
        }
        idx = close + 1;
    }
    out
}

fn pymongo_attr_accesses(text: &str) -> Vec<PyMongoAccess> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some(dot_rel) = text[idx..].find('.') else {
            break;
        };
        let dot = idx + dot_rel;
        let Some(receiver_start) = receiver_start_before(text, dot) else {
            idx = dot + 1;
            continue;
        };
        if !is_pymongo_db_receiver(&text[receiver_start..dot]) {
            idx = dot + 1;
            continue;
        }
        let Some((collection, collection_end)) = read_python_identifier(text, dot + 1) else {
            idx = dot + 1;
            continue;
        };
        if !is_collection_name(collection) {
            idx = collection_end;
            continue;
        }
        if let Some((kind, strategy)) = method_access_after_collection(text, collection_end) {
            out.push(PyMongoAccess {
                offset: receiver_start,
                collection_name: collection.to_string(),
                kind,
                strategy,
            });
        }
        idx = collection_end;
    }
    out
}

fn pymongo_get_collection_accesses(text: &str) -> Vec<PyMongoAccess> {
    let mut out = Vec::new();
    let marker = ".get_collection";
    let mut idx = 0usize;
    while let Some(rel) = text[idx..].find(marker) {
        let start = idx + rel;
        let Some(receiver_start) = receiver_start_before(text, start) else {
            idx = start + marker.len();
            continue;
        };
        if !is_pymongo_db_receiver(&text[receiver_start..start]) {
            idx = start + marker.len();
            continue;
        }
        let open = skip_ws(text, start + marker.len());
        if text.as_bytes().get(open) != Some(&b'(') {
            idx = start + marker.len();
            continue;
        }
        let literal_start = skip_ws(text, open + 1);
        if !matches!(text.as_bytes().get(literal_start), Some(b'"' | b'\'')) {
            idx = open + 1;
            continue;
        }
        let Some((collection_name, literal_end)) = read_string_literal(text, literal_start) else {
            idx = open + 1;
            continue;
        };
        let close = skip_ws(text, literal_end);
        if text.as_bytes().get(close) != Some(&b')') {
            idx = literal_end;
            continue;
        }
        if let Some((kind, strategy)) = method_access_after_collection(text, close + 1) {
            out.push(PyMongoAccess {
                offset: receiver_start,
                collection_name,
                kind,
                strategy,
            });
        }
        idx = close + 1;
    }
    out
}

fn method_access_after_collection(
    text: &str,
    start: usize,
) -> Option<(TableAccessKind, &'static str)> {
    let dot = skip_ws(text, start);
    if text.as_bytes().get(dot) != Some(&b'.') {
        return None;
    }
    let (method, method_end) = read_python_identifier(text, dot + 1)?;
    let open = skip_ws(text, method_end);
    if text.as_bytes().get(open) != Some(&b'(') {
        return None;
    }
    pymongo_method_access(method)
}

fn pymongo_method_access(method: &str) -> Option<(TableAccessKind, &'static str)> {
    if matches!(
        method,
        "find"
            | "find_one"
            | "aggregate"
            | "count_documents"
            | "estimated_document_count"
            | "distinct"
            | "watch"
    ) {
        return Some((TableAccessKind::Read, "python-pymongo-read"));
    }
    if matches!(
        method,
        "insert_one"
            | "insert_many"
            | "update_one"
            | "update_many"
            | "replace_one"
            | "delete_one"
            | "delete_many"
            | "bulk_write"
            | "find_one_and_update"
            | "find_one_and_replace"
            | "find_one_and_delete"
    ) {
        return Some((TableAccessKind::Write, "python-pymongo-write"));
    }
    None
}

fn is_pymongo_db_receiver(receiver: &str) -> bool {
    let tail = receiver.rsplit('.').next().unwrap_or(receiver).trim();
    matches!(tail, "db" | "database" | "mongo_db" | "mongodb")
        || tail.ends_with("_db")
        || tail.ends_with("_database")
}

fn receiver_start_before(text: &str, end: usize) -> Option<usize> {
    let mut idx = end;
    while idx > 0 {
        let ch = text[..idx].chars().next_back()?;
        if is_python_ident_continue(ch) || ch == '.' {
            idx -= ch.len_utf8();
        } else {
            break;
        }
    }
    (idx < end).then_some(idx)
}

fn read_python_identifier(text: &str, start: usize) -> Option<(&str, usize)> {
    let mut chars = text[start..].char_indices();
    let (_, first) = chars.next()?;
    if !is_python_ident_start(first) {
        return None;
    }
    let mut end = start + first.len_utf8();
    for (rel, ch) in chars {
        if !is_python_ident_continue(ch) {
            break;
        }
        end = start + rel + ch.len_utf8();
    }
    Some((&text[start..end], end))
}

fn is_collection_name(name: &str) -> bool {
    !matches!(
        name,
        "client" | "database" | "get_collection" | "with_options"
    )
}

fn is_python_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_python_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn skip_ws(text: &str, mut idx: usize) -> usize {
    while idx < text.len() {
        let Some(ch) = text[idx..].chars().next() else {
            break;
        };
        if ch.is_whitespace() {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }
    idx
}

fn pymongo_collection_id(file_path: &str, collection_name: &str) -> String {
    format!(
        "table:pymongo:{:016x}",
        stable_hash(&format!("{file_path}|{collection_name}"))
    )
}
