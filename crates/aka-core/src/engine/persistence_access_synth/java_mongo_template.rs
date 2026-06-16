use std::collections::{BTreeMap, BTreeSet};

use super::{
    java_var_reference_boundary_ok, java_variables_of_type, node_text_window,
    normalize_table_access_key, read_java_identifier, TableAccessDetection, TableAccessEntity,
    TableAccessKind, TableAccessRef,
};
use crate::engine::{read_string_literal, split_top_level_commas, SynthNode};

pub(super) fn detect_table_accesses(
    text: &str,
    nodes: &[&SynthNode],
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    let file_vars = mongo_template_variables(text);
    let class_vars = class_mongo_template_variables(text, nodes);
    for node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        let Some(body) = node_text_window(text, node) else {
            continue;
        };
        let mut vars = file_vars.clone();
        for file_vars in class_vars.values() {
            vars.extend(file_vars.clone());
        }
        if let Some(parent_vars) = class_vars.get(node.parent_class.as_deref().unwrap_or_default())
        {
            vars.extend(parent_vars.clone());
        }
        vars.extend(mongo_template_variables(body));
        for var in vars {
            for access in mongo_template_accesses(body, &var, entities) {
                out.push(TableAccessDetection {
                    table: access.table,
                    node_id: node.aka_id.clone(),
                    kind: access.kind,
                    strategy: access.strategy.into(),
                });
            }
        }
    }
    out
}

fn class_mongo_template_variables(
    text: &str,
    nodes: &[&SynthNode],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out = BTreeMap::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        let Some(body) = node_text_window(text, node) else {
            continue;
        };
        let vars = mongo_template_variables(body);
        if !vars.is_empty() {
            out.insert(node.qn.clone(), vars.clone());
            out.insert(node.name.clone(), vars);
        }
    }
    out
}

fn mongo_template_variables(body: &str) -> BTreeSet<String> {
    let mut vars = BTreeSet::new();
    for type_name in ["MongoTemplate", "ReactiveMongoTemplate"] {
        vars.extend(java_variables_of_type(body, type_name));
    }
    vars
}

#[derive(Debug, Clone)]
struct MongoTemplateAccess {
    table: TableAccessRef,
    kind: TableAccessKind,
    strategy: &'static str,
}

fn mongo_template_accesses(
    body: &str,
    var: &str,
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Vec<MongoTemplateAccess> {
    let mut out = Vec::new();
    let marker = format!("{var}.");
    let mut offset = 0usize;
    while let Some(pos) = body[offset..].find(&marker) {
        let start = offset + pos;
        if !java_var_reference_boundary_ok(body, start) {
            offset = start + marker.len();
            continue;
        }
        let method_start = start + marker.len();
        let Some((method, method_end)) = read_java_identifier(body, method_start) else {
            offset = start + marker.len();
            continue;
        };
        let rest = &body[method_end..];
        let open_rel = rest.find('(').filter(|idx| rest[..*idx].trim().is_empty());
        let Some(open) = open_rel.map(|idx| method_end + idx) else {
            offset = start + marker.len();
            continue;
        };
        let Some(close) = find_matching_paren(body, open) else {
            offset = start + marker.len();
            continue;
        };
        if let Some((kind, strategy)) = mongo_template_method_access(method) {
            let args = &body[open + 1..close];
            if let Some(table) = mongo_template_table_arg(args, entities) {
                out.push(MongoTemplateAccess {
                    table,
                    kind,
                    strategy,
                });
            }
        }
        offset = close + 1;
    }
    out.sort_by(|a, b| {
        a.kind
            .edge_type()
            .cmp(b.kind.edge_type())
            .then_with(|| a.table.table_id.cmp(&b.table.table_id))
            .then_with(|| a.strategy.cmp(b.strategy))
    });
    out.dedup_by(|a, b| {
        a.kind == b.kind && a.table.table_id == b.table.table_id && a.strategy == b.strategy
    });
    out
}

fn mongo_template_method_access(method: &str) -> Option<(TableAccessKind, &'static str)> {
    if matches!(
        method,
        "find"
            | "findAll"
            | "findAndModify"
            | "findAndRemove"
            | "findById"
            | "findOne"
            | "exists"
            | "count"
            | "aggregate"
            | "stream"
    ) {
        return Some((TableAccessKind::Read, "java-mongo-template-read"));
    }
    if matches!(
        method,
        "save"
            | "insert"
            | "insertAll"
            | "insertList"
            | "remove"
            | "updateFirst"
            | "updateMulti"
            | "upsert"
            | "findAndReplace"
    ) {
        return Some((TableAccessKind::Write, "java-mongo-template-write"));
    }
    None
}

fn mongo_template_table_arg(
    args: &str,
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Option<TableAccessRef> {
    let parts = split_top_level_commas(args);
    for part in parts.iter().rev() {
        if let Some(collection) = arg_string_literal(part) {
            let key = normalize_table_access_key(&collection);
            if let Some(entity) = entities
                .values()
                .find(|entity| normalize_table_access_key(&entity.table_name) == key)
            {
                return Some(TableAccessRef {
                    table_id: entity.table_id.clone(),
                    table_name: entity.table_name.clone(),
                });
            }
        }
    }
    for part in parts {
        if let Some(entity) = class_arg_entity(part, entities) {
            return Some(TableAccessRef {
                table_id: entity.table_id,
                table_name: entity.table_name,
            });
        }
    }
    None
}

fn arg_string_literal(arg: &str) -> Option<String> {
    let idx = arg.find(['"', '\''])?;
    read_string_literal(arg, idx).map(|(literal, _)| literal)
}

fn class_arg_entity(
    arg: &str,
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Option<TableAccessEntity> {
    let class_pos = arg.find(".class")?;
    let before = arg[..class_pos]
        .trim()
        .trim_matches(['(', ')'])
        .rsplit(|ch: char| !(ch == '_' || ch == '.' || ch.is_ascii_alphanumeric()))
        .next()?
        .trim()
        .rsplit('.')
        .next()?
        .trim();
    entities.get(before).cloned()
}

fn find_matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut idx = open;
    while idx < text.len() {
        let ch = text[idx..].chars().next()?;
        match ch {
            '"' | '\'' => {
                let (_, end) = read_string_literal(text, idx)?;
                idx = end;
                continue;
            }
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
        idx += ch.len_utf8();
    }
    None
}
