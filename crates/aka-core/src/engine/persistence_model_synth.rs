use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::json;

use super::persistence_java_synth::{
    java_column_names as java_persistence_column_names,
    java_mongo_field_names as java_persistence_mongo_field_names, java_relationship_targets,
};
use super::persistence_synth::{EntityInfo, SynthRepository, SynthTable};
use super::{
    find_matching_paren, source_annotations_before_node, split_top_level_commas, stable_hash,
    EdgeRec, SynthNode,
};

pub(super) fn detect_entity_table(text: &str, node: &SynthNode) -> Option<SynthTable> {
    let decorators = decorators_for_node(text, node);
    if is_jvm_node(node) && has_decorator_named(&decorators, "Entity") {
        let table_name = decorators
            .iter()
            .find(|decorator| decorator_name(decorator) == Some("Table"))
            .and_then(|decorator| annotation_arg_string(decorator, &["name", "value"]))
            .unwrap_or_else(|| camel_to_snake(&node.name));
        return Some(table_for_node(
            node,
            table_name,
            "java-jpa-entity",
            java_column_names(text, node),
        ));
    }
    if is_jvm_node(node) && has_decorator_named(&decorators, "Document") {
        let collection_name = decorators
            .iter()
            .find(|decorator| decorator_name(decorator) == Some("Document"))
            .and_then(|decorator| {
                annotation_arg_string(decorator, &["collection", "value", "name"])
            })
            .unwrap_or_else(|| camel_to_snake(&node.name));
        return Some(table_for_node(
            node,
            collection_name,
            "java-spring-data-mongo-document",
            java_mongo_field_names(text, node),
        ));
    }
    if is_python_node(node) {
        if let Some(table_name) = python_class_table_name(text, node) {
            return Some(table_for_node(
                node,
                table_name,
                "python-sqlalchemy-model",
                python_column_names(text, node),
            ));
        }
    }
    None
}

pub(super) fn detect_repository(
    text: &str,
    node: &SynthNode,
    entities: &BTreeMap<String, EntityInfo>,
) -> Option<SynthRepository> {
    if is_jvm_node(node) {
        let entity_name = java_repository_entity(text, &node.name)?;
        return Some(repository_for_entity(
            node,
            &entity_name,
            entities.get(&entity_name),
            "java-spring-data-repository",
        ));
    }
    if is_python_node(node) {
        let entity_name = python_repository_entity(text, node)?;
        return Some(repository_for_entity(
            node,
            &entity_name,
            entities.get(&entity_name),
            "python-repository-heuristic",
        ));
    }
    None
}

