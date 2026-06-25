//! Optional post-baseline analyzer merge.
//!
//! Providers here must wrap mature OSS analyzer results. The baseline graph and
//! search index are already ready before this module runs, so every error is
//! reported as an outcome and left to callers to log/skip.

use std::path::Path;

use aka_core::{
    EngineEvent, FactBatch, FactSourceError, LspEnrichmentPolicy, PipelineProgress, PipelineStage,
    RepoPaths,
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

pub fn run_optional_enrichment(
    repo: &Path,
    paths: &RepoPaths,
    policy: LspEnrichmentPolicy,
    on_event: &mut dyn FnMut(&EngineEvent),
) -> OptionalEnrichmentOutcome {
    run_optional_enrichment_with_providers(repo, paths, policy, &[], on_event)
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

    use aka_core::{ChunkFact, EdgeFact, FactStats, NodeFact};
    use aka_graph::GraphStore;

    use super::*;
    use crate::indexer::index_facts;

    struct BatchProvider {
        batch: FactBatch,
    }

    impl OptionalEnrichmentProvider for BatchProvider {
        fn id(&self) -> &'static str {
            "fake-scip"
        }

        fn produce(&self, _repo: &Path) -> Result<Option<FactBatch>, FactSourceError> {
            Ok(Some(self.batch.clone()))
        }
    }

    struct ErrorProvider;

    impl OptionalEnrichmentProvider for ErrorProvider {
        fn id(&self) -> &'static str {
            "fake-error"
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
                if stream == "lsp-enrichment" && line.contains("provider=fake-error failed")
        )));
    }
}
