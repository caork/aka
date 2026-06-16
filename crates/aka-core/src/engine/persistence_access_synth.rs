use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::{
    find_call_args, node_at_offset, read_string_literal, source_annotations_before_node,
    split_top_level_commas, stable_hash, EdgeRec, SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct TableAccessEntity {
    pub(super) entity_id: String,
    pub(super) entity_name: String,
    pub(super) table_id: String,
    pub(super) table_name: String,
}

#[derive(Debug, Clone)]
pub(super) struct TableAccessRepository {
    pub(super) repo_name: String,
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
    repositories: &BTreeMap<String, TableAccessRepository>,
) -> Vec<EdgeRec> {
    let mut out = Vec::new();
    for detection in detect_sql_literal_table_accesses(text, nodes, table_lookup)
        .into_iter()
        .chain(detect_annotation_table_accesses(text, nodes, table_lookup))
        .chain(detect_orm_table_accesses(
            text,
            nodes,
            entities,
            repositories,
        ))
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

pub(super) fn table_access_edges_for_sql(
    node_id: &str,
    sql: &str,
    table_lookup: &BTreeMap<String, TableAccessRef>,
    strategy: &str,
) -> Vec<EdgeRec> {
    sql_table_accesses(sql, table_lookup)
        .into_iter()
        .map(|(kind, table, _)| {
            table_access_edge(TableAccessDetection {
                table,
                node_id: node_id.to_string(),
                kind,
                strategy: strategy.to_string(),
            })
        })
        .collect()
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
    text: &str,
    nodes: &[&SynthNode],
    table_lookup: &BTreeMap<String, TableAccessRef>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    for node in nodes.iter().copied() {
        if !matches!(node.label.as_str(), "Function" | "Method") {
            continue;
        }
        for decorator in decorators_for_node(text, node) {
            let Some(name) = decorator_name(&decorator) else {
                continue;
            };
            let Some(strategy_prefix) = java_annotation_sql_strategy_prefix(name) else {
                continue;
            };
            let Some(query) = first_raw_string_literal(&decorator) else {
                continue;
            };
            for (kind, table, _) in sql_table_accesses(&query, table_lookup) {
                out.push(TableAccessDetection {
                    table,
                    node_id: node.aka_id.clone(),
                    kind,
                    strategy: strategy_prefix.into(),
                });
            }
        }
    }
    out
}

fn decorators_for_node(text: &str, node: &SynthNode) -> Vec<String> {
    let mut decorators = node.decorators.clone();
    decorators.extend(source_annotations_before_node(text, node));
    decorators.sort();
    decorators.dedup();
    decorators
}

fn java_annotation_sql_strategy_prefix(name: &str) -> Option<&'static str> {
    match name {
        "Query" => Some("java-annotation-query"),
        "NativeQuery" => Some("java-annotation-nativequery"),
        "NamedQuery" => Some("java-annotation-namedquery"),
        "Select" => Some("java-mybatis-select"),
        "Insert" => Some("java-mybatis-insert"),
        "Update" => Some("java-mybatis-update"),
        "Delete" => Some("java-mybatis-delete"),
        _ => None,
    }
}

fn detect_orm_table_accesses(
    text: &str,
    nodes: &[&SynthNode],
    entities: &BTreeMap<String, TableAccessEntity>,
    repositories: &BTreeMap<String, TableAccessRepository>,
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
                    let (kind, strategy) = orm_manager_access_kind(text, start + marker.len());
                    out.push(TableAccessDetection {
                        table: TableAccessRef {
                            table_id: entity.table_id.clone(),
                            table_name: entity.table_name.clone(),
                        },
                        node_id: node.aka_id.clone(),
                        kind,
                        strategy: strategy.into(),
                    });
                }
                offset = start + marker.len();
            }
        }
    }
    out.extend(detect_python_instance_table_accesses(text, nodes, entities));
    out.extend(detect_python_session_table_accesses(text, nodes, entities));
    out.extend(detect_python_sqlalchemy_core_writes(text, nodes, entities));
    out.extend(detect_java_repository_table_accesses(
        text,
        nodes,
        repositories,
    ));
    out
}

