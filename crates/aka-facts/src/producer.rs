//! Producer interfaces for in-process semantic facts.
//!
//! Native parsers, SCIP importers, stack-graphs adapters, and live LSP
//! fallbacks all feed the same sink. This keeps transports such as JSONL
//! sidecars as optional debug output instead of part of the indexing contract.

use std::path::Path;

use crate::{
    FactBatch, FactSink, FactSourceError, FileFact, OccurrenceFact, RelationFact,
    SemanticFactBundle, SymbolFact,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProducerCapability {
    Files,
    Symbols,
    Occurrences,
    Relations,
    Chunks,
}

#[derive(Debug, Clone, Copy)]
pub struct ProducerContext<'a> {
    pub repo_root: &'a Path,
    pub no_chunks: bool,
}

pub trait SemanticFactSink {
    fn push_file(&mut self, fact: FileFact) -> Result<(), FactSourceError>;
    fn push_symbol(&mut self, fact: SymbolFact) -> Result<(), FactSourceError>;
    fn push_occurrence(&mut self, fact: OccurrenceFact) -> Result<(), FactSourceError>;
    fn push_relation(&mut self, fact: RelationFact) -> Result<(), FactSourceError>;
}

pub trait SemanticFactProducer {
    fn id(&self) -> &'static str;

    fn capabilities(&self) -> &'static [ProducerCapability];

    fn produce(
        &self,
        ctx: &ProducerContext<'_>,
        sink: &mut dyn SemanticFactSink,
    ) -> Result<(), FactSourceError>;
}

#[derive(Debug, Default)]
pub struct SemanticFactBundleBuilder {
    bundle: SemanticFactBundle,
}

impl SemanticFactBundleBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn finish(self) -> SemanticFactBundle {
        self.bundle
    }

    pub fn finish_lowered(self) -> FactBatch {
        self.finish().lower()
    }
}

impl SemanticFactSink for SemanticFactBundleBuilder {
    fn push_file(&mut self, fact: FileFact) -> Result<(), FactSourceError> {
        self.bundle.files.push(fact);
        Ok(())
    }

    fn push_symbol(&mut self, fact: SymbolFact) -> Result<(), FactSourceError> {
        self.bundle.symbols.push(fact);
        Ok(())
    }

    fn push_occurrence(&mut self, fact: OccurrenceFact) -> Result<(), FactSourceError> {
        self.bundle.occurrences.push(fact);
        Ok(())
    }

    fn push_relation(&mut self, fact: RelationFact) -> Result<(), FactSourceError> {
        self.bundle.relations.push(fact);
        Ok(())
    }
}

pub fn produce_semantic_batch(
    producers: &[&dyn SemanticFactProducer],
    ctx: &ProducerContext<'_>,
) -> Result<FactBatch, FactSourceError> {
    let mut sink = SemanticFactBundleBuilder::new();
    for producer in producers {
        producer.produce(ctx, &mut sink)?;
    }
    Ok(sink.finish_lowered())
}

pub fn produce_semantic_into(
    producers: &[&dyn SemanticFactProducer],
    ctx: &ProducerContext<'_>,
    sink: &mut dyn FactSink<Error = FactSourceError>,
) -> Result<(), FactSourceError> {
    let batch = produce_semantic_batch(producers, ctx)?;
    batch.replay_into(sink)
}

