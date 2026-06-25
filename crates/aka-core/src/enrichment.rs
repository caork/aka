//! Optional post-baseline enrichment guards.
//!
//! Baseline engine facts, graph, and search must be usable without this module.
//! Future providers should wrap mature OSS language services and must report
//! skipped/timeout outcomes instead of failing the indexing job.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::settings::AkaSettings;
use crate::types::{EngineEvent, PipelineProgress, PipelineStage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LspEnrichmentPolicy {
    pub enabled: bool,
    pub max_duration: Duration,
}

impl LspEnrichmentPolicy {
    pub fn from_settings(settings: AkaSettings) -> Self {
        Self {
            enabled: settings.lsp_enrichment_enabled,
            max_duration: Duration::from_secs(settings.lsp_enrichment_max_secs),
        }
    }
}

impl Default for LspEnrichmentPolicy {
    fn default() -> Self {
        Self::from_settings(AkaSettings::default())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LspEnrichmentOutcome {
    Disabled,
    NoProviders,
}

pub fn run_optional_lsp_enrichment(
    repo: &Path,
    policy: LspEnrichmentPolicy,
    mut on_event: impl FnMut(&EngineEvent),
) -> LspEnrichmentOutcome {
    if !policy.enabled {
        emit_skipped(
            &mut on_event,
            format!("LSP enrichment disabled for {}", repo.display()),
            "skipped enabled=false reason=disabled",
        );
        return LspEnrichmentOutcome::Disabled;
    }

    emit_skipped(
        &mut on_event,
        format!(
            "LSP enrichment skipped for {}: no providers installed",
            repo.display()
        ),
        format!(
            "skipped enabled=true providers=0 max_secs={} reason=no_providers",
            policy.max_duration.as_secs()
        ),
    );
    LspEnrichmentOutcome::NoProviders
}

fn emit_skipped(on_event: &mut impl FnMut(&EngineEvent), message: String, line: impl Into<String>) {
    on_event(&EngineEvent::Progress {
        progress: PipelineProgress::new(PipelineStage::LspEnrichment, message),
    });
    on_event(&EngineEvent::Log {
        stream: "lsp-enrichment".into(),
        line: line.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_lsp_enrichment_reports_skip_without_error() {
        let mut events = Vec::new();
        let outcome = run_optional_lsp_enrichment(
            Path::new("/repo"),
            LspEnrichmentPolicy::default(),
            |event| events.push(event.clone()),
        );

        assert_eq!(outcome, LspEnrichmentOutcome::Disabled);
        assert!(matches!(
            events.first(),
            Some(EngineEvent::Progress { progress })
                if progress.stage == PipelineStage::LspEnrichment
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Log { stream, line }
                if stream == "lsp-enrichment" && line.contains("reason=disabled")
        )));
    }

    #[test]
    fn enabled_lsp_enrichment_without_providers_is_skipped() {
        let mut events = Vec::new();
        let outcome = run_optional_lsp_enrichment(
            Path::new("/repo"),
            LspEnrichmentPolicy {
                enabled: true,
                max_duration: Duration::from_secs(15),
            },
            |event| events.push(event.clone()),
        );

        assert_eq!(outcome, LspEnrichmentOutcome::NoProviders);
        assert!(events.iter().any(|event| matches!(
            event,
            EngineEvent::Log { stream, line }
                if stream == "lsp-enrichment"
                    && line.contains("providers=0")
                    && line.contains("max_secs=15")
        )));
    }
}