fn detect_java_repository_table_accesses(
    text: &str,
    nodes: &[&SynthNode],
    repositories: &BTreeMap<String, TableAccessRepository>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    let file_vars = java_repository_variables(text, repositories);
    let class_vars = java_class_repository_variables(text, nodes, repositories);
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
        vars.extend(java_repository_variables(body, repositories));
        for (var, repo) in vars {
            for (kind, strategy) in java_repository_accesses(body, &var) {
                out.push(TableAccessDetection {
                    table: TableAccessRef {
                        table_id: repo.table_id.clone(),
                        table_name: repo.table_name.clone(),
                    },
                    node_id: node.aka_id.clone(),
                    kind,
                    strategy: strategy.into(),
                });
            }
        }
    }
    out
}

fn java_class_repository_variables(
    text: &str,
    nodes: &[&SynthNode],
    repositories: &BTreeMap<String, TableAccessRepository>,
) -> BTreeMap<String, BTreeMap<String, TableAccessRepository>> {
    let mut out = BTreeMap::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
    {
        let Some(body) = node_text_window(text, node) else {
            continue;
        };
        let vars = java_repository_variables(body, repositories);
        if !vars.is_empty() {
            out.insert(node.qn.clone(), vars.clone());
            out.insert(node.name.clone(), vars);
        }
    }
    out
}

fn java_repository_variables(
    body: &str,
    repositories: &BTreeMap<String, TableAccessRepository>,
) -> BTreeMap<String, TableAccessRepository> {
    let mut vars = BTreeMap::new();
    for repo in repositories.values() {
        for var in java_variables_of_type(body, &repo.repo_name) {
            vars.insert(var, repo.clone());
        }
    }
    vars
}

fn java_variables_of_type(body: &str, type_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = body[offset..].find(type_name) {
        let start = offset + pos;
        if !java_identifier_boundary_ok(body, start, type_name) {
            offset = start + type_name.len();
            continue;
        }
        let Some(var) = java_identifier_after_type(body, start + type_name.len()) else {
            offset = start + type_name.len();
            continue;
        };
        out.push(var.to_string());
        offset = start + type_name.len();
    }
    out.sort();
    out.dedup();
    out
}

fn java_identifier_after_type(text: &str, mut idx: usize) -> Option<&str> {
    while idx < text.len() {
        let ch = text[idx..].chars().next()?;
        if ch.is_ascii_whitespace() || matches!(ch, '<' | '>' | ',' | '?' | '&') {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }
    let start = idx;
    let mut chars = text[start..].char_indices();
    let (_, first) = chars.next()?;
    if !is_java_ident_start(first) {
        return None;
    }
    idx = start + first.len_utf8();
    for (rel, ch) in chars {
        if !is_java_ident_continue(ch) {
            break;
        }
        idx = start + rel + ch.len_utf8();
    }
    Some(&text[start..idx])
}

fn java_repository_accesses(body: &str, var: &str) -> Vec<(TableAccessKind, &'static str)> {
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
        if let Some((method, end)) = read_java_identifier(body, method_start) {
            if body[end..].trim_start().starts_with('(') {
                if java_repository_write_method(method) {
                    out.push((TableAccessKind::Write, "java-spring-data-repository-write"));
                } else if java_repository_read_method(method) {
                    out.push((TableAccessKind::Read, "java-spring-data-repository-read"));
                }
            }
        }
        offset = start + marker.len();
    }
    out.sort_by(|a, b| {
        a.0.edge_type()
            .cmp(b.0.edge_type())
            .then_with(|| a.1.cmp(b.1))
    });
    out.dedup();
    out
}

fn java_repository_read_method(method: &str) -> bool {
    method.starts_with("find")
        || method.starts_with("get")
        || method.starts_with("read")
        || method.starts_with("exists")
        || method.starts_with("count")
        || matches!(method, "query" | "search")
}

fn java_repository_write_method(method: &str) -> bool {
    method.starts_with("save")
        || method.starts_with("delete")
        || method.starts_with("remove")
        || method.starts_with("insert")
        || method.starts_with("update")
}

fn detect_python_instance_table_accesses(
    text: &str,
    nodes: &[&SynthNode],
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        let Some(body) = node_text_window(text, node) else {
            continue;
        };
        let vars = python_entity_variables(body, entities);
        for (var, entity) in vars {
            if python_instance_write_call(body, &var) {
                out.push(TableAccessDetection {
                    table: TableAccessRef {
                        table_id: entity.table_id,
                        table_name: entity.table_name,
                    },
                    node_id: node.aka_id.clone(),
                    kind: TableAccessKind::Write,
                    strategy: "python-orm-instance-write".into(),
                });
            }
        }
    }
    out
}