pub(super) fn detect_python_relationships(
    text: &str,
    nodes: &[&SynthNode],
    entities: &BTreeMap<String, EntityInfo>,
) -> Vec<EdgeRec> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Class"))
    {
        let Some(source) = entities.get(&node.name).or_else(|| entities.get(&node.qn)) else {
            continue;
        };
        let Some(class_body) = class_body_text(text, node) else {
            continue;
        };
        for target_name in python_relationship_targets(class_body) {
            let Some(target) = entities.get(&target_name) else {
                continue;
            };
            if source.entity_id == target.entity_id {
                continue;
            }
            out.push(relation_edge(
                source,
                target,
                "python-sqlalchemy-relationship",
            ));
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

pub(super) fn detect_java_relationships(
    text: &str,
    nodes: &[&SynthNode],
    entities: &BTreeMap<String, EntityInfo>,
) -> Vec<EdgeRec> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .copied()
        .filter(|node| matches!(node.label.as_str(), "Class") && is_jvm_node(node))
    {
        let Some(source) = entities.get(&node.name).or_else(|| entities.get(&node.qn)) else {
            continue;
        };
        let Some(class_body) = class_body_text(text, node) else {
            continue;
        };
        for target_name in java_relationship_targets(class_body) {
            let Some(target) = entities
                .get(&target_name)
                .or_else(|| entities.get(strip_package(&target_name)))
            else {
                continue;
            };
            if source.entity_id == target.entity_id {
                continue;
            }
            out.push(relation_edge(source, target, "java-jpa-relationship"));
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

pub(super) fn strip_package(qn: &str) -> &str {
    qn.rsplit('.').next().unwrap_or(qn)
}

fn table_for_node(
    node: &SynthNode,
    table_name: String,
    source: &str,
    columns: Vec<String>,
) -> SynthTable {
    let table_name = table_name.trim().to_string();
    let id = format!(
        "table:heuristic:{:016x}",
        stable_hash(&format!("{}|{}|{}", source, node.aka_id, table_name))
    );
    SynthTable {
        id,
        name: table_name,
        source: source.into(),
        entity_id: node.aka_id.clone(),
        entity_name: node.name.clone(),
        file_path: node.file_path.clone(),
        columns,
    }
}

fn repository_for_entity(
    node: &SynthNode,
    entity_name: &str,
    entity: Option<&EntityInfo>,
    strategy: &str,
) -> SynthRepository {
    SynthRepository {
        id: format!(
            "repository:heuristic:{:016x}",
            stable_hash(&format!("{}|{}|{}", strategy, node.aka_id, entity_name))
        ),
        name: node.display_name().to_string(),
        repo_id: node.aka_id.clone(),
        entity_id: entity.map(|e| e.entity_id.clone()),
        entity_name: entity
            .map(|e| e.entity_name.clone())
            .unwrap_or_else(|| entity_name.to_string()),
        table_id: entity.map(|e| e.table_id.clone()),
        file_path: node.file_path.clone(),
        strategy: strategy.into(),
    }
}

fn relation_edge(source: &EntityInfo, target: &EntityInfo, strategy: &str) -> EdgeRec {
    EdgeRec {
        id: format!(
            "relation:heuristic:{:016x}",
            stable_hash(&format!(
                "{}|{}|{}",
                strategy, source.entity_id, target.entity_id
            ))
        ),
        source_id: source.entity_id.clone(),
        target_id: target.entity_id.clone(),
        edge_type: "HAS_RELATION".into(),
        confidence: 0.66,
        reason: "aka persistence relationship synthesis".into(),
        step: None,
        evidence: Some(json!({
            "source": "aka-cbm-synth",
            "kind": "entity-relation",
            "strategy": strategy,
            "fromTable": source.table_name,
            "toTable": target.table_name,
        })),
    }
}

fn decorators_for_node(text: &str, node: &SynthNode) -> Vec<String> {
    let mut decorators = node.decorators.clone();
    decorators.extend(source_annotations_before_node(text, node));
    decorators.sort();
    decorators.dedup();
    decorators
}

fn java_repository_entity(text: &str, name: &str) -> Option<String> {
    for marker in [
        "JpaRepository<",
        "CrudRepository<",
        "PagingAndSortingRepository<",
        "MongoRepository<",
        "ReactiveCrudRepository<",
    ] {
        let Some(pos) = text.find(marker) else {
            continue;
        };
        if !text[..pos].contains(name) {
            continue;
        }
        let args = &text[pos + marker.len()..];
        let entity = args.split([',', '>']).next()?.trim();
        let entity = entity.rsplit('.').next().unwrap_or(entity).trim();
        if is_type_name(entity) {
            return Some(entity.to_string());
        }
    }
    None
}

fn python_repository_entity(text: &str, node: &SynthNode) -> Option<String> {
    let body = class_body_text(text, node)?;
    for line in body.lines() {
        let trimmed = line.trim();
        for key in ["model", "entity", "entity_cls", "model_class"] {
            let Some(rest) = trimmed.strip_prefix(key) else {
                continue;
            };
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('=') else {
                continue;
            };
            let candidate = rest
                .trim()
                .split(['(', ',', '#'])
                .next()
                .unwrap_or("")
                .trim();
            if is_type_name(candidate) {
                return Some(candidate.to_string());
            }
        }
    }
    let lower = node.name.to_ascii_lowercase();
    lower
        .strip_suffix("repository")
        .or_else(|| lower.strip_suffix("repo"))
        .and_then(|base| {
            (!base.is_empty()).then(|| {
                let mut chars = base.chars();
                chars
                    .next()
                    .map(|ch| ch.to_ascii_uppercase().to_string() + chars.as_str())
                    .unwrap_or_default()
            })
        })
}

fn python_class_table_name(text: &str, node: &SynthNode) -> Option<String> {
    let body = class_body_text(text, node)?;
    python_sqlalchemy_table_name(body)
        .or_else(|| python_django_meta_table_name(body))
        .or_else(|| python_django_default_table_name(text, node))
}

fn python_sqlalchemy_table_name(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("__tablename__")?.trim_start();
        let rest = rest.strip_prefix('=')?.trim();
        first_raw_string_literal(rest)
    })
}

fn python_django_meta_table_name(body: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().collect();
    let mut idx = 0usize;
    while idx < lines.len() {
        let line = lines[idx];
        let trimmed = line.trim();
        if !(trimmed == "class Meta:" || trimmed.starts_with("class Meta(")) {
            idx += 1;
            continue;
        }
        let meta_indent = leading_space_count(line);
        idx += 1;
        while idx < lines.len() {
            let nested = lines[idx];
            let nested_trimmed = nested.trim();
            if nested_trimmed.is_empty() || nested_trimmed.starts_with('#') {
                idx += 1;
                continue;
            }
            if leading_space_count(nested) <= meta_indent {
                break;
            }
            if let Some(rest) = nested_trimmed.strip_prefix("db_table") {
                let rest = rest.trim_start();
                let rest = rest.strip_prefix('=')?.trim();
                return first_raw_string_literal(rest);
            }
            idx += 1;
        }
    }
    None
}

fn python_django_default_table_name(text: &str, node: &SynthNode) -> Option<String> {
    let declaration = python_class_declaration(text, node)?;
    if !python_extends_django_model(declaration) {
        return None;
    }
    let app_label = python_django_app_label(&node.file_path)?;
    Some(format!("{}_{}", app_label, camel_to_snake(&node.name)))
}

fn python_class_declaration<'a>(text: &'a str, node: &SynthNode) -> Option<&'a str> {
    let line = text
        .lines()
        .nth(node.start_line_key().max(1) as usize - 1)?
        .trim();
    if line.starts_with("class ") {
        return Some(line);
    }
    class_body_text(text, node)?
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("class "))
}

