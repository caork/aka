use std::collections::BTreeSet;

use super::{find_call_args, read_string_literal, skip_ws, split_top_level_commas};

pub(super) fn extract_java_map_response_keys(text: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for call in find_call_args(text, "Map.of") {
        keys.extend(java_map_of_keys(call.args));
    }
    for call in find_call_args(text, "Map.ofEntries") {
        keys.extend(java_map_of_entries_keys(call.args));
    }
    keys.into_iter().collect()
}

fn java_map_of_keys(args: &str) -> Vec<String> {
    split_top_level_commas(args)
        .into_iter()
        .step_by(2)
        .filter_map(java_string_arg)
        .filter(|key| is_object_key(key))
        .collect()
}

fn java_map_of_entries_keys(args: &str) -> Vec<String> {
    split_top_level_commas(args)
        .into_iter()
        .filter_map(|entry| {
            let trimmed = entry.trim();
            for callee in ["Map.entry", "entry"] {
                for call in find_call_args(trimmed, callee) {
                    let first = split_top_level_commas(call.args).into_iter().next()?;
                    if let Some(key) = java_string_arg(first) {
                        return Some(key);
                    }
                }
            }
            None
        })
        .filter(|key| is_object_key(key))
        .collect()
}

fn java_string_arg(arg: &str) -> Option<String> {
    let trimmed = arg.trim();
    let start = skip_ws(trimmed, 0);
    read_string_literal(trimmed, start)
        .and_then(|(literal, end)| (skip_ws(trimmed, end) == trimmed.len()).then_some(literal))
}

fn is_object_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '$'))
}