fn detect_python_session_table_accesses(
    text: &str,
    nodes: &[&SynthNode],
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        let Some(body) = node_text_window(text, node) else {
            continue;
        };
        let vars = python_entity_variables(body, entities);
        for callee in [".add", ".merge", ".delete"] {
            for call in find_call_args(body, callee) {
                if !python_is_session_receiver(body, call.start) {
                    continue;
                }
                let Some(entity) = python_session_write_entity(call.args, &vars, entities) else {
                    continue;
                };
                out.push(TableAccessDetection {
                    table: TableAccessRef {
                        table_id: entity.table_id,
                        table_name: entity.table_name,
                    },
                    node_id: node.aka_id.clone(),
                    kind: TableAccessKind::Write,
                    strategy: "python-sqlalchemy-session-write".into(),
                });
            }
        }
    }
    out
}

fn detect_python_sqlalchemy_core_writes(
    text: &str,
    nodes: &[&SynthNode],
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Vec<TableAccessDetection> {
    let mut out = Vec::new();
    for callee in ["insert", "update", "delete"] {
        for call in find_call_args(text, callee) {
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
                kind: TableAccessKind::Write,
                strategy: "python-sqlalchemy-core-write".into(),
            });
        }
    }
    out
}

fn python_session_write_entity(
    args: &str,
    vars: &BTreeMap<String, TableAccessEntity>,
    entities: &BTreeMap<String, TableAccessEntity>,
) -> Option<TableAccessEntity> {
    let first = split_top_level_commas(args).into_iter().next()?;
    let first = first
        .trim()
        .trim_start_matches("await ")
        .trim_start_matches('*')
        .trim();
    if let Some(entity) = first_type_token(first).and_then(|name| entities.get(&name).cloned()) {
        return Some(entity);
    }
    let var = first.split(['.', '[', '(', '#']).next()?.trim();
    vars.get(var).cloned()
}

fn python_entity_variables(
    body: &str,
    entities: &BTreeMap<String, TableAccessEntity>,
) -> BTreeMap<String, TableAccessEntity> {
    let mut vars = BTreeMap::new();
    let entity_list = unique_entities(entities);
    for line in body.lines() {
        let Some((lhs, rhs)) = line.split_once('=') else {
            continue;
        };
        if lhs.contains("==") || rhs.trim_start().starts_with('=') {
            continue;
        }
        let Some(var) = python_assignment_name(lhs) else {
            continue;
        };
        let rhs = rhs.trim_start();
        for entity in &entity_list {
            if python_rhs_constructs_entity(rhs, &entity.entity_name) {
                vars.insert(var.to_string(), entity.clone());
                break;
            }
        }
    }
    vars
}

fn python_assignment_name(lhs: &str) -> Option<&str> {
    let name = lhs.trim().strip_prefix("async ").unwrap_or(lhs.trim());
    (!name.is_empty()
        && name
            .chars()
            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric()))
    .then_some(name)
}

fn python_rhs_constructs_entity(rhs: &str, entity_name: &str) -> bool {
    let rhs = rhs
        .trim_start_matches("await ")
        .trim_start_matches("return ")
        .trim_start();
    rhs.starts_with(&format!("{entity_name}("))
        || rhs.starts_with(&format!("{entity_name}.objects."))
        || rhs.starts_with(&format!("{entity_name}.query."))
        || python_rhs_loads_entity(rhs, entity_name)
}

fn python_rhs_loads_entity(rhs: &str, entity_name: &str) -> bool {
    for callee in [".get", ".merge"] {
        for call in find_call_args(rhs, callee) {
            if split_top_level_commas(call.args)
                .first()
                .and_then(|arg| first_type_token(arg))
                .as_deref()
                == Some(entity_name)
            {
                return true;
            }
        }
    }
    false
}

fn python_instance_write_call(body: &str, var: &str) -> bool {
    [".save(", ".asave(", ".delete(", ".adelete("]
        .iter()
        .any(|method| python_contains_var_method_call(body, var, method))
}

