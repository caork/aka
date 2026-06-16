use super::ResourceDetection;
use crate::engine::SynthNode;

mod java;
mod python;

use java::{extract_java_notifications, has_java_notification_context};
use python::{extract_python_notifications, has_python_notification_context};

pub(super) fn extract_notification_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_notification_context(text) {
        out.extend(extract_python_notifications(text, nodes));
    }
    if has_java_notification_context(text) {
        out.extend(extract_java_notifications(text, nodes));
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then_with(|| a.node_id.cmp(&b.node_id)));
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}
