use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::migration_synth::{detect_migrations, ExistingTable, MigrationTableRef, SynthMigration};
use super::persistence_access_synth::{
    detect_table_access_edges, normalize_table_access_key, TableAccessEntity, TableAccessRef,
    TableAccessRepository,
};
use super::persistence_mybatis_synth::detect_mybatis_xml_table_access_edges;
use super::{
    find_matching_paren, project_code_nodes_by_file, read_repo_text,
    source_annotations_before_node, split_top_level_commas, stable_hash, EdgeRec, NodeRec,
    ProjectSourceSet, SynthNode,
};

#[derive(Debug, Clone, Default)]
pub(super) struct SynthPersistenceGraph {
    tables: Vec<SynthTable>,
    repositories: Vec<SynthRepository>,
    migrations: Vec<SynthMigration>,
    edges: Vec<EdgeRec>,
}

impl SynthPersistenceGraph {
    pub(super) fn node_recs(&self) -> Vec<NodeRec> {
        self.tables
            .iter()
            .map(SynthTable::node_rec)
            .chain(self.repositories.iter().map(SynthRepository::node_rec))
            .chain(self.migrations.iter().map(SynthMigration::node_rec))
            .collect()
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        self.tables
            .iter()
            .flat_map(SynthTable::edge_recs)
            .chain(
                self.repositories
                    .iter()
                    .flat_map(SynthRepository::edge_recs),
            )
            .chain(self.migrations.iter().flat_map(SynthMigration::edge_recs))
            .chain(self.edges.iter().cloned())
            .collect()
    }
}

#[derive(Debug, Clone)]
struct SynthTable {
    id: String,
    name: String,
    source: String,
    entity_id: String,
    entity_name: String,
    file_path: String,
    columns: Vec<String>,
}

impl SynthTable {
    fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("tableName".into(), Value::String(self.name.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("tableSource".into(), Value::String(self.source.clone()));
        properties.insert("entityId".into(), Value::String(self.entity_id.clone()));
        properties.insert("entityName".into(), Value::String(self.entity_name.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        if !self.columns.is_empty() {
            properties.insert(
                "columns".into(),
                Value::Array(self.columns.iter().cloned().map(Value::String).collect()),
            );
        }
        NodeRec {
            id: self.id.clone(),
            label: "Table".into(),
            properties,
        }
    }

    fn edge_recs(&self) -> Vec<EdgeRec> {
        if self.source == "migration-script" {
            return Vec::new();
        }
        vec![EdgeRec {
            id: format!(
                "{}:maps-entity:{:016x}",
                self.id,
                stable_hash(&self.entity_id)
            ),
            source_id: self.entity_id.clone(),
            target_id: self.id.clone(),
            edge_type: "MAPS_TO_TABLE".into(),
            confidence: 0.78,
            reason: "aka persistence table synthesis".into(),
            step: None,
            evidence: Some(json!({
                "source": "aka-cbm-synth",
                "kind": "entity-table",
                "table": self.name,
                "entity": self.entity_name,
                "strategy": self.source,
            })),
        }]
    }
}

#[derive(Debug, Clone)]
struct SynthRepository {
    id: String,
    name: String,
    repo_id: String,
    entity_id: Option<String>,
    entity_name: String,
    table_id: Option<String>,
    file_path: String,
    strategy: String,
}

impl SynthRepository {
    fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert(
            "repositorySource".into(),
            Value::String(self.strategy.clone()),
        );
        properties.insert("repositoryId".into(), Value::String(self.repo_id.clone()));
        properties.insert("entityName".into(), Value::String(self.entity_name.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        if let Some(entity_id) = &self.entity_id {
            properties.insert("entityId".into(), Value::String(entity_id.clone()));
        }
        if let Some(table_id) = &self.table_id {
            properties.insert("tableId".into(), Value::String(table_id.clone()));
        }
        NodeRec {
            id: self.id.clone(),
            label: "Repository".into(),
            properties,
        }
    }

    fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = vec![EdgeRec {
            id: format!(
                "{}:repository-node:{:016x}",
                self.id,
                stable_hash(&self.repo_id)
            ),
            source_id: self.repo_id.clone(),
            target_id: self.id.clone(),
            edge_type: "DEFINES_REPOSITORY".into(),
            confidence: 0.72,
            reason: "aka persistence repository synthesis".into(),
            step: None,
            evidence: Some(json!({
                "source": "aka-cbm-synth",
                "kind": "repository-definition",
                "entity": self.entity_name,
                "strategy": self.strategy,
            })),
        }];
        if let Some(entity_id) = &self.entity_id {
            out.push(EdgeRec {
                id: format!("{}:manages-entity:{:016x}", self.id, stable_hash(entity_id)),
                source_id: self.id.clone(),
                target_id: entity_id.clone(),
                edge_type: "MANAGES_ENTITY".into(),
                confidence: 0.76,
                reason: "aka persistence repository synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "repository-entity",
                    "entity": self.entity_name,
                    "strategy": self.strategy,
                })),
            });
        }
        if let Some(table_id) = &self.table_id {
            out.push(EdgeRec {
                id: format!(
                    "{}:repository-table:{:016x}",
                    self.id,
                    stable_hash(table_id)
                ),
                source_id: self.id.clone(),
                target_id: table_id.clone(),
                edge_type: "REPOSITORY_FOR".into(),
                confidence: 0.7,
                reason: "aka persistence repository table linkage".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "repository-table",
                    "entity": self.entity_name,
                    "strategy": self.strategy,
                })),
            });
        }
        out
    }
}