fn python_contains_var_method_call(body: &str, var: &str, method: &str) -> bool {
    let needle = format!("{var}{method}");
    let mut offset = 0usize;
    while let Some(pos) = body[offset..].find(&needle) {
        let start = offset + pos;
        let before = start
            .checked_sub(1)
            .and_then(|idx| body.as_bytes().get(idx))
            .copied()
            .map(char::from);
        if before.is_none_or(|ch| !is_python_ident_continue(ch)) {
            return true;
        }
        offset = start + needle.len();
    }
    false
}

fn is_python_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn python_is_session_receiver(text: &str, dot_start: usize) -> bool {
    let Some(receiver) = python_receiver_before_dot(text, dot_start) else {
        return false;
    };
    let tail = receiver.rsplit('.').next().unwrap_or(receiver);
    matches!(
        tail.to_ascii_lowercase().as_str(),
        "session" | "db" | "db_session" | "async_session"
    )
}

fn python_receiver_before_dot(text: &str, dot_start: usize) -> Option<&str> {
    if text.as_bytes().get(dot_start) != Some(&b'.') {
        return None;
    }
    let mut start = dot_start;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if is_python_ident_continue(ch) || ch == '.' {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let receiver = text[start..dot_start].trim_matches('.');
    (!receiver.is_empty()).then_some(receiver)
}

fn orm_manager_access_kind(text: &str, start: usize) -> (TableAccessKind, &'static str) {
    let rest = text.get(start..).unwrap_or_default();
    let window = rest.split(['\n', ';']).next().unwrap_or_default().trim();
    let lower = window.to_ascii_lowercase();
    for marker in [
        ".create(",
        ".acreate(",
        ".bulk_create(",
        ".abulk_create(",
        ".update(",
        ".aupdate(",
        ".bulk_update(",
        ".abulk_update(",
        ".update_or_create(",
        ".aupdate_or_create(",
        ".get_or_create(",
        ".aget_or_create(",
        ".delete(",
        ".adelete(",
    ] {
        if lower.contains(marker) {
            return (TableAccessKind::Write, "orm-entity-manager-write");
        }
    }
    if lower.starts_with("create(")
        || lower.starts_with("acreate(")
        || lower.starts_with("bulk_create(")
        || lower.starts_with("abulk_create(")
        || lower.starts_with("aupdate(")
        || lower.starts_with("abulk_update(")
        || lower.starts_with("update_or_create(")
        || lower.starts_with("aupdate_or_create(")
        || lower.starts_with("get_or_create(")
        || lower.starts_with("aget_or_create(")
        || lower.starts_with("adelete(")
    {
        return (TableAccessKind::Write, "orm-entity-manager-write");
    }
    (TableAccessKind::Read, "orm-entity-manager-read")
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
            out.push((
                TableAccessKind::Read,
                table.clone(),
                "sql-select-from".into(),
            ));
        }
    }
    for name in sql_names_after(sql, " join ") {
        if let Some(table) = table_lookup.get(&normalize_table_access_key(&name)) {
            out.push((
                TableAccessKind::Read,
                table.clone(),
                "sql-select-join".into(),
            ));
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

fn java_identifier_boundary_ok(text: &str, start: usize, ident: &str) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|idx| text.as_bytes().get(idx))
        .copied()
        .map(char::from);
    let after = text
        .as_bytes()
        .get(start + ident.len())
        .copied()
        .map(char::from);
    before.is_none_or(|ch| !is_java_ident_continue(ch))
        && after.is_none_or(|ch| !is_java_ident_continue(ch))
}

fn java_var_reference_boundary_ok(text: &str, start: usize) -> bool {
    start
        .checked_sub(1)
        .and_then(|idx| text.as_bytes().get(idx))
        .copied()
        .map(char::from)
        .is_none_or(|ch| !is_java_ident_continue(ch))
}

fn is_java_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || matches!(ch, '_' | '$')
}

fn is_java_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$')
}

fn node_text_window<'a>(text: &'a str, node: &SynthNode) -> Option<&'a str> {
    let start_line = node.start_line_key().max(1);
    let end_line = node.end_line_key().max(start_line);
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
    start.map(|start| &text[start.min(text.len())..end.min(text.len())])
}
