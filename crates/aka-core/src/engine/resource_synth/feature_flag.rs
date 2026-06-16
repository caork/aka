use super::{infra_config, read_string_literal, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, split_top_level_commas, SynthNode};

pub(super) fn extract_feature_flag_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_feature_flag_context(text) {
        out.extend(extract_python_feature_flags(text, nodes));
    }
    if has_java_feature_flag_context(text) {
        out.extend(extract_java_feature_flags(text, nodes));
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then_with(|| a.node_id.cmp(&b.node_id)));
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

pub(super) fn extract_feature_flag_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        if !is_feature_flag_config_key(&key) {
            continue;
        }
        for flag in feature_flags_from_config_value(&value) {
            out.push(ResourceDetection::feature_flag(
                flag,
                infra_config::config_id(&key),
                feature_flag_config_strategy(&key),
            ));
        }
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

fn has_python_feature_flag_context(text: &str) -> bool {
    text.contains("ldclient")
        || text.contains("launchdarkly")
        || text.contains("UnleashClient")
        || text.contains("featureflags")
        || text.contains("waffle")
}

fn extract_python_feature_flags(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in [
        (".variation", "python-launchdarkly-variation"),
        (".is_enabled", "python-unleash-enabled"),
        (".is_enabled_async", "python-unleash-enabled"),
        ("flag_is_active", "python-django-waffle"),
    ] {
        for call in find_call_args(text, callee) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            let Some(flag) = flag_literal_from_args(call.args) else {
                continue;
            };
            out.push(ResourceDetection::feature_flag(
                flag,
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out
}

fn has_java_feature_flag_context(text: &str) -> bool {
    text.contains("LDClient")
        || text.contains("LDUser")
        || text.contains("launchdarkly")
        || text.contains("Unleash")
        || text.contains("FeatureManager")
        || text.contains("org.togglz")
}

fn extract_java_feature_flags(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in [
        (".boolVariation", "java-launchdarkly-variation"),
        (".stringVariation", "java-launchdarkly-variation"),
        (".intVariation", "java-launchdarkly-variation"),
        (".doubleVariation", "java-launchdarkly-variation"),
        (".jsonValueVariation", "java-launchdarkly-variation"),
        (".isEnabled", "java-unleash-enabled"),
        (".isActive", "java-togglz-active"),
    ] {
        for call in find_call_args(text, callee) {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            let Some(flag) = flag_literal_from_args(call.args) else {
                continue;
            };
            out.push(ResourceDetection::feature_flag(
                flag,
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out
}

fn flag_literal_from_args(args: &str) -> Option<String> {
    split_top_level_commas(args)
        .into_iter()
        .find_map(first_flag_literal)
}

fn first_flag_literal(arg: &str) -> Option<String> {
    let trimmed = arg.trim();
    let start = trimmed.find(['"', '\''])?;
    let (literal, _) = read_string_literal(trimmed, start)?;
    is_feature_flag_literal(&literal).then_some(literal)
}

fn is_feature_flag_literal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.contains("://")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/'))
}

fn is_feature_flag_config_key(key: &str) -> bool {
    let parts = key.split('.').collect::<Vec<_>>();
    let contains = |needle: &str| {
        parts
            .iter()
            .any(|part| *part == needle || part.contains(needle))
    };
    (contains("feature") && (contains("flag") || contains("toggle")))
        || contains("launchdarkly")
        || contains("unleash")
        || contains("splitio")
        || contains("split")
        || contains("togglz")
        || contains("waffle")
        || key.ends_with(".flags")
        || key.ends_with(".flag")
        || key.ends_with(".toggles")
        || key.ends_with(".toggle")
}

fn feature_flags_from_config_value(value: &str) -> Vec<String> {
    let cleaned = value
        .trim()
        .trim_matches(['"', '\'', '`'])
        .trim_matches(['[', ']'])
        .trim();
    if cleaned.is_empty() || cleaned.starts_with("${") || cleaned.eq_ignore_ascii_case("true") {
        return Vec::new();
    }
    cleaned
        .split(',')
        .map(|part| part.trim().trim_matches(['"', '\'', '`']))
        .filter(|part| is_feature_flag_literal(part))
        .map(ToOwned::to_owned)
        .collect()
}

fn feature_flag_config_strategy(key: &str) -> &'static str {
    if key.contains("launchdarkly") {
        "launchdarkly-config-flag"
    } else if key.contains("unleash") {
        "unleash-config-flag"
    } else if key.contains("split") {
        "split-config-flag"
    } else if key.contains("togglz") {
        "togglz-config-flag"
    } else if key.contains("waffle") {
        "waffle-config-flag"
    } else {
        "feature-flag-config"
    }
}