fn python_extends_django_model(declaration: &str) -> bool {
    let Some(open) = declaration.find('(') else {
        return false;
    };
    let close = find_matching_paren(declaration, open).unwrap_or(declaration.len());
    let bases = &declaration[open + 1..close];
    split_top_level_commas(bases).into_iter().any(|base| {
        let base = base.trim();
        matches!(base, "Model" | "models.Model" | "django.db.models.Model")
    })
}

fn python_django_app_label(file_path: &str) -> Option<String> {
    let normalized = file_path.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let models_idx = parts.iter().position(|part| *part == "models.py");
    if let Some(idx) = models_idx {
        return idx
            .checked_sub(1)
            .and_then(|app_idx| django_app_label_part(parts.get(app_idx).copied()));
    }
    let models_dir_idx = parts.iter().position(|part| *part == "models")?;
    models_dir_idx
        .checked_sub(1)
        .and_then(|app_idx| django_app_label_part(parts.get(app_idx).copied()))
}

fn django_app_label_part(part: Option<&str>) -> Option<String> {
    let label = part?
        .trim()
        .trim_end_matches(".py")
        .trim_matches(['.', '-', '_']);
    if label.is_empty() || matches!(label, "src" | "app" | "apps" | "project") {
        return None;
    }
    let label = label.replace('-', "_");
    label
        .chars()
        .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        .then_some(label)
}

fn leading_space_count(line: &str) -> usize {
    line.chars().take_while(|ch| *ch == ' ').count()
}

fn python_column_names(text: &str, node: &SynthNode) -> Vec<String> {
    let mut columns = BTreeSet::new();
    let Some(body) = class_body_text(text, node) else {
        return Vec::new();
    };
    for line in body.lines() {
        let trimmed = line.trim();
        if !(trimmed.contains("Column(") || trimmed.contains("mapped_column(")) {
            continue;
        }
        if let Some((name, _)) = trimmed.split_once('=') {
            let name = name.trim();
            if is_ident(name) {
                columns.insert(name.to_string());
            }
        }
    }
    for column in python_django_column_names(body) {
        columns.insert(column);
    }
    columns.into_iter().collect()
}

fn python_relationship_targets(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for marker in ["relationship(", "Relationship("] {
        let mut offset = 0usize;
        while let Some(pos) = text[offset..].find(marker) {
            let start = offset + pos + marker.len() - 1;
            let Some(close) = find_matching_paren(text, start) else {
                offset = start + 1;
                continue;
            };
            let args = &text[start + 1..close];
            if let Some(target) = split_top_level_commas(args)
                .first()
                .and_then(|arg| first_raw_string_literal(arg).or_else(|| first_type_token(arg)))
            {
                out.insert(target);
            }
            offset = close + 1;
        }
    }
    for target in python_django_relationship_targets(text) {
        out.insert(target);
    }
    out.into_iter().collect()
}

fn python_django_column_names(body: &str) -> Vec<String> {
    let mut columns = BTreeSet::new();
    for line in body.lines() {
        let trimmed = line.trim();
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if !is_ident(name) {
            continue;
        }
        let Some(field_kind) = python_django_field_kind(value.trim()) else {
            continue;
        };
        if python_django_foreign_key_field(field_kind) {
            columns.insert(format!("{name}_id"));
        } else {
            columns.insert(name.to_string());
        }
    }
    columns.into_iter().collect()
}

