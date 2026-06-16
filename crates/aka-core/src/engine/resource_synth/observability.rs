use super::{infra_config, ResourceDetection};

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
