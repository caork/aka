use super::{infra_config, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn extract_analytics_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_analytics_context(text) {
        out.extend(extract_python_analytics(text, nodes));
    }
    if has_java_analytics_context(text) {
        out.extend(extract_java_analytics(text, nodes));
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

pub(super) fn extract_analytics_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some(provider) = analytics_provider_for_config_key(&key) else {
            continue;
        };
        if !analytics_config_value_is_present(&value) {
            continue;
        }
        out.push(ResourceDetection::analytics(
            provider.into(),
            infra_config::config_id(&key),
            analytics_config_strategy(provider),
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

fn has_python_analytics_context(text: &str) -> bool {
    text.contains("analytics-python")
        || text.contains("import analytics")
        || text.contains("amplitude")
        || text.contains("Amplitude(")
        || text.contains("mixpanel")
        || text.contains("Mixpanel(")
        || text.contains("posthog")
        || text.contains("Posthog(")
}

fn extract_python_analytics(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_python_segment_calls(text, nodes));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "amplitude",
        &["Amplitude("],
        &[
            (".track", "python-amplitude-track"),
            (".identify", "python-amplitude-identify"),
            (".group_identify", "python-amplitude-group-identify"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "mixpanel",
        &["Mixpanel("],
        &[
            (".track", "python-mixpanel-track"),
            (".people_set", "python-mixpanel-people-set"),
            (".import_data", "python-mixpanel-import-data"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "posthog",
        &["Posthog(", "PostHog("],
        &[
            (".capture", "python-posthog-capture"),
            (".identify", "python-posthog-identify"),
            (".group_identify", "python-posthog-group-identify"),
        ],
    ));
    out
}

fn extract_python_segment_calls(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    if !text.contains("import analytics") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (callee, strategy) in [
        ("analytics.track", "python-segment-track"),
        ("analytics.identify", "python-segment-identify"),
        ("analytics.group", "python-segment-group"),
    ] {
        for call in find_call_args(text, callee) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::analytics(
                "segment".into(),
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out
}

fn extract_python_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    constructors: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for call in find_call_args(text, callee) {
            if !python_call_is_provider(text, call.start, provider, constructors) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::analytics(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn has_java_analytics_context(text: &str) -> bool {
    text.contains("AnalyticsClient")
        || text.contains("com.segment.analytics")
        || text.contains("Amplitude")
        || text.contains("MixpanelAPI")
        || text.contains("PostHog")
}

fn extract_java_analytics(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "segment",
        &["Analytics", "AnalyticsClient"],
        &[
            (".enqueue", "java-segment-enqueue"),
            (".track", "java-segment-track"),
            (".identify", "java-segment-identify"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "amplitude",
        &["Amplitude", "AmplitudeClient"],
        &[
            (".track", "java-amplitude-track"),
            (".identify", "java-amplitude-identify"),
            (".logEvent", "java-amplitude-log-event"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "mixpanel",
        &["MixpanelAPI", "MixpanelClient"],
        &[
            (".track", "java-mixpanel-track"),
            (".peopleSet", "java-mixpanel-people-set"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "posthog",
        &["PostHog", "Posthog"],
        &[
            (".capture", "java-posthog-capture"),
            (".identify", "java-posthog-identify"),
        ],
    ));
    out
}

fn extract_java_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    types: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for call in find_call_args(text, callee) {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !java_receiver_has_type(text, receiver, types) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::analytics(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn python_call_is_provider(
    text: &str,
    dot_start: usize,
    provider: &str,
    constructors: &[&str],
) -> bool {
    let Some(receiver) = receiver_before_dot(text, dot_start) else {
        return false;
    };
    let receiver = receiver_tail(receiver);
    if provider == "segment" {
        return receiver == "analytics" && text.contains("import analytics");
    }
    receiver.to_ascii_lowercase().contains(provider)
        || python_receiver_assigned_to(text, receiver, constructors)
}

fn python_receiver_assigned_to(text: &str, receiver: &str, constructors: &[&str]) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            return false;
        };
        let lhs = lhs
            .trim()
            .trim_start_matches("self.")
            .trim_start_matches("cls.");
        lhs == receiver && constructors.iter().any(|ctor| rhs.contains(ctor))
    })
}

fn java_receiver_has_type(text: &str, receiver: &str, types: &[&str]) -> bool {
    let receiver = receiver_tail(receiver);
    text.lines().any(|line| {
        let line = line.trim();
        types.iter().any(|ty| {
            line.contains(&format!("{ty} {receiver}"))
                || line.contains(&format!("{ty} {receiver},"))
                || line.contains(&format!("{ty} {receiver})"))
                || line.contains(&format!("{ty} {receiver} ="))
        })
    })
}

fn receiver_before_dot(text: &str, dot_start: usize) -> Option<&str> {
    if text.as_bytes().get(dot_start) != Some(&b'.') {
        return None;
    }
    let mut start = dot_start;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if ch == '.' || ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let receiver = text[start..dot_start].trim_matches('.');
    (!receiver.is_empty()).then_some(receiver)
}

fn receiver_tail(receiver: &str) -> &str {
    receiver.rsplit('.').next().unwrap_or(receiver)
}

fn analytics_provider_for_config_key(key: &str) -> Option<&'static str> {
    if key_contains_any(key, &["segment"]) {
        Some("segment")
    } else if key_contains_any(key, &["amplitude"]) {
        Some("amplitude")
    } else if key_contains_any(key, &["mixpanel"]) {
        Some("mixpanel")
    } else if key_contains_any(key, &["posthog", "post.hog"]) {
        Some("posthog")
    } else {
        None
    }
}

fn analytics_config_value_is_present(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '`']);
    !value.is_empty()
        && !value.starts_with("${")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "none" | "null" | "0"
        )
}

fn analytics_config_strategy(provider: &str) -> &'static str {
    match provider {
        "segment" => "segment-config",
        "amplitude" => "amplitude-config",
        "mixpanel" => "mixpanel-config",
        "posthog" => "posthog-config",
        _ => "analytics-config",
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
