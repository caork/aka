use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, is_noisy_source_path, read_repo_text, read_string_literal,
    split_top_level_commas, stable_hash, EdgeRec, NodeRec, ProjectSourceSet,
};

#[derive(Debug, Clone)]
pub(super) struct SynthMigration {
    pub(super) id: String,
    pub(super) name: String,
    migration_type: String,
    pub(super) file_path: String,
    version: Option<String>,
    pub(super) tables: Vec<MigrationTableRef>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct MigrationTableRef {
    pub(super) table_id: String,
    pub(super) table_name: String,
    operations: Vec<String>,
}

impl SynthMigration {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert(
            "migrationType".into(),
            Value::String(self.migration_type.clone()),
        );
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert(
            "migrationSource".into(),
            Value::String("source-scan".into()),
        );
        if let Some(version) = &self.version {
            properties.insert("version".into(), Value::String(version.clone()));
        }
        if !self.tables.is_empty() {
            properties.insert(
                "tables".into(),
                Value::Array(
                    self.tables
                        .iter()
                        .map(|table| Value::String(table.table_name.clone()))
                        .collect(),
                ),
            );
            let operations: BTreeSet<_> = self
                .tables
                .iter()
                .flat_map(|table| table.operations.iter().cloned())
                .collect();
            properties.insert(
                "operations".into(),
                Value::Array(operations.into_iter().map(Value::String).collect()),
            );
        }
        NodeRec {
            id: self.id.clone(),
            label: "Migration".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        self.tables
            .iter()
            .map(|table| EdgeRec {
                id: format!(
                    "{}:changes-table:{:016x}",
                    self.id,
                    stable_hash(&format!(
                        "{}|{}",
                        table.table_id,
                        table.operations.join(",")
                    ))
                ),
                source_id: self.id.clone(),
                target_id: table.table_id.clone(),
                edge_type: "MIGRATES_TABLE".into(),
                confidence: 0.74,
                reason: "aka migration synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "migration-table",
                    "migration": self.name,
                    "migrationType": self.migration_type,
                    "table": table.table_name,
                    "operations": table.operations,
                })),
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub(super) struct ExistingTable {
    pub(super) id: String,
    pub(super) name: String,
}

pub(super) fn detect_migrations(
    repo: &Path,
    project_sources: &ProjectSourceSet,
    existing_tables: impl IntoIterator<Item = ExistingTable>,
) -> Vec<SynthMigration> {
    let mut out = Vec::new();
    let table_lookup = table_lookup(existing_tables);
    if project_sources.has_git_listing() {
        for file_path in project_sources
            .iter()
            .filter(|path| is_migration_path(path) && project_sources.contains_project_file(repo, path))
        {
            let Some(text) = read_repo_text(repo, file_path) else {
                continue;
            };
            if let Some(migration) = migration_from_file(file_path, &text, &table_lookup) {
                out.push(migration);
            }
        }
        out.sort_by(|a, b| {
            a.file_path
                .cmp(&b.file_path)
                .then_with(|| a.name.cmp(&b.name))
        });
        return out;
    }

    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let rel = path
                    .strip_prefix(repo)
                    .ok()
                    .map(|path| path.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                if !is_noisy_migration_dir(&rel) {
                    stack.push(path);
                }
                continue;
            }
            let Ok(rel) = path.strip_prefix(repo) else {
                continue;
            };
            let file_path = rel.to_string_lossy().replace('\\', "/");
            if !is_migration_path(&file_path)
                || !project_sources.contains_project_file(repo, &file_path)
            {
                continue;
            }
            let Some(text) = std::fs::read_to_string(&path).ok() else {
                continue;
            };
            if let Some(migration) = migration_from_file(&file_path, &text, &table_lookup) {
                out.push(migration);
            }
        }
    }
    out.sort_by(|a, b| {
        a.file_path
            .cmp(&b.file_path)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn table_lookup(tables: impl IntoIterator<Item = ExistingTable>) -> BTreeMap<String, String> {
    tables
        .into_iter()
        .map(|table| (normalize_table_name(&table.name), table.id))
        .collect()
}

fn is_noisy_migration_dir(path: &str) -> bool {
    let path = path.replace('\\', "/");
    path.split('/').any(|segment| {
        matches!(
            segment,
            ".git" | ".hg" | ".svn" | "node_modules" | "target" | "build" | "dist"
        )
    }) || is_noisy_source_path(&path)
}

fn is_migration_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    let ext = Path::new(&lower).extension().and_then(|ext| ext.to_str());
    let migration_dir = lower.contains("/db/migration/")
        || lower.contains("/db/changelog/")
        || lower.contains("/migrations/")
        || lower.contains("/migration/");
    match ext {
        Some("sql") => migration_dir || flyway_file_name(&lower).is_some(),
        Some("xml" | "yaml" | "yml" | "json") => {
            migration_dir || lower.contains("changelog") || lower.contains("liquibase")
        }
        Some("py") => {
            lower.contains("/migrations/")
                || lower.starts_with("migrations/")
                || lower.contains("/alembic/versions/")
                || lower.starts_with("alembic/versions/")
        }
        _ => false,
    }
}

fn migration_from_file(
    file_path: &str,
    text: &str,
    table_lookup: &BTreeMap<String, String>,
) -> Option<SynthMigration> {
    let lower = file_path.to_ascii_lowercase();
    let ext = Path::new(&lower).extension().and_then(|ext| ext.to_str());
    let migration_type = match ext {
        Some("sql") => "sql-migration",
        Some("xml" | "yaml" | "yml" | "json") => "liquibase-migration",
        Some("py") => "python-migration",
        _ => return None,
    };
    let detections = match migration_type {
        "sql-migration" => detect_sql_migration_tables(text),
        "liquibase-migration" => detect_liquibase_tables(text),
        "python-migration" => detect_python_migration_tables(text),
        _ => Vec::new(),
    };
    if detections.is_empty() {
        return None;
    }
    let mut by_table: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for detection in detections {
        if is_plausible_table_name(&detection.table_name) {
            by_table
                .entry(detection.table_name)
                .or_default()
                .insert(detection.operation);
        }
    }
    if by_table.is_empty() {
        return None;
    }
    let tables = by_table
        .into_iter()
        .map(|(table_name, operations)| {
            let table_id = table_lookup
                .get(&normalize_table_name(&table_name))
                .cloned()
                .unwrap_or_else(|| migration_table_id(&table_name));
            MigrationTableRef {
                table_id,
                table_name,
                operations: operations.into_iter().collect(),
            }
        })
        .collect::<Vec<_>>();
    let name = migration_name(file_path);
    let version = migration_version(file_path);
    Some(SynthMigration {
        id: format!(
            "migration:heuristic:{:016x}",
            stable_hash(&format!("{migration_type}|{file_path}|{name}"))
        ),
        name,
        migration_type: migration_type.into(),
        file_path: file_path.into(),
        version,
        tables,
    })
}

#[derive(Debug, Clone)]
struct MigrationDetection {
    table_name: String,
    operation: String,
}

fn detect_sql_migration_tables(text: &str) -> Vec<MigrationDetection> {
    let mut out = Vec::new();
    for (needle, operation) in [
        ("create table", "create"),
        ("alter table", "alter"),
        ("drop table", "drop"),
        ("truncate table", "truncate"),
        ("rename table", "rename"),
    ] {
        out.extend(sql_table_names_after(text, needle, operation));
    }
    out
}

fn sql_table_names_after(text: &str, needle: &str, operation: &str) -> Vec<MigrationDetection> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = lower[offset..].find(needle) {
        let start = offset + pos + needle.len();
        let rest = &text[start..];
        if let Some(table_name) = read_sql_table_name(rest) {
            out.push(MigrationDetection {
                table_name,
                operation: operation.into(),
            });
        }
        offset = start;
    }
    out
}

fn read_sql_table_name(text: &str) -> Option<String> {
    let mut rest = text.trim_start();
    for prefix in ["if not exists", "if exists", "only"] {
        if rest.to_ascii_lowercase().starts_with(prefix) {
            rest = rest[prefix.len()..].trim_start();
        }
    }
    let mut name = String::new();
    let mut quote: Option<char> = None;
    for ch in rest.chars() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                name.push(ch);
            }
            continue;
        }
        if matches!(ch, '"' | '`' | '[') {
            quote = Some(if ch == '[' { ']' } else { ch });
            continue;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '$') {
            name.push(ch);
            continue;
        }
        break;
    }
    clean_table_name(&name)
}

