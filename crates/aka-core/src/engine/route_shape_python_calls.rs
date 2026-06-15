use std::collections::BTreeSet;

use super::route_shape::{balanced_brace_body, is_object_key, top_level_object_keys};
use super::{find_call_args, split_top_level_commas};

pub(super) fn extract_python_response_call_keys(text: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for callee in ["JsonResponse", "jsonify", "Response"] {
        for call in find_call_args(text, callee) {
            keys.extend(response_call_object_keys(call.args));
            keys.extend(response_call_keyword_keys(callee, call.args));
        }
    }
    keys.into_iter().collect()
}

fn response_call_object_keys(args: &str) -> Vec<String> {
    let first = split_top_level_commas(args)
        .into_iter()
        .map(str::trim)
        .find(|arg| !arg.is_empty())
        .unwrap_or("");
    if first.as_bytes().first() != Some(&b'{') {
        return Vec::new();
    }
    balanced_brace_body(first, 0)
        .map(top_level_object_keys)
        .unwrap_or_default()
}

fn response_call_keyword_keys(callee: &str, args: &str) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for arg in split_top_level_commas(args) {
        let Some((name, _)) = arg.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if is_object_key(name) && !is_common_response_call_keyword(callee, name) {
            keys.insert(name.to_string());
        }
    }
    keys.into_iter().collect()
}

fn is_common_response_call_keyword(callee: &str, name: &str) -> bool {
    match callee {
        "jsonify" => false,
        "JsonResponse" => matches!(name, "safe" | "status" | "json_dumps_params"),
        "Response" => matches!(
            name,
            "status" | "reason" | "headers" | "content_type" | "mimetype"
        ),
        _ => false,
    }
}