#[derive(Debug, Clone)]
struct EntityInfo {
    entity_id: String,
    entity_name: String,
    table_id: String,
    table_name: String,
}

pub(super) fn synthesize_persistence_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> SynthPersistenceGraph {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut tables: BTreeMap<String, SynthTable> = BTreeMap::new();
    let mut entity_by_name: BTreeMap<String, EntityInfo> = BTreeMap::new();
    let mut repositories: BTreeMap<String, SynthRepository> = BTreeMap::new();
    let mut migrations: BTreeMap<String, SynthMigration> = BTreeMap::new();
    let mut edges: BTreeMap<String, EdgeRec> = BTreeMap::new();

    for (file_path, file_nodes) in &by_file {
        let text = read_repo_text(repo, file_path).unwrap_or_default();
        for node in file_nodes
            .iter()
            .copied()
            .filter(|node| matches!(node.label.as_str(), "Class"))
        {
            if let Some(table) = detect_entity_table(&text, node) {
                let info = EntityInfo {
                    entity_id: node.aka_id.clone(),
                    entity_name: node.name.clone(),
                    table_id: table.id.clone(),
                    table_name: table.name.clone(),
                };
                entity_by_name.insert(node.name.clone(), info.clone());
                entity_by_name.insert(strip_package(&node.qn).to_string(), info.clone());
                entity_by_name.insert(node.qn.clone(), info.clone());
                tables.entry(table.id.clone()).or_insert(table);
            }
        }
    }

    for (file_path, file_nodes) in &by_file {
        let text = read_repo_text(repo, file_path).unwrap_or_default();
        for node in file_nodes
            .iter()
            .copied()
            .filter(|node| matches!(node.label.as_str(), "Class" | "Interface"))
        {
            if let Some(repo_node) = detect_repository(&text, node, &entity_by_name) {
                repositories
                    .entry(repo_node.id.clone())
                    .or_insert(repo_node);
            }
        }
        for edge in detect_python_relationships(&text, file_nodes, &entity_by_name) {
            edges.entry(edge.id.clone()).or_insert(edge);
        }
    }

    let existing_tables = tables.values().map(|table| ExistingTable {
        id: table.id.clone(),
        name: table.name.clone(),
    });
    for migration in detect_migrations(repo, &project_sources, existing_tables) {
        for table_ref in &migration.tables {
            tables
                .entry(table_ref.table_id.clone())
                .or_insert_with(|| migration_table_placeholder(table_ref, &migration));
        }
        migrations.insert(migration.id.clone(), migration);
    }

    let table_lookup = table_access_lookup(&tables, &entity_by_name);
    let table_access_entities = table_access_entities(&entity_by_name);
    let table_access_repositories = table_access_repositories(&repositories, &tables);
    for (file_path, file_nodes) in &by_file {
        let text = read_repo_text(repo, file_path).unwrap_or_default();
        for edge in detect_table_access_edges(
            &text,
            file_nodes,
            &table_lookup,
            &table_access_entities,
            &table_access_repositories,
        ) {
            edges.entry(edge.id.clone()).or_insert(edge);
        }
    }
    for edge in detect_mybatis_xml_table_access_edges(repo, &project_sources, nodes, &table_lookup)
    {
        edges.entry(edge.id.clone()).or_insert(edge);
    }

    let mut tables: Vec<_> = tables.into_values().collect();
    tables.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.entity_name.cmp(&b.entity_name))
    });
    let mut repositories: Vec<_> = repositories.into_values().collect();
    repositories.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.entity_name.cmp(&b.entity_name))
    });
    SynthPersistenceGraph {
        tables,
        repositories,
        migrations: migrations.into_values().collect(),
        edges: edges.into_values().collect(),
    }
}

