use super::{infra_config, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn extract_observability_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_observability_context(text) {
        out.extend(extract_python_observability_resources(text, nodes));
    }
    if has_java_observability_context(text) {
        out.extend(extract_java_observability_resources(text, nodes));
    }
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

pub(super) fn extract_observability_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some(platform) = observability_platform_for_config_key(&key) else {
            continue;
        };
        if !observability_config_value_is_present(&value) {
            continue;
        }
        out.push(ResourceDetection::observability(
            platform.into(),
            infra_config::config_id(&key),
            observability_config_strategy(platform),
        ));
    }
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn has_python_observability_context(text: &str) -> bool {
    text.contains("sentry_sdk")
        || text.contains("opentelemetry")
        || text.contains("trace.get_tracer")
}

fn extract_python_observability_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if text.contains("sentry_sdk") {
        for (callee, strategy) in [
            (
                "sentry_sdk.capture_exception",
                "python-sentry-capture-exception",
            ),
            (
                "sentry_sdk.capture_message",
                "python-sentry-capture-message",
            ),
            ("sentry_sdk.start_transaction", "python-sentry-transaction"),
        ] {
            for call in find_call_args(text, callee) {
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::observability(
                    "sentry".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    if text.contains("opentelemetry") || text.contains("trace.get_tracer") {
        for (callee, strategy) in [
            (
                ".start_as_current_span",
                "python-opentelemetry-current-span",
            ),
            (".start_span", "python-opentelemetry-span"),
        ] {
            for call in find_call_args(text, callee) {
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::observability(
                    "opentelemetry".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    out
}

fn has_java_observability_context(text: &str) -> bool {
    text.contains("io.sentry")
        || text.contains("Sentry.")
        || text.contains("io.opentelemetry")
        || text.contains("OpenTelemetry")
        || text.contains("GlobalOpenTelemetry")
}

fn extract_java_observability_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if text.contains("io.sentry") || text.contains("Sentry.") {
        for (callee, strategy) in [
            ("Sentry.captureException", "java-sentry-capture-exception"),
            ("Sentry.captureMessage", "java-sentry-capture-message"),
            ("Sentry.startTransaction", "java-sentry-transaction"),
        ] {
            for call in find_call_args(text, callee) {
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::observability(
                    "sentry".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    if text.contains("io.opentelemetry")
        || text.contains("OpenTelemetry")
        || text.contains("GlobalOpenTelemetry")
    {
        for call in find_call_args(text, "GlobalOpenTelemetry.getTracer") {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::observability(
                "opentelemetry".into(),
                node.aka_id.clone(),
                "java-opentelemetry-tracer",
            ));
            let window_end = (call.start + call.args.len() + 500).min(text.len());
            let window = &text[call.start..window_end];
            for (needle, strategy) in [
                (".spanBuilder(", "java-opentelemetry-span-builder"),
                (".startSpan(", "java-opentelemetry-start-span"),
            ] {
                if window.contains(needle) {
                    out.push(ResourceDetection::observability(
                        "opentelemetry".into(),
                        node.aka_id.clone(),
                        strategy,
                    ));
                }
            }
        }
        for (callee, strategy) in [
            (".spanBuilder", "java-opentelemetry-span-builder"),
            (".startSpan", "java-opentelemetry-start-span"),
        ] {
            for call in find_call_args(text, callee) {
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::observability(
                    "opentelemetry".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    out
}

fn observability_platform_for_config_key(key: &str) -> Option<&'static str> {
    if key_contains_any(key, &["sentry"]) {
        Some("sentry")
    } else if key_contains_any(key, &["datadog", "dd"]) {
        Some("datadog")
    } else if key_contains_any(key, &["newrelic", "new.relic"]) {
        Some("newrelic")
    } else if key_contains_any(key, &["opentelemetry", "otel"]) {
        Some("opentelemetry")
    } else if key_contains_any(key, &["prometheus"]) {
        Some("prometheus")
    } else {
        None
    }
}

fn observability_config_value_is_present(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '`']);
    !value.is_empty()
        && !value.starts_with("${")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "none" | "null" | "0"
        )
}

fn observability_config_strategy(platform: &str) -> &'static str {
    match platform {
        "sentry" => "sentry-config",
        "datadog" => "datadog-config",
        "newrelic" => "newrelic-config",
        "opentelemetry" => "opentelemetry-config",
        "prometheus" => "prometheus-config",
        _ => "observability-config",
    }
}

fn key_contains_any(key: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        if needle.contains('.') && key.contains(needle) {
            return true;
        }
        key.split('.')
            .any(|part| part == *needle || part.contains(needle))
    })
}