fn detect_liquibase_tables(text: &str) -> Vec<MigrationDetection> {
    let mut out = Vec::new();
    for (key, operation) in [
        ("createTable", "create"),
        ("addColumn", "alter"),
        ("dropTable", "drop"),
        ("renameTable", "rename"),
        ("tableName", "alter"),
    ] {
        for value in values_after_key(text, key) {
            out.push(MigrationDetection {
                table_name: value,
                operation: operation.into(),
            });
        }
    }
    out
}

fn values_after_key(text: &str, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(pos) = text[offset..].find(key) {
        let idx = offset + pos + key.len();
        let rest = text[idx..].trim_start();
        let value = if let Some(rest) = rest.strip_prefix('=') {
            first_raw_string_literal(rest)
        } else if let Some(rest) = rest.strip_prefix(':') {
            yaml_scalar_value(rest)
        } else {
            None
        };
        if let Some(value) = value.and_then(|value| clean_table_name(&value)) {
            out.push(value);
        }
        offset = idx;
    }
    out
}

fn yaml_scalar_value(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    if trimmed.starts_with(['"', '\'']) {
        return first_raw_string_literal(trimmed);
    }
    let value = trimmed
        .lines()
        .next()
        .unwrap_or("")
        .split(['#', ',', '}'])
        .next()
        .unwrap_or("")
        .trim();
    clean_table_name(value)
}

