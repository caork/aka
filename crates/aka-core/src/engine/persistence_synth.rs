use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};

use super::migration_synth::{detect_migrations, ExistingTable, MigrationTableRef, SynthMigration};
use super::persistence_access_synth::{
    detect_table_access_edges, normalize_table_access_key, TableAccessEntity, TableAccessRef,
    TableAccessRepository,
};
use super::persistence_model_synth::{
    detect_entity_table, detect_java_relationships, detect_python_relationships, detect_repository,
    strip_package,
};
use super::persistence_mybatis_synth::detect_mybatis_xml_table_access_edges;
use super::persistence_pymongo_synth::detect_pymongo_collections;
use super::{
    project_code_nodes_by_file, read_repo_text, stable_hash, EdgeRec, NodeRec, ProjectSourceSet,
    SynthNode,
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
pub(super) struct SynthTable {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) source: String,
    pub(super) entity_id: String,
    pub(super) entity_name: String,
    pub(super) file_path: String,
    pub(super) columns: Vec<String>,
}

impl From<super::persistence_pymongo_synth::PyMongoCollection> for SynthTable {
    fn from(collection: super::persistence_pymongo_synth::PyMongoCollection) -> Self {
        Self {
            id: collection.table_id,
            name: collection.collection_name,
            source: "python-pymongo-collection".into(),
            entity_id: collection.owner_id,
            entity_name: collection.owner_name,
            file_path: collection.file_path,
            columns: Vec::new(),
        }
    }
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
pub(super) struct SynthRepository {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) repo_id: String,
    pub(super) entity_id: Option<String>,
    pub(super) entity_name: String,
    pub(super) table_id: Option<String>,
    pub(super) file_path: String,
    pub(super) strategy: String,
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
pub(super) struct EntityInfo {
    pub(super) entity_id: String,
    pub(super) entity_name: String,
    pub(super) table_id: String,
    pub(super) table_name: String,
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
        for collection in detect_pymongo_collections(&text, file_path, file_nodes) {
            tables
                .entry(collection.table_id.clone())
                .or_insert_with(|| collection.into());
        }
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
        for edge in detect_java_relationships(&text, file_nodes, &entity_by_name) {
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
