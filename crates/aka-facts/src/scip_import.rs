//! SCIP importer for mature OSS analyzer results.
//!
//! This module only converts an existing `index.scip` into `aka-facts`.
//! Running language-specific indexers remains outside this crate, and graph
//! enrichment still needs the aka-core allowlist/provenance guard before merge.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use protobuf::{Enum, Message};
use scip::types::{
    symbol_information, Document, Index, Occurrence, Relationship, SymbolInformation, SymbolRole,
};
use serde_json::Value;
use thiserror::Error;

use crate::{
    FactId, FileFact, JsonMap, OccurrenceFact, OccurrenceRole, RelationFact, RelationKind,
    SemanticFactBundle, SymbolFact, SymbolKind, TextRange,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipIndexMetadata {
    pub tool_name: Option<String>,
    pub tool_version: Option<String>,
    pub project_root: Option<String>,
}

#[derive(Debug, Error)]
pub enum ScipImportError {
    #[error("read SCIP index {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("decode SCIP index {path}: {source}")]
    Decode {
        path: String,
        #[source]
        source: protobuf::Error,
    },
}

pub fn import_scip_path(path: &Path) -> Result<SemanticFactBundle, ScipImportError> {
    read_scip_index(path).map(|index| import_scip_index(&index))
}

pub fn import_scip_path_with_metadata(
    path: &Path,
) -> Result<(ScipIndexMetadata, SemanticFactBundle), ScipImportError> {
    let index = read_scip_index(path)?;
    Ok((scip_index_metadata(&index), import_scip_index(&index)))
}

pub fn read_scip_index_metadata(path: &Path) -> Result<ScipIndexMetadata, ScipImportError> {
    Ok(scip_index_metadata(&read_scip_index(path)?))
}

fn read_scip_index(path: &Path) -> Result<Index, ScipImportError> {
    let bytes = fs::read(path).map_err(|source| ScipImportError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let index = Index::parse_from_bytes(&bytes).map_err(|source| ScipImportError::Decode {
        path: path.display().to_string(),
        source,
    })?;
    Ok(index)
}

fn scip_index_metadata(index: &Index) -> ScipIndexMetadata {
    let metadata = index.metadata.as_ref();
    let tool_info = metadata.and_then(|metadata| metadata.tool_info.as_ref());
    ScipIndexMetadata {
        tool_name: tool_info.and_then(|tool| empty_to_none(&tool.name)),
        tool_version: tool_info.and_then(|tool| empty_to_none(&tool.version)),
        project_root: metadata.and_then(|metadata| empty_to_none(&metadata.project_root)),
    }
}

pub fn import_scip_index(index: &Index) -> SemanticFactBundle {
    let mut importer = ScipImporter::default();
    for document in &index.documents {
        importer.import_document(document);
    }
    for symbol in &index.external_symbols {
        importer.import_symbol(symbol, None, None);
    }
    importer.finish()
}

#[derive(Default)]
struct ScipImporter {
    files: BTreeMap<FactId, FileFact>,
    symbols: BTreeMap<FactId, SymbolFact>,
    occurrences: Vec<OccurrenceFact>,
    relations: BTreeMap<FactId, RelationFact>,
    definition_ranges: BTreeMap<FactId, TextRange>,
}

impl ScipImporter {
    fn import_document(&mut self, document: &Document) {
        if document.relative_path.is_empty() {
            return;
        }
        let file_id = file_id(&document.relative_path);
        self.files.entry(file_id.clone()).or_insert_with(|| {
            let mut properties = JsonMap::new();
            properties.insert("source".into(), Value::String("scip".into()));
            FileFact {
                id: file_id.clone(),
                path: document.relative_path.clone(),
                language: empty_to_none(&document.language),
                digest: None,
                generated: false,
                properties,
            }
        });

        for occurrence in &document.occurrences {
            self.import_occurrence(occurrence, document, &file_id);
        }
        for symbol in &document.symbols {
            let range = self
                .definition_ranges
                .get(&symbol_id(&symbol.symbol))
                .cloned();
            self.import_symbol(symbol, Some(&document.relative_path), range);
        }
    }

    fn import_symbol(
        &mut self,
        symbol: &SymbolInformation,
        file_path: Option<&str>,
        range: Option<TextRange>,
    ) {
        if symbol.symbol.is_empty() {
            return;
        }
        let id = symbol_id(&symbol.symbol);
        let mut properties = JsonMap::new();
        properties.insert("source".into(), Value::String("scip".into()));
        properties.insert("scipSymbol".into(), Value::String(symbol.symbol.clone()));
        if !symbol.enclosing_symbol.is_empty() {
            properties.insert(
                "enclosingSymbol".into(),
                Value::String(symbol.enclosing_symbol.clone()),
            );
        }

        let documentation = if symbol.documentation.is_empty() {
            None
        } else {
            Some(symbol.documentation.join("\n\n"))
        };
        let symbol_fact = SymbolFact {
            id: id.clone(),
            symbol: symbol.symbol.clone(),
            name: display_name(symbol),
            qualified_name: Some(symbol.symbol.clone()),
            kind: symbol_kind(symbol),
            file_path: file_path.map(ToOwned::to_owned),
            range,
            documentation,
            properties,
        };
        self.symbols.entry(id.clone()).or_insert(symbol_fact);

        for relationship in &symbol.relationships {
            self.import_relationship(&id, relationship);
        }
    }

    fn import_occurrence(&mut self, occurrence: &Occurrence, document: &Document, file_id: &str) {
        if occurrence.symbol.is_empty() {
            return;
        }
        let Some(range) = occurrence_range(occurrence) else {
            return;
        };
        let symbol_id = symbol_id(&occurrence.symbol);
        let role = occurrence_role(occurrence.symbol_roles);
        if role == OccurrenceRole::Definition || role == OccurrenceRole::Declaration {
            self.definition_ranges
                .entry(symbol_id.clone())
                .or_insert_with(|| range.clone());
        }

        let mut properties = JsonMap::new();
        properties.insert("source".into(), Value::String("scip".into()));
        properties.insert(
            "symbolRoles".into(),
            Value::from(i64::from(occurrence.symbol_roles)),
        );
        properties.insert(
            "positionEncoding".into(),
            Value::String(format!(
                "{:?}",
                document.position_encoding.enum_value_or_default()
            )),
        );
        if let Ok(syntax) = occurrence.syntax_kind.enum_value() {
            properties.insert("syntaxKind".into(), Value::String(format!("{syntax:?}")));
        }

        let occurrence_id = occurrence_id(file_id, &symbol_id, &range, occurrence.symbol_roles);
        self.occurrences.push(OccurrenceFact {
            id: occurrence_id.clone(),
            symbol_id: symbol_id.clone(),
            file_id: file_id.to_string(),
            range: range.clone(),
            role: role.clone(),
            syntax_kind: None,
            properties,
        });

        if !matches!(
            role,
            OccurrenceRole::Definition | OccurrenceRole::Declaration
        ) {
            let edge_id = format!("{occurrence_id}:ref");
            self.relations
                .entry(edge_id.clone())
                .or_insert(RelationFact {
                    id: edge_id,
                    source: file_id.to_string(),
                    target: symbol_id,
                    kind: relation_kind_for_occurrence(&role),
                    confidence: 1.0,
                    reason: Some("scip occurrence".into()),
                    step: None,
                    evidence: Some(serde_json::json!({
                        "source": "scip",
                        "role": format!("{role:?}"),
                        "range": occurrence.range,
                    })),
                });
        }
    }

    fn import_relationship(&mut self, source: &str, relationship: &Relationship) {
        if relationship.symbol.is_empty() {
            return;
        }
        let target = symbol_id(&relationship.symbol);
        for (suffix, kind, flag) in [
            (
                "implementation",
                RelationKind::Implements,
                relationship.is_implementation,
            ),
            (
                "type-definition",
                RelationKind::DependsOn,
                relationship.is_type_definition,
            ),
            (
                "definition",
                RelationKind::Defines,
                relationship.is_definition,
            ),
            (
                "reference",
                RelationKind::DependsOn,
                relationship.is_reference,
            ),
        ] {
            if !flag {
                continue;
            }
            let edge_id = format!("{source}:scip:{suffix}:{target}");
            self.relations
                .entry(edge_id.clone())
                .or_insert(RelationFact {
                    id: edge_id,
                    source: source.to_string(),
                    target: target.clone(),
                    kind: kind.clone(),
                    confidence: 1.0,
                    reason: Some("scip relationship".into()),
                    step: None,
                    evidence: Some(serde_json::json!({
                        "source": "scip",
                        "relationship": suffix,
                        "targetSymbol": relationship.symbol,
                    })),
                });
        }
    }

    fn finish(self) -> SemanticFactBundle {
        SemanticFactBundle {
            files: self.files.into_values().collect(),
            symbols: self.symbols.into_values().collect(),
            occurrences: self.occurrences,
            relations: self.relations.into_values().collect(),
            chunks: Vec::new(),
        }
    }
}

fn file_id(path: &str) -> FactId {
    format!("file:{}", path.replace('\\', "/"))
}

fn symbol_id(symbol: &str) -> FactId {
    format!("scip:symbol:{symbol}")
}

fn occurrence_id(file_id: &str, symbol_id: &str, range: &TextRange, roles: i32) -> FactId {
    format!(
        "scip:occ:{file_id}:{symbol_id}:{}:{}:{}:{}:{roles}",
        range.start_line_0based,
        range.start_col_0based.unwrap_or_default(),
        range.end_line_0based,
        range.end_col_0based.unwrap_or_default()
    )
}

fn empty_to_none(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn display_name(symbol: &SymbolInformation) -> String {
    if !symbol.display_name.is_empty() {
        return symbol.display_name.clone();
    }
    scip::symbol::parse_symbol(&symbol.symbol)
        .ok()
        .and_then(|parsed| {
            parsed
                .descriptors
                .last()
                .filter(|descriptor| !descriptor.name.is_empty())
                .map(|descriptor| descriptor.name.clone())
        })
        .unwrap_or_else(|| symbol.symbol.clone())
}

fn symbol_kind(symbol: &SymbolInformation) -> SymbolKind {
    match symbol.kind.enum_value_or_default() {
        symbol_information::Kind::Class => SymbolKind::Class,
        symbol_information::Kind::Interface | symbol_information::Kind::Protocol => {
            SymbolKind::Interface
        }
        symbol_information::Kind::Enum => SymbolKind::Enum,
        symbol_information::Kind::Struct
        | symbol_information::Kind::Type
        | symbol_information::Kind::TypeAlias
        | symbol_information::Kind::Union => SymbolKind::Type,
        symbol_information::Kind::Trait => SymbolKind::Trait,
        symbol_information::Kind::Function => SymbolKind::Function,
        symbol_information::Kind::Method
        | symbol_information::Kind::AbstractMethod
        | symbol_information::Kind::Constructor
        | symbol_information::Kind::Getter
        | symbol_information::Kind::Setter
        | symbol_information::Kind::StaticMethod
        | symbol_information::Kind::TraitMethod => SymbolKind::Method,
        symbol_information::Kind::Field
        | symbol_information::Kind::Property
        | symbol_information::Kind::StaticField => SymbolKind::Field,
        symbol_information::Kind::Module | symbol_information::Kind::Namespace => {
            SymbolKind::Module
        }
        symbol_information::Kind::Package | symbol_information::Kind::PackageObject => {
            SymbolKind::Package
        }
        symbol_information::Kind::File => SymbolKind::File,
        symbol_information::Kind::Variable
        | symbol_information::Kind::Value
        | symbol_information::Kind::Parameter => SymbolKind::Variable,
        other => SymbolKind::Unknown(format!("{other:?}")),
    }
}

fn occurrence_range(occurrence: &Occurrence) -> Option<TextRange> {
    match occurrence.range.as_slice() {
        [start_line, start_col, end_col] => Some(TextRange {
            start_line_0based: to_u32(*start_line)?,
            end_line_0based: to_u32(*start_line)?,
            start_col_0based: Some(to_u32(*start_col)?),
            end_col_0based: Some(to_u32(*end_col)?),
        }),
        [start_line, start_col, end_line, end_col] => Some(TextRange {
            start_line_0based: to_u32(*start_line)?,
            end_line_0based: to_u32(*end_line)?,
            start_col_0based: Some(to_u32(*start_col)?),
            end_col_0based: Some(to_u32(*end_col)?),
        }),
        _ => None,
    }
}

fn to_u32(value: i32) -> Option<u32> {
    u32::try_from(value).ok()
}

fn occurrence_role(roles: i32) -> OccurrenceRole {
    if has_role(roles, SymbolRole::Definition) {
        OccurrenceRole::Definition
    } else if has_role(roles, SymbolRole::ForwardDefinition) {
        OccurrenceRole::Declaration
    } else if has_role(roles, SymbolRole::Import) {
        OccurrenceRole::Import
    } else if has_role(roles, SymbolRole::WriteAccess) {
        OccurrenceRole::Write
    } else if has_role(roles, SymbolRole::ReadAccess) {
        OccurrenceRole::Read
    } else {
        OccurrenceRole::Reference
    }
}

fn has_role(roles: i32, role: SymbolRole) -> bool {
    roles & role.value() != 0
}

fn relation_kind_for_occurrence(role: &OccurrenceRole) -> RelationKind {
    match role {
        OccurrenceRole::Import => RelationKind::Imports,
        OccurrenceRole::Write => RelationKind::Writes,
        OccurrenceRole::Read => RelationKind::Reads,
        _ => RelationKind::DependsOn,
    }
}

#[cfg(test)]
mod tests {
    use protobuf::EnumOrUnknown;
    use scip::types::{symbol_information, Occurrence, SymbolInformation};

    use super::*;

    #[test]
    fn imports_scip_symbols_occurrences_and_relationships() {
        let mut index = Index::new();
        let mut doc = Document::new();
        doc.language = "rust".into();
        doc.relative_path = "src/lib.rs".into();

        let mut service = SymbolInformation::new();
        service.symbol = "rust cargo demo 1.0.0 demo/Service#".into();
        service.display_name = "Service".into();
        service.kind = EnumOrUnknown::new(symbol_information::Kind::Trait);

        let mut handler = SymbolInformation::new();
        handler.symbol = "rust cargo demo 1.0.0 demo/handler().".into();
        handler.display_name = "handler".into();
        handler.kind = EnumOrUnknown::new(symbol_information::Kind::Function);
        let mut relationship = Relationship::new();
        relationship.symbol = service.symbol.clone();
        relationship.is_implementation = true;
        handler.relationships.push(relationship);

        let mut def = Occurrence::new();
        def.symbol = handler.symbol.clone();
        def.range = vec![2, 0, 2, 7];
        def.symbol_roles = SymbolRole::Definition.value();

        let mut reference = Occurrence::new();
        reference.symbol = service.symbol.clone();
        reference.range = vec![8, 4, 11];
        reference.symbol_roles = SymbolRole::ReadAccess.value();

        doc.symbols.push(service);
        doc.symbols.push(handler);
        doc.occurrences.push(def);
        doc.occurrences.push(reference);
        index.documents.push(doc);

        let bundle = import_scip_index(&index);

        assert_eq!(bundle.files.len(), 1);
        assert_eq!(bundle.files[0].path, "src/lib.rs");
        assert_eq!(bundle.symbols.len(), 2);
        let handler = bundle
            .symbols
            .iter()
            .find(|symbol| symbol.name == "handler")
            .expect("handler symbol");
        assert_eq!(handler.kind, SymbolKind::Function);
        assert_eq!(
            handler.range.as_ref().map(|range| range.start_line_0based),
            Some(2)
        );
        assert_eq!(bundle.occurrences.len(), 2);
        assert!(bundle
            .relations
            .iter()
            .any(|relation| relation.kind == RelationKind::Implements));
        assert!(bundle
            .relations
            .iter()
            .any(|relation| relation.kind == RelationKind::Reads));
    }

    #[test]
    fn imports_reference_edges_to_external_symbols() {
        let mut index = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "src/lib.rs".into();
        let mut occurrence = Occurrence::new();
        occurrence.symbol = "rust cargo dep 1.0.0 dep/Service#".into();
        occurrence.range = vec![4, 1, 6];
        occurrence.symbol_roles = SymbolRole::ReadAccess.value();
        doc.occurrences.push(occurrence);
        index.documents.push(doc);

        let bundle = import_scip_index(&index);

        assert_eq!(bundle.occurrences.len(), 1);
        assert_eq!(bundle.relations.len(), 1);
        assert_eq!(bundle.relations[0].kind, RelationKind::Reads);
        assert_eq!(bundle.relations[0].source, "file:src/lib.rs");
        assert_eq!(
            bundle.relations[0].target,
            "scip:symbol:rust cargo dep 1.0.0 dep/Service#"
        );
    }

    #[test]
    fn skips_invalid_occurrence_ranges() {
        let mut index = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "src/lib.rs".into();
        let mut occurrence = Occurrence::new();
        occurrence.symbol = "rust cargo demo 1.0.0 demo/handler().".into();
        occurrence.range = vec![1, 2];
        doc.occurrences.push(occurrence);
        index.documents.push(doc);

        let bundle = import_scip_index(&index);

        assert_eq!(bundle.files.len(), 1);
        assert!(bundle.occurrences.is_empty());
    }

    #[test]
    fn falls_back_to_symbol_string_for_display_name() {
        let mut symbol = SymbolInformation::new();
        symbol.symbol = "not a valid scip symbol".into();

        assert_eq!(display_name(&symbol), "not a valid scip symbol");
    }

    #[test]
    fn maps_three_part_range_to_single_line_span() {
        let mut occurrence = Occurrence::new();
        occurrence.range = vec![10, 3, 8];

        let range = occurrence_range(&occurrence).expect("range");

        assert_eq!(range.start_line_0based, 10);
        assert_eq!(range.end_line_0based, 10);
        assert_eq!(range.start_col_0based, Some(3));
        assert_eq!(range.end_col_0based, Some(8));
    }
}
