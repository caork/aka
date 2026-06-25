//! Optional post-baseline enrichment guards.
//!
//! Baseline engine facts, graph, and search must be usable without this module.
//! Future providers must wrap mature OSS analyzers and must report
//! skipped/timeout outcomes instead of failing the indexing job. Analyzer
//! output is accepted only through the allowlist below and is stamped with
//! provenance before it can be merged into graph/search facts.

use std::path::Path;
use std::time::Duration;

use aka_facts::{EdgeFact, FactBatch, NodeFact};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::settings::AkaSettings;
use crate::types::{EngineEvent, PipelineProgress, PipelineStage};

const ENRICHMENT_ADAPTER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OssAnalyzerEnrichmentPolicy {
    pub enabled: bool,
    pub max_duration: Duration,
}

impl OssAnalyzerEnrichmentPolicy {
    pub fn from_settings(settings: AkaSettings) -> Self {
        Self {
            enabled: settings.oss_analyzer_enrichment_enabled,
            max_duration: Duration::from_secs(settings.oss_analyzer_enrichment_max_secs),
        }
    }
}

impl Default for OssAnalyzerEnrichmentPolicy {
    fn default() -> Self {
        Self::from_settings(AkaSettings::default())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OssAnalyzerEnrichmentOutcome {
    Disabled,
    NoProviders,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OssAnalyzerKind {
    Scip,
    StackGraphs,
    Lsp,
}

impl OssAnalyzerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scip => "scip",
            Self::StackGraphs => "stack-graphs",
            Self::Lsp => "lsp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OssAnalyzer {
    pub id: &'static str,
    pub display_name: &'static str,
    pub kind: OssAnalyzerKind,
    pub fact_source: &'static str,
    pub default_enabled: bool,
}

const OSS_ANALYZERS: &[OssAnalyzer] = &[
    OssAnalyzer {
        id: "scip",
        display_name: "SCIP",
        kind: OssAnalyzerKind::Scip,
        fact_source: "scip",
        default_enabled: false,
    },
    OssAnalyzer {
        id: "stack-graphs",
        display_name: "tree-sitter stack-graphs",
        kind: OssAnalyzerKind::StackGraphs,
        fact_source: "stack-graphs",
        default_enabled: false,
    },
    OssAnalyzer {
        id: "rust-analyzer",
        display_name: "rust-analyzer",
        kind: OssAnalyzerKind::Lsp,
        fact_source: "lsp",
        default_enabled: false,
    },
    OssAnalyzer {
        id: "pyright",
        display_name: "Pyright",
        kind: OssAnalyzerKind::Lsp,
        fact_source: "lsp",
        default_enabled: false,
    },
    OssAnalyzer {
        id: "jdtls",
        display_name: "Eclipse JDT Language Server",
        kind: OssAnalyzerKind::Lsp,
        fact_source: "lsp",
        default_enabled: false,
    },
    OssAnalyzer {
        id: "typescript-language-server",
        display_name: "TypeScript Language Server",
        kind: OssAnalyzerKind::Lsp,
        fact_source: "lsp",
        default_enabled: false,
    },
    OssAnalyzer {
        id: "gopls",
        display_name: "gopls",
        kind: OssAnalyzerKind::Lsp,
        fact_source: "lsp",
        default_enabled: false,
    },
];

pub fn allowed_oss_analyzers() -> &'static [OssAnalyzer] {
    OSS_ANALYZERS
}

pub fn allowed_lsp_analyzers() -> impl Iterator<Item = &'static OssAnalyzer> {
    OSS_ANALYZERS
        .iter()
        .filter(|analyzer| analyzer.kind == OssAnalyzerKind::Lsp)
}

pub fn find_oss_analyzer(id: &str) -> Option<&'static OssAnalyzer> {
    let canonical = canonical_oss_analyzer_id(id)?;
    OSS_ANALYZERS
        .iter()
        .find(|analyzer| analyzer.id == canonical)
}

fn canonical_oss_analyzer_id(id: &str) -> Option<&'static str> {
    match id.trim().to_ascii_lowercase().as_str() {
        "scip" | "scip-cli" => Some("scip"),
        "stack-graphs" | "tree-sitter-stack-graphs" | "tree_sitter_stack_graphs" => {
            Some("stack-graphs")
        }
        "rust-analyzer" | "rust_analyzer" => Some("rust-analyzer"),
        "pyright" => Some("pyright"),
        "jdtls" | "eclipse-jdt-language-server" | "eclipse.jdt.ls" => Some("jdtls"),
        "typescript-language-server"
        | "typescript-language-server/tsserver"
        | "tsserver"
        | "typescript" => Some("typescript-language-server"),
        "gopls" => Some("gopls"),
        _ => None,
    }
}