fn detect_python_migration_tables(text: &str) -> Vec<MigrationDetection> {
    let mut out = Vec::new();
    for (needle, operation) in [
        (".create_table", "create"),
        (".drop_table", "drop"),
        (".add_column", "alter"),
        (".alter_column", "alter"),
        ("migrations.CreateModel", "create"),
        ("migrations.DeleteModel", "drop"),
        ("migrations.AddField", "alter"),
        ("migrations.AlterField", "alter"),
        ("migrations.RunSQL", "sql"),
    ] {
        for call in find_call_args(text, needle) {
            if needle == "migrations.CreateModel" || needle == "migrations.DeleteModel" {
                if let Some(name) = keyword_string_literal(call.args, "name") {
                    out.push(MigrationDetection {
                        table_name: camel_to_snake(&name),
                        operation: operation.into(),
                    });
                }
                continue;
            }
            if needle == "migrations.RunSQL" {
                if let Some(sql) = first_raw_string_literal(call.args) {
                    out.extend(detect_sql_migration_tables(&sql));
                }
                continue;
            }
            if let Some(table_name) = first_raw_string_literal(call.args) {
                out.push(MigrationDetection {
                    table_name,
                    operation: operation.into(),
                });
            }
        }
    }
    out
}

fn keyword_string_literal(args: &str, key: &str) -> Option<String> {
    for part in split_top_level_commas(args) {
        let (found, value) = part.split_once('=')?;
        if found.trim() == key {
            return first_raw_string_literal(value);
        }
    }
    None
}

pub(super) fn migration_table_id(table_name: &str) -> String {
    format!(
        "table:migration:{:016x}",
        stable_hash(&normalize_table_name(table_name))
    )
}

fn migration_name(file_path: &str) -> String {
    Path::new(file_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file_path)
        .to_string()
}

fn migration_version(file_path: &str) -> Option<String> {
    let name = migration_name(file_path);
    flyway_file_name(&name)
        .or_else(|| {
            name.split('_')
                .next()
                .filter(|part| part.chars().all(|ch| ch.is_ascii_digit()))
                .map(str::to_string)
        })
        .filter(|version| !version.is_empty())
}

fn flyway_file_name(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let stripped = lower
        .strip_prefix('v')
        .or_else(|| lower.strip_prefix('u'))
        .or_else(|| lower.strip_prefix('r'))?;
    let version = stripped.split("__").next()?;
    (!version.is_empty()
        && version
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch == '_' || ch == '.'))
    .then(|| version.to_string())
}

fn normalize_table_name(name: &str) -> String {
    name.trim_matches('"')
        .trim_matches('`')
        .trim_matches('[')
        .trim_matches(']')
        .rsplit('.')
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase()
}

fn clean_table_name(name: &str) -> Option<String> {
    let normalized = name
        .trim()
        .trim_matches('"')
        .trim_matches('`')
        .trim_matches('[')
        .trim_matches(']')
        .rsplit('.')
        .next()
        .unwrap_or(name)
        .trim();
    is_plausible_table_name(normalized).then(|| normalized.to_string())
}

fn is_plausible_table_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(
            name.to_ascii_lowercase().as_str(),
            "select" | "from" | "where" | "index" | "constraint" | "primary" | "foreign"
        )
        && name
            .chars()
            .all(|ch| ch == '_' || ch == '-' || ch.is_ascii_alphanumeric())
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
