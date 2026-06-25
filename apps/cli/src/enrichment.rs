//! Optional post-baseline analyzer merge.
//!
//! Providers here must wrap mature OSS analyzer results. The baseline graph and
//! search index are already ready before this module runs, so every error is
//! reported as an outcome and left to callers to log/skip.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aka_core::{
    find_oss_analyzer, AnalyzerRunMetadata, EngineEvent, FactBatch, FactSourceError,
    LspEnrichmentPolicy, PipelineProgress, PipelineStage, RepoPaths,
};

use crate::indexer::{merge_enrichment_facts_with_progress, EnrichmentMergeSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionalEnrichmentOutcome {
    Disabled,
    NoProviders,
    Merged(EnrichmentMergeSummary),
    Failed(String),
}

pub trait OptionalEnrichmentProvider {
    fn id(&self) -> &'static str;

    fn produce(&self, repo: &Path) -> Result<Option<FactBatch>, FactSourceError>;
}

#[derive(Debug, Clone)]
pub struct EnrichmentProviderConfig {
    pub scip_index_path: Option<PathBuf>,
}

pub fn run_optional_enrichment(
    repo: &Path,
    paths: &RepoPaths,
    policy: LspEnrichmentPolicy,
    config: EnrichmentProviderConfig,
    on_event: &mut dyn FnMut(&EngineEvent),
) -> OptionalEnrichmentOutcome {
    #[cfg(feature = "scip-import")]
    {
        let scip = ScipIndexProvider::new(config.scip_index_path, policy.max_duration);
        let providers: [&dyn OptionalEnrichmentProvider; 1] = [&scip];
        run_optional_enrichment_with_providers(repo, paths, policy, &providers, on_event)
    }
    #[cfg(not(feature = "scip-import"))]
    {
        let _ = config;
        run_optional_enrichment_with_providers(repo, paths, policy, &[], on_event)
    }
}

pub fn run_optional_enrichment_with_providers(
    repo: &Path,
    paths: &RepoPaths,
    policy: LspEnrichmentPolicy,
    providers: &[&dyn OptionalEnrichmentProvider],
    on_event: &mut dyn FnMut(&EngineEvent),
) -> OptionalEnrichmentOutcome {
    if !policy.enabled {
        emit_skipped(
            on_event,
            format!("OSS analyzer enrichment disabled for {}", repo.display()),
            "skipped enabled=false reason=disabled",
        );
        return OptionalEnrichmentOutcome::Disabled;
    }

    if providers.is_empty() {
        emit_skipped(
            on_event,
            format!(
                "OSS analyzer enrichment skipped for {}: no providers installed",
                repo.display()
            ),
            format!(
                "skipped enabled=true providers=0 max_secs={} reason=no_providers",
                policy.max_duration.as_secs()
            ),
        );
        return OptionalEnrichmentOutcome::NoProviders;
    }

    for provider in providers {
        if find_oss_analyzer(provider.id()).is_none() {
            let message = format!(
                "provider={} rejected reason=unsupported_analyzer",
                provider.id()
            );
            emit_log(on_event, message.clone());
            return OptionalEnrichmentOutcome::Failed(message);
        }
        emit_log(
            on_event,
            format!(
                "provider={} start max_secs={}",
                provider.id(),
                policy.max_duration.as_secs()
            ),
        );
        match provider.produce(repo) {
            Ok(Some(batch)) => {
                let mut index_progress = |event: crate::indexer::IndexProgressEvent| {
                    on_event(&EngineEvent::Progress {
                        progress: PipelineProgress::new(
                            PipelineStage::LspEnrichment,
                            event.message.clone(),
                        )
                        .counts(event.current.unwrap_or(0), event.total.unwrap_or(0)),
                    });
                    on_event(&EngineEvent::Phase {
                        phase: event.stage.into(),
                        current: event.current.unwrap_or(0),
                        total: event.total.unwrap_or(0),
                    });
                    emit_log(on_event, format!("{}: {}", event.stage, event.message));
                };
                match merge_enrichment_facts_with_progress(&batch, paths, Some(&mut index_progress))
                {
                    Ok(summary) => {
                        emit_log(
                            on_event,
                            format!(
                                "provider={} merged nodes={} edges={} duplicate_edges={} dangling_edges={}",
                                provider.id(),
                                summary.new_nodes,
                                summary.new_edges,
                                summary.duplicate_edges,
                                summary.dangling_edges
                            ),
                        );
                        return OptionalEnrichmentOutcome::Merged(summary);
                    }
                    Err(err) => {
                        let message =
                            format!("provider={} merge_failed error={err}", provider.id());
                        emit_log(on_event, message.clone());
                        return OptionalEnrichmentOutcome::Failed(message);
                    }
                }
            }
            Ok(None) => {
                emit_log(
                    on_event,
                    format!("provider={} skipped reason=no_facts", provider.id()),
                );
            }
            Err(err) => {
                let message = format!("provider={} failed error={err}", provider.id());
                emit_log(on_event, message.clone());
                return OptionalEnrichmentOutcome::Failed(message);
            }
        }
    }

    emit_skipped(
        on_event,
        format!(
            "OSS analyzer enrichment skipped for {}: providers produced no facts",
            repo.display()
        ),
        format!(
            "skipped enabled=true providers={} reason=no_facts",
            providers.len()
        ),
    );
    OptionalEnrichmentOutcome::NoProviders
}

#[cfg(feature = "scip-import")]
#[derive(Debug, Clone)]
struct ScipIndexProvider {
    configured_path: Option<PathBuf>,
    max_duration: Duration,
}

#[cfg(feature = "scip-import")]
impl ScipIndexProvider {
    fn new(configured_path: Option<PathBuf>, max_duration: Duration) -> Self {
        Self {
            configured_path,
            max_duration,
        }
    }

    fn candidate_path(&self, repo: &Path) -> PathBuf {
        self.configured_path
            .clone()
            .unwrap_or_else(|| repo.join("index.scip"))
    }
}

#[cfg(feature = "scip-import")]
impl OptionalEnrichmentProvider for ScipIndexProvider {
    fn id(&self) -> &'static str {
        "scip"
    }

    fn produce(&self, repo: &Path) -> Result<Option<FactBatch>, FactSourceError> {
        let started_at = Instant::now();
        let path = self.candidate_path(repo);
        if !path.is_file() {
            return Ok(None);
        }
        let (metadata, bundle) = aka_core::import_scip_path_with_metadata(&path)
            .map_err(|err| FactSourceError::Message(err.to_string()))?;
        let tool_version = metadata.tool_version.clone().ok_or_else(|| {
            FactSourceError::Message(format!(
                "SCIP index {} missing metadata.tool_info.version",
                path.display()
            ))
        })?;
        let provenance = AnalyzerRunMetadata::new("scip", tool_version)
            .map_err(|err| FactSourceError::Message(err.to_string()))?;
        let mut batch = bundle.lower();
        if started_at.elapsed() >= self.max_duration {
            return Err(FactSourceError::Message(format!(
                "SCIP import timed out after decode path={} max_secs={}",
                path.display(),
                self.max_duration.as_secs()
            )));
        }
        aka_core::stamp_enrichment_batch(&mut batch, &provenance)
            .map_err(|err| FactSourceError::Message(err.to_string()))?;
        Ok(Some(batch))
    }
}