#[derive(Debug, Error)]
pub enum EnrichmentError {
    #[error(
        "unsupported enrichment analyzer {id:?}; only mature OSS analyzer results may enrich the graph"
    )]
    UnsupportedAnalyzer { id: String },
    #[error("enrichment analyzer {id:?} did not report a tool version")]
    MissingToolVersion { id: String },
    #[error("enrichment {fact_kind} {fact_id:?} is missing OSS analyzer provenance")]
    MissingProvenance {
        fact_kind: &'static str,
        fact_id: String,
    },
    #[error(
        "enrichment {fact_kind} {fact_id:?} provenance analyzer {id:?} is not in the OSS analyzer allowlist"
    )]
    UnsupportedProvenanceAnalyzer {
        fact_kind: &'static str,
        fact_id: String,
        id: String,
    },
    #[error("enrichment {fact_kind} {fact_id:?} provenance is missing analyzer toolVersion")]
    MissingProvenanceToolVersion {
        fact_kind: &'static str,
        fact_id: String,
    },
    #[error("enrichment {fact_kind} {fact_id:?} provenance is missing analyzer source")]
    MissingProvenanceSource {
        fact_kind: &'static str,
        fact_id: String,
    },
    #[error("enrichment {fact_kind} {fact_id:?} provenance is missing adapterVersion")]
    MissingProvenanceAdapterVersion {
        fact_kind: &'static str,
        fact_id: String,
    },
    #[error("enrichment {fact_kind} {fact_id:?} provenance did not mark oss=true")]
    MissingOpenSourceProvenance {
        fact_kind: &'static str,
        fact_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzerRunMetadata {
    pub analyzer_id: String,
    pub analyzer_kind: OssAnalyzerKind,
    pub tool: String,
    pub tool_version: String,
    pub adapter_version: String,
    pub source: String,
    pub oss: bool,
}

impl AnalyzerRunMetadata {
    pub fn new(
        analyzer_id: impl AsRef<str>,
        tool_version: impl Into<String>,
    ) -> Result<Self, EnrichmentError> {
        let raw_id = analyzer_id.as_ref();
        let analyzer =
            find_oss_analyzer(raw_id).ok_or_else(|| EnrichmentError::UnsupportedAnalyzer {
                id: raw_id.to_string(),
            })?;
        let tool_version = tool_version.into();
        if tool_version.trim().is_empty() {
            return Err(EnrichmentError::MissingToolVersion {
                id: analyzer.id.into(),
            });
        }
        Ok(Self {
            analyzer_id: analyzer.id.into(),
            analyzer_kind: analyzer.kind,
            tool: analyzer.display_name.into(),
            tool_version,
            adapter_version: ENRICHMENT_ADAPTER_VERSION.into(),
            source: analyzer.fact_source.into(),
            oss: true,
        })
    }

    pub fn analyzer(&self) -> Result<&'static OssAnalyzer, EnrichmentError> {
        find_oss_analyzer(&self.analyzer_id).ok_or_else(|| EnrichmentError::UnsupportedAnalyzer {
            id: self.analyzer_id.clone(),
        })
    }

    fn provenance_value(&self) -> Value {
        serde_json::json!({
            "source": self.source,
            "analyzerId": self.analyzer_id,
            "analyzerKind": self.analyzer_kind.as_str(),
            "tool": self.tool,
            "toolVersion": self.tool_version,
            "adapterVersion": self.adapter_version,
            "oss": self.oss,
        })
    }
}

pub fn stamp_enrichment_batch(
    batch: &mut FactBatch,
    metadata: &AnalyzerRunMetadata,
) -> Result<(), EnrichmentError> {
    metadata.analyzer()?;
    for node in &mut batch.nodes {
        stamp_enrichment_node(node, metadata);
    }
    for edge in &mut batch.edges {
        stamp_enrichment_edge(edge, metadata);
    }
    Ok(())
}

pub fn stamp_enrichment_node(node: &mut NodeFact, metadata: &AnalyzerRunMetadata) {
    node.properties
        .insert("source".into(), Value::String(metadata.source.clone()));
    node.properties
        .insert("provenance".into(), metadata.provenance_value());
}