fn detect_entity_table(text: &str, node: &SynthNode) -> Option<SynthTable> {
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
            java_column_names(text),
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

fn decorators_for_node(text: &str, node: &SynthNode) -> Vec<String> {
    let mut decorators = node.decorators.clone();
    decorators.extend(source_annotations_before_node(text, node));
    decorators.sort();
    decorators.dedup();
    decorators
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

fn detect_repository(
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

fn detect_python_relationships(
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

fn table_access_lookup(
    tables: &BTreeMap<String, SynthTable>,
    entities: &BTreeMap<String, EntityInfo>,
) -> BTreeMap<String, TableAccessRef> {
    let mut lookup = BTreeMap::new();
    for table in tables.values() {
        lookup.insert(
            normalize_table_access_key(&table.name),
            TableAccessRef {
                table_id: table.id.clone(),
                table_name: table.name.clone(),
            },
        );
    }
    for entity in entities.values() {
        let table = TableAccessRef {
            table_id: entity.table_id.clone(),
            table_name: entity.table_name.clone(),
        };
        lookup.insert(
            normalize_table_access_key(&entity.entity_name),
            table.clone(),
        );
        lookup.insert(normalize_table_access_key(&entity.table_name), table);
    }
    lookup
}

fn table_access_entities(
    entities: &BTreeMap<String, EntityInfo>,
) -> BTreeMap<String, TableAccessEntity> {
    entities
        .iter()
        .map(|(name, entity)| {
            (
                name.clone(),
                TableAccessEntity {
                    entity_id: entity.entity_id.clone(),
                    entity_name: entity.entity_name.clone(),
                    table_id: entity.table_id.clone(),
                    table_name: entity.table_name.clone(),
                },
            )
        })
        .collect()
}

fn table_access_repositories(
    repositories: &BTreeMap<String, SynthRepository>,
    tables: &BTreeMap<String, SynthTable>,
) -> BTreeMap<String, TableAccessRepository> {
    let mut out = BTreeMap::new();
    for repo in repositories.values() {
        let Some(table_id) = &repo.table_id else {
            continue;
        };
        let table_name = tables
            .get(table_id)
            .map(|table| table.name.clone())
            .unwrap_or_else(|| repo.entity_name.clone());
        let value = TableAccessRepository {
            repo_name: repo.name.clone(),
            table_id: table_id.clone(),
            table_name,
        };
        out.insert(repo.name.clone(), value.clone());
        out.insert(strip_package(&repo.name).to_string(), value);
    }
    out
}

fn migration_table_placeholder(
    table_ref: &MigrationTableRef,
    migration: &SynthMigration,
) -> SynthTable {
    SynthTable {
        id: table_ref.table_id.clone(),
        name: table_ref.table_name.clone(),
        source: "migration-script".into(),
        entity_id: migration.id.clone(),
        entity_name: migration.name.clone(),
        file_path: migration.file_path.clone(),
        columns: Vec::new(),
    }
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
    python_sqlalchemy_table_name(body).or_else(|| python_django_meta_table_name(body))
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
    out.into_iter().collect()
}

fn java_column_names(text: &str) -> Vec<String> {
    let mut columns = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.contains("@Column") {
            continue;
        }
        if let Some(name) = annotation_arg_string(trimmed, &["name", "value"]) {
            columns.insert(name);
        }
    }
    columns.into_iter().collect()
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

fn strip_package(qn: &str) -> &str {
    qn.rsplit('.').next().unwrap_or(qn)
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