pub fn replay_semantic_bundle_into(
    bundle: SemanticFactBundle,
    sink: &mut dyn FactSink<Error = FactSourceError>,
) -> Result<(), FactSourceError> {
    bundle.lower().replay_into(sink)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::{
        ChunkFact, FactBatchBuilder, FactSource, JsonMap, OccurrenceRole, RelationKind, SymbolKind,
        TextRange,
    };

    struct FakeScipProducer;

    impl SemanticFactProducer for FakeScipProducer {
        fn id(&self) -> &'static str {
            "fake-scip"
        }

        fn capabilities(&self) -> &'static [ProducerCapability] {
            &[
                ProducerCapability::Files,
                ProducerCapability::Symbols,
                ProducerCapability::Occurrences,
                ProducerCapability::Relations,
                ProducerCapability::Chunks,
            ]
        }

        fn produce(
            &self,
            ctx: &ProducerContext<'_>,
            sink: &mut dyn SemanticFactSink,
        ) -> Result<(), FactSourceError> {
            assert!(ctx.repo_root.ends_with("fixture"));
            sink.push_file(FileFact {
                id: "file:src/lib.rs".into(),
                path: "src/lib.rs".into(),
                language: Some("rust".into()),
                digest: None,
                generated: false,
                properties: JsonMap::new(),
            })?;
            sink.push_symbol(SymbolFact {
                id: "sym:main".into(),
                symbol: "rust src/lib.rs/main().".into(),
                name: "main".into(),
                qualified_name: Some("fixture::main".into()),
                kind: SymbolKind::Function,
                file_path: Some("src/lib.rs".into()),
                range: None,
                documentation: None,
                properties: JsonMap::new(),
            })?;
            sink.push_occurrence(OccurrenceFact {
                id: "occ:def:main".into(),
                symbol_id: "sym:main".into(),
                file_id: "file:src/lib.rs".into(),
                range: TextRange {
                    start_line_0based: 0,
                    end_line_0based: 2,
                    start_col_0based: Some(0),
                    end_col_0based: Some(1),
                },
                role: OccurrenceRole::Definition,
                syntax_kind: Some("function_item".into()),
                properties: JsonMap::new(),
            })?;
            sink.push_relation(RelationFact {
                id: "rel:defines:main".into(),
                source: "sym:main".into(),
                target: "file:src/lib.rs".into(),
                kind: RelationKind::Defines,
                confidence: 1.0,
                reason: Some("scip occurrence".into()),
                step: None,
                evidence: Some(serde_json::json!({ "source": "scip" })),
            })?;
            Ok(())
        }
    }

    #[test]
    fn semantic_producer_lowers_without_artifact_transport() {
        let repo = Path::new("/tmp/fixture");
        let ctx = ProducerContext {
            repo_root: repo,
            no_chunks: false,
        };

        let batch = produce_semantic_batch(&[&FakeScipProducer], &ctx).unwrap();
        let nodes: Vec<_> = batch.nodes().unwrap().map(Result::unwrap).collect();
        let edges: Vec<_> = batch.edges().unwrap().map(Result::unwrap).collect();

        assert_eq!(batch.stats.files, 1);
        assert_eq!(batch.stats.nodes, 2);
        assert_eq!(batch.stats.edges, 1);
        assert_eq!(nodes[1].label, "Function");
        assert_eq!(nodes[1].start_line(), Some(0));
        assert_eq!(edges[0].edge_type, "DEFINES");
        assert_eq!(
            edges[0]
                .evidence
                .as_ref()
                .and_then(|value| value.get("source")),
            Some(&Value::String("scip".into()))
        );
    }

    #[test]
    fn semantic_producer_replays_into_fact_sink() {
        let repo = Path::new("/tmp/fixture");
        let ctx = ProducerContext {
            repo_root: repo,
            no_chunks: true,
        };
        let mut sink = FactBatchBuilder::new();

        produce_semantic_into(&[&FakeScipProducer], &ctx, &mut sink).unwrap();
        let batch = sink.finish();

        assert_eq!(batch.stats.nodes, 2);
        assert_eq!(batch.edges.len(), 1);
    }

    #[test]
    fn semantic_bundle_replay_preserves_chunks() {
        let bundle = SemanticFactBundle {
            chunks: vec![ChunkFact {
                node_id: "sym:main".into(),
                kind: "ast-function".into(),
                file_path: "src/lib.rs".into(),
                start_line: 0,
                end_line: 0,
                text: "fn main() {}".into(),
            }],
            ..SemanticFactBundle::default()
        };
        let mut sink = FactBatchBuilder::new();

        replay_semantic_bundle_into(bundle, &mut sink).unwrap();
        let batch = sink.finish();

        assert_eq!(batch.chunks.len(), 1);
        assert_eq!(batch.stats.chunks, 1);
    }
}