fn python_django_relationship_targets(body: &str) -> Vec<String> {
    let mut targets = BTreeSet::new();
    for line in body.lines() {
        let trimmed = line.trim();
        let Some((_, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let Some(field_kind) = python_django_field_kind(value) else {
            continue;
        };
        if !python_django_relationship_field(field_kind) {
            continue;
        }
        let Some(open) = value.find('(') else {
            continue;
        };
        let close = find_matching_paren(value, open).unwrap_or(value.len());
        let args = &value[open + 1..close];
        if let Some(target) = split_top_level_commas(args)
            .first()
            .and_then(|arg| first_raw_string_literal(arg).or_else(|| first_type_token(arg)))
        {
            targets.insert(target);
        }
    }
    targets.into_iter().collect()
}

fn python_django_field_kind(value: &str) -> Option<&str> {
    let before_paren = value.split_once('(')?.0.trim();
    let name = before_paren
        .rsplit('.')
        .next()
        .unwrap_or(before_paren)
        .trim();
    (name.ends_with("Field") || python_django_relationship_field(name)).then_some(name)
}

fn python_django_relationship_field(field_kind: &str) -> bool {
    matches!(
        field_kind,
        "ForeignKey" | "OneToOneField" | "ManyToManyField"
    )
}

fn python_django_foreign_key_field(field_kind: &str) -> bool {
    matches!(field_kind, "ForeignKey" | "OneToOneField")
}

fn java_column_names(text: &str, node: &SynthNode) -> Vec<String> {
    let Some(body) = class_body_text(text, node) else {
        return Vec::new();
    };
    java_persistence_column_names(body)
}

fn java_mongo_field_names(text: &str, node: &SynthNode) -> Vec<String> {
    let Some(body) = class_body_text(text, node) else {
        return Vec::new();
    };
    java_persistence_mongo_field_names(body)
}

fn class_body_text<'a>(text: &'a str, node: &SynthNode) -> Option<&'a str> {
    let start = line_offset(text, node.start_line_key().max(1) as usize)?;
    let end = line_offset(
        text,
        node.end_line_key().max(node.start_line_key()).max(1) as usize + 1,
    )
    .unwrap_or(text.len());
    text.get(start..end)
}

fn line_offset(text: &str, line_1based: usize) -> Option<usize> {
    if line_1based <= 1 {
        return Some(0);
    }
    let mut line = 1usize;
    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            line += 1;
            if line == line_1based {
                return Some(idx + 1);
            }
        }
    }
    None
}

fn has_decorator_named(decorators: &[String], expected: &str) -> bool {
    decorators
        .iter()
        .any(|decorator| decorator_name(decorator) == Some(expected))
}

fn decorator_name(decorator: &str) -> Option<&str> {
    let text = decorator.trim().trim_start_matches('@');
    let name = text.split_once('(').map(|(name, _)| name).unwrap_or(text);
    Some(name.rsplit('.').next().unwrap_or(name).trim()).filter(|name| !name.is_empty())
}

fn annotation_arg_string(annotation: &str, keys: &[&str]) -> Option<String> {
    let open = annotation.find('(')?;
    let close = find_matching_paren(annotation, open).unwrap_or(annotation.len());
    let args = &annotation[open + 1..close];
    for part in split_top_level_commas(args) {
        let value = if let Some((key, value)) = part.split_once('=') {
            if !keys.iter().any(|expected| key.trim().ends_with(expected)) {
                continue;
            }
            value.trim()
        } else if keys.contains(&"value") {
            part.trim()
        } else {
            continue;
        };
        if let Some(literal) = first_raw_string_literal(value) {
            return Some(literal);
        }
    }
    None
}

fn first_raw_string_literal(text: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < text.len() {
        let byte = text.as_bytes().get(idx).copied()?;
        if matches!(byte, b'\'' | b'"' | b'`') {
            return read_raw_string_literal(text, idx).map(|(literal, _)| literal);
        }
        idx += 1;
    }
    None
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

fn first_type_token(text: &str) -> Option<String> {
    let token = text.trim().split(['(', ',', '#']).next()?.trim();
    is_type_name(token).then(|| token.to_string())
}

fn is_jvm_node(node: &SynthNode) -> bool {
    matches!(
        node.language.to_ascii_lowercase().as_str(),
        "java" | "kotlin" | "scala" | "groovy"
    ) || matches!(
        Path::new(&node.file_path.to_ascii_lowercase())
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("java" | "kt" | "kts" | "scala" | "groovy")
    )
}

fn is_python_node(node: &SynthNode) -> bool {
    node.language.eq_ignore_ascii_case("python")
        || node.file_path.to_ascii_lowercase().ends_with(".py")
}

fn is_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_uppercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn camel_to_snake(name: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if idx > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