fn emit_skipped(on_event: &mut dyn FnMut(&EngineEvent), message: String, line: impl Into<String>) {
    on_event(&EngineEvent::Progress {
        progress: PipelineProgress::new(PipelineStage::LspEnrichment, message),
    });
    emit_log(on_event, line.into());
}

fn emit_log(on_event: &mut dyn FnMut(&EngineEvent), line: impl Into<String>) {
    on_event(&EngineEvent::Log {
        stream: "lsp-enrichment".into(),
        line: line.into(),
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use aka_core::{ChunkFact, EdgeFact, FactSource, FactStats, NodeFact};
    use aka_graph::GraphStore;

    use super::*;
    use crate::indexer::index_facts;

    struct BatchProvider {
        batch: FactBatch,
    }

    impl OptionalEnrichmentProvider for BatchProvider {
        fn id(&self) -> &'static str {
            "scip"
        }

        fn produce(&self, _repo: &Path) -> Result<Option<FactBatch>, FactSourceError> {
            Ok(Some(self.batch.clone()))
        }
    }

    struct ErrorProvider;

    impl OptionalEnrichmentProvider for ErrorProvider {
        fn id(&self) -> &'static str {
            "scip"
        }

        fn produce(&self, _repo: &Path) -> Result<Option<FactBatch>, FactSourceError> {
            Err(FactSourceError::Message("provider exploded".into()))
        }
    }

    #[test]
    fn optional_enrichment_merges_provider_batch_without_failing_baseline() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = RepoPaths {
            root: tmp.path().join("aka-data").join("repo"),
        };
        std::fs::create_dir_all(&paths.root).unwrap();
        let baseline = FactBatch::new(
            FactStats {
                files: 0,
                nodes: 1,
                edges: 0,
                chunks: 0,
            },
            vec![NodeFact {
                id: "sym:handler".into(),
                label: "Function".into(),
                properties: serde_json::json!({"name": "handler"})
                    .as_object()
                    .unwrap()
                    .clone(),
            }],
            Vec::new(),
            Vec::new(),
        );
        index_facts(&baseline, &paths).unwrap();
        let provider = BatchProvider {
            batch: FactBatch::new(
                FactStats {
                    files: 0,
                    nodes: 1,
                    edges: 1,
                    chunks: 1,
                },
                vec![NodeFact {
                    id: "scip:symbol:service".into(),
                    label: "Interface".into(),
                    properties: serde_json::json!({
                        "name": "Service",
                        "source": "scip",
                        "provenance": {"analyzerId": "scip", "toolVersion": "1.0"}
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                }],
                vec![EdgeFact {
                    id: "scip:edge:handler-service".into(),
                    source_id: "sym:handler".into(),
                    target_id: "scip:symbol:service".into(),
                    edge_type: "DEPENDS_ON".into(),
                    confidence: 1.0,
                    reason: "scip relationship".into(),
                    step: None,
                    evidence: Some(serde_json::json!({
                        "source": "scip",
                        "provenance": {"analyzerId": "scip", "toolVersion": "1.0"}
                    })),
                }],
                vec![ChunkFact {
                    node_id: "scip:symbol:service".into(),
                    kind: "scip-symbol".into(),
                    file_path: "src/lib.rs".into(),
                    start_line: 0,
                    end_line: 0,
                    text: "trait Service".into(),
                }],
            ),
        };
        let events = RefCell::new(Vec::new());

        let outcome = run_optional_enrichment_with_providers(
            &repo,
            &paths,
            LspEnrichmentPolicy {
                enabled: true,
                max_duration: std::time::Duration::from_secs(5),
            },
            &[&provider],
            &mut |event| events.borrow_mut().push(event.clone()),
        );

        let OptionalEnrichmentOutcome::Merged(summary) = outcome else {
            panic!("expected merged outcome, got {outcome:?}");
        };
        assert_eq!(summary.new_nodes, 1);
        assert_eq!(summary.new_edges, 1);
        let graph = GraphStore::open(&paths.graph_db()).unwrap();
        assert_eq!(graph.node_count().unwrap(), 2);
        assert_eq!(graph.edge_count().unwrap(), 1);
        assert!(events.borrow().iter().any(|event| matches!(
            event,
            EngineEvent::Log { stream, line }
                if stream == "lsp-enrichment" && line.contains("merged nodes=1 edges=1")
        )));
    }

    #[test]
    fn optional_enrichment_provider_error_is_reported_as_failed_outcome() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = RepoPaths {
            root: tmp.path().join("aka-data").join("repo"),
        };
        let events = RefCell::new(Vec::new());

        let outcome = run_optional_enrichment_with_providers(
            &repo,
            &paths,
            LspEnrichmentPolicy {
                enabled: true,
                max_duration: std::time::Duration::from_secs(5),
            },
            &[&ErrorProvider],
            &mut |event| events.borrow_mut().push(event.clone()),
        );

        assert!(matches!(outcome, OptionalEnrichmentOutcome::Failed(_)));
        assert!(events.borrow().iter().any(|event| matches!(
            event,
            EngineEvent::Log { stream, line }
                if stream == "lsp-enrichment" && line.contains("provider=scip failed")
        )));
    }

    struct UnsupportedProvider;

    impl OptionalEnrichmentProvider for UnsupportedProvider {
        fn id(&self) -> &'static str {
            "custom-rust-heuristic"
        }

        fn produce(&self, _repo: &Path) -> Result<Option<FactBatch>, FactSourceError> {
            Ok(None)
        }
    }

    #[test]
    fn optional_enrichment_rejects_non_allowlisted_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = RepoPaths {
            root: tmp.path().join("aka-data").join("repo"),
        };

        let outcome = run_optional_enrichment_with_providers(
            &repo,
            &paths,
            LspEnrichmentPolicy {
                enabled: true,
                max_duration: Duration::from_secs(5),
            },
            &[&UnsupportedProvider],
            &mut |_| {},
        );

        assert!(
            matches!(outcome, OptionalEnrichmentOutcome::Failed(message) if message.contains("unsupported_analyzer"))
        );
    }

    #[cfg(feature = "scip-import")]
    #[test]
    fn scip_provider_skips_when_index_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let provider = ScipIndexProvider::new(None, Duration::from_secs(5));

        let batch = provider.produce(&repo).unwrap();

        assert!(batch.is_none());
    }

    #[cfg(feature = "scip-import")]
    #[test]
    fn scip_provider_imports_existing_index_and_stamps_provenance() {
        use protobuf::{Enum, EnumOrUnknown, Message};
        use scip::types::{
            symbol_information, Document, Index, Metadata, Occurrence, SymbolInformation,
            SymbolRole, ToolInfo,
        };

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let index_path = repo.join("index.scip");
        let mut index = Index::new();
        let mut metadata = Metadata::new();
        let mut tool_info = ToolInfo::new();
        tool_info.name = "scip-java".into();
        tool_info.version = "0.3.0".into();
        metadata.tool_info = Some(tool_info).into();
        index.metadata = Some(metadata).into();
        let mut doc = Document::new();
        doc.language = "java".into();
        doc.relative_path = "src/main/java/demo/Service.java".into();
        let mut service = SymbolInformation::new();
        service.symbol = "java maven demo 1.0.0 demo/Service#".into();
        service.display_name = "Service".into();
        service.kind = EnumOrUnknown::new(symbol_information::Kind::Interface);
        let mut def = Occurrence::new();
        def.symbol = service.symbol.clone();
        def.range = vec![1, 0, 1, 7];
        def.symbol_roles = SymbolRole::Definition.value();
        doc.symbols.push(service);
        doc.occurrences.push(def);
        index.documents.push(doc);
        std::fs::write(&index_path, index.write_to_bytes().unwrap()).unwrap();
        let provider = ScipIndexProvider::new(None, Duration::from_secs(5));

        let batch = provider.produce(&repo).unwrap().expect("scip facts");
        let node = batch
            .nodes()
            .unwrap()
            .filter_map(Result::ok)
            .find(|node| node.id.starts_with("scip:symbol:"))
            .expect("scip symbol node");

        assert_eq!(
            node.properties.get("source"),
            Some(&serde_json::json!("scip"))
        );
        assert_eq!(
            node.properties
                .get("provenance")
                .and_then(|value| value.get("toolVersion")),
            Some(&serde_json::json!("0.3.0"))
        );
    }
}