pub fn stamp_enrichment_edge(edge: &mut EdgeFact, metadata: &AnalyzerRunMetadata) {
    let provenance = metadata.provenance_value();
    match edge.evidence.take() {
        Some(Value::Object(mut evidence)) => {
            evidence.insert("source".into(), Value::String(metadata.source.clone()));
            evidence.insert("provenance".into(), provenance);
            edge.evidence = Some(Value::Object(evidence));
        }
        Some(existing) => {
            let mut evidence = Map::new();
            evidence.insert("source".into(), Value::String(metadata.source.clone()));
            evidence.insert("value".into(), existing);
            evidence.insert("provenance".into(), provenance);
            edge.evidence = Some(Value::Object(evidence));
        }
        None => {
            let mut evidence = Map::new();
            evidence.insert("source".into(), Value::String(metadata.source.clone()));
            evidence.insert("provenance".into(), provenance);
            edge.evidence = Some(Value::Object(evidence));
        }
    }
}

pub fn validate_enrichment_batch_provenance(batch: &FactBatch) -> Result<(), EnrichmentError> {
    for node in &batch.nodes {
        let provenance = node
            .properties
            .get("provenance")
            .ok_or_else(|| missing_provenance("node", &node.id))?;
        validate_provenance("node", &node.id, provenance)?;
    }

    for edge in &batch.edges {
        let provenance = edge
            .evidence
            .as_ref()
            .and_then(|evidence| evidence.get("provenance"))
            .ok_or_else(|| missing_provenance("edge", &edge.id))?;
        validate_provenance("edge", &edge.id, provenance)?;
    }

    Ok(())
}

fn validate_provenance(
    fact_kind: &'static str,
    fact_id: &str,
    provenance: &Value,
) -> Result<(), EnrichmentError> {
    let analyzer_id =
        provenance_analyzer_id(provenance).ok_or_else(|| missing_provenance(fact_kind, fact_id))?;
    if find_oss_analyzer(analyzer_id).is_none() {
        return Err(EnrichmentError::UnsupportedProvenanceAnalyzer {
            fact_kind,
            fact_id: fact_id.to_string(),
            id: analyzer_id.to_string(),
        });
    }
    if provenance_str(provenance, "source").is_none() {
        return Err(EnrichmentError::MissingProvenanceSource {
            fact_kind,
            fact_id: fact_id.to_string(),
        });
    }
    if provenance_tool_version(provenance).is_none() {
        return Err(EnrichmentError::MissingProvenanceToolVersion {
            fact_kind,
            fact_id: fact_id.to_string(),
        });
    }
    if provenance_str(provenance, "adapterVersion").is_none() {
        return Err(EnrichmentError::MissingProvenanceAdapterVersion {
            fact_kind,
            fact_id: fact_id.to_string(),
        });
    }
    if provenance
        .as_object()
        .and_then(|provenance| provenance.get("oss"))
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err(EnrichmentError::MissingOpenSourceProvenance {
            fact_kind,
            fact_id: fact_id.to_string(),
        });
    }
    Ok(())
}

fn missing_provenance(fact_kind: &'static str, fact_id: &str) -> EnrichmentError {
    EnrichmentError::MissingProvenance {
        fact_kind,
        fact_id: fact_id.to_string(),
    }
}

fn provenance_analyzer_id(value: &Value) -> Option<&str> {
    provenance_str(value, "analyzerId")
}

fn provenance_tool_version(value: &Value) -> Option<&str> {
    provenance_str(value, "toolVersion")
}

fn provenance_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .as_object()
        .and_then(|provenance| provenance.get(key))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
}

pub fn run_optional_oss_analyzer_enrichment(
    repo: &Path,
    policy: OssAnalyzerEnrichmentPolicy,
    mut on_event: impl FnMut(&EngineEvent),
) -> OssAnalyzerEnrichmentOutcome {
    if !policy.enabled {
        emit_skipped(
            &mut on_event,
            format!("OSS analyzer enrichment disabled for {}", repo.display()),
            "skipped enabled=false reason=disabled",
        );
        return OssAnalyzerEnrichmentOutcome::Disabled;
    }

    emit_skipped(
        &mut on_event,
        format!(
            "OSS analyzer enrichment skipped for {}: no providers installed",
            repo.display()
        ),
        format!(
            "skipped enabled=true providers=0 allowed={} max_secs={} reason=no_providers",
            allowed_oss_analyzers()
                .iter()
                .map(|analyzer| analyzer.id)
                .collect::<Vec<_>>()
                .join(","),
            policy.max_duration.as_secs()
        ),
    );
    OssAnalyzerEnrichmentOutcome::NoProviders
}

fn emit_skipped(on_event: &mut impl FnMut(&EngineEvent), message: String, line: impl Into<String>) {
    on_event(&EngineEvent::Progress {
        progress: PipelineProgress::new(PipelineStage::OssAnalyzerEnrichment, message),
    });
    on_event(&EngineEvent::Log {
        stream: "oss-analyzer-enrichment".into(),
        line: line.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_oss_analyzer_enrichment_reports_skip_without_error() {
        let mut events = Vec::new();
        let outcome = run_optional_oss_analyzer_enrichment(
            Path::new("/repo"),
            OssAnalyzerEnrichmentPolicy::default(),
            |event| events.push(event.clone()),
        );

        assert_eq!(outcome, OssAnalyzerEnrichmentOutcome::Disabled);
        assert!(matches!(
            events.first(),
            Some(EngineEvent::Progress { progress })
                if progress.stage == PipelineStage::OssAnalyzerEnrichment
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Log { stream, line }
                if stream == "oss-analyzer-enrichment" && line.contains("reason=disabled")
        )));
    }

    #[test]
    fn enabled_oss_analyzer_enrichment_without_providers_is_skipped() {
        let mut events = Vec::new();
        let outcome = run_optional_oss_analyzer_enrichment(
            Path::new("/repo"),
            OssAnalyzerEnrichmentPolicy {
                enabled: true,
                max_duration: Duration::from_secs(15),
            },
            |event| events.push(event.clone()),
        );

        assert_eq!(outcome, OssAnalyzerEnrichmentOutcome::NoProviders);
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Log { stream, line }
                if stream == "oss-analyzer-enrichment"
                    && line.contains("providers=0")
                    && line.contains("allowed=scip,stack-graphs,rust-analyzer,pyright,jdtls,typescript-language-server,gopls")
                    && line.contains("max_secs=15")
        )));
    }

    #[test]
    fn oss_analyzer_allowlist_accepts_aliases() {
        let analyzer = find_oss_analyzer("tsserver").expect("tsserver alias");
        assert_eq!(analyzer.id, "typescript-language-server");
        assert_eq!(analyzer.kind, OssAnalyzerKind::Lsp);
        assert!(allowed_lsp_analyzers().any(|analyzer| analyzer.id == "pyright"));
    }

    #[test]
    fn unknown_analyzer_is_rejected() {
        let err = AnalyzerRunMetadata::new("unsupported-private-analyzer", "1.0").unwrap_err();
        assert!(matches!(
            err,
            EnrichmentError::UnsupportedAnalyzer { id } if id == "unsupported-private-analyzer"
        ));
    }

    #[test]
    fn analyzer_version_is_required_for_provenance() {
        let err = AnalyzerRunMetadata::new("pyright", "").unwrap_err();
        assert!(matches!(
            err,
            EnrichmentError::MissingToolVersion { id } if id == "pyright"
        ));
    }

    #[test]
    fn stamps_enrichment_facts_with_oss_provenance() {
        let metadata = AnalyzerRunMetadata::new("pyright", "1.2.3").expect("metadata");
        let mut batch = FactBatch::new(
            Default::default(),
            vec![NodeFact {
                id: "sym:handler".into(),
                label: "Function".into(),
                properties: serde_json::json!({ "source": "legacy-scan" })
                    .as_object()
                    .unwrap()
                    .clone(),
            }],
            vec![
                EdgeFact {
                    id: "edge:calls".into(),
                    source_id: "sym:handler".into(),
                    target_id: "sym:service".into(),
                    edge_type: "CALLS".into(),
                    confidence: 1.0,
                    reason: "lsp reference".into(),
                    step: None,
                    evidence: Some(serde_json::json!({
                        "source": "legacy-scan",
                        "rule": "references"
                    })),
                },
                EdgeFact {
                    id: "edge:def".into(),
                    source_id: "sym:handler".into(),
                    target_id: "file:app.py".into(),
                    edge_type: "DEFINES".into(),
                    confidence: 1.0,
                    reason: "lsp definition".into(),
                    step: None,
                    evidence: Some(Value::String("definition".into())),
                },
            ],
            Vec::new(),
        );

        stamp_enrichment_batch(&mut batch, &metadata).expect("stamp");

        assert_eq!(
            batch.nodes[0].properties.get("source"),
            Some(&Value::String("lsp".into()))
        );
        assert_eq!(
            batch.nodes[0]
                .properties
                .get("provenance")
                .and_then(|value| value.get("analyzerId")),
            Some(&Value::String("pyright".into()))
        );
        assert_eq!(
            batch.edges[0]
                .evidence
                .as_ref()
                .and_then(|value| value.get("source")),
            Some(&Value::String("lsp".into()))
        );
        assert_eq!(
            batch.edges[0]
                .evidence
                .as_ref()
                .and_then(|value| value.get("rule")),
            Some(&Value::String("references".into()))
        );
        assert_eq!(
            batch.edges[0]
                .evidence
                .as_ref()
                .and_then(|value| value.get("provenance"))
                .and_then(|value| value.get("toolVersion")),
            Some(&Value::String("1.2.3".into()))
        );
        assert_eq!(
            batch.edges[1]
                .evidence
                .as_ref()
                .and_then(|value| value.get("value")),
            Some(&Value::String("definition".into()))
        );
    }

    #[test]
    fn validates_enrichment_batch_provenance() {
        let metadata = AnalyzerRunMetadata::new("pyright", "1.2.3").expect("metadata");
        let mut batch = FactBatch::new(
            Default::default(),
            vec![NodeFact {
                id: "sym:handler".into(),
                label: "Function".into(),
                properties: Default::default(),
            }],
            vec![EdgeFact {
                id: "edge:calls".into(),
                source_id: "sym:handler".into(),
                target_id: "sym:service".into(),
                edge_type: "CALLS".into(),
                confidence: 1.0,
                reason: "lsp reference".into(),
                step: None,
                evidence: None,
            }],
            Vec::new(),
        );
        stamp_enrichment_batch(&mut batch, &metadata).expect("stamp");

        validate_enrichment_batch_provenance(&batch).expect("valid provenance");
    }

    #[test]
    fn rejects_enrichment_batch_without_tool_version() {
        let batch = FactBatch::new(
            Default::default(),
            vec![NodeFact {
                id: "sym:handler".into(),
                label: "Function".into(),
                properties: serde_json::json!({
                    "provenance": {
                        "source": "lsp",
                        "analyzerId": "pyright",
                        "analyzerKind": "lsp",
                        "tool": "Pyright",
                        "adapterVersion": "test",
                        "oss": true
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
            }],
            Vec::new(),
            Vec::new(),
        );

        let err = validate_enrichment_batch_provenance(&batch).unwrap_err();

        assert!(matches!(
            err,
            EnrichmentError::MissingProvenanceToolVersion {
                fact_kind: "node",
                fact_id
            } if fact_id == "sym:handler"
        ));
    }

    #[test]
    fn rejects_enrichment_batch_from_non_allowlisted_analyzer() {
        let batch = FactBatch::new(
            Default::default(),
            vec![NodeFact {
                id: "sym:handler".into(),
                label: "Function".into(),
                properties: serde_json::json!({
                    "provenance": {
                        "source": "custom",
                        "analyzerId": "unsupported-private-analyzer",
                        "analyzerKind": "lsp",
                        "tool": "Private analyzer",
                        "toolVersion": "1.0",
                        "adapterVersion": "test",
                        "oss": true
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
            }],
            Vec::new(),
            Vec::new(),
        );

        let err = validate_enrichment_batch_provenance(&batch).unwrap_err();

        assert!(matches!(
            err,
            EnrichmentError::UnsupportedProvenanceAnalyzer {
                fact_kind: "node",
                fact_id,
                id
            } if fact_id == "sym:handler" && id == "unsupported-private-analyzer"
        ));
    }

    #[test]
    fn rejects_enrichment_batch_without_adapter_version() {
        let batch = FactBatch::new(
            Default::default(),
            vec![NodeFact {
                id: "sym:handler".into(),
                label: "Function".into(),
                properties: serde_json::json!({
                    "provenance": {
                        "source": "lsp",
                        "analyzerId": "pyright",
                        "analyzerKind": "lsp",
                        "tool": "Pyright",
                        "toolVersion": "1.2.3",
                        "oss": true
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
            }],
            Vec::new(),
            Vec::new(),
        );

        let err = validate_enrichment_batch_provenance(&batch).unwrap_err();

        assert!(matches!(
            err,
            EnrichmentError::MissingProvenanceAdapterVersion {
                fact_kind: "node",
                fact_id
            } if fact_id == "sym:handler"
        ));
    }
}
