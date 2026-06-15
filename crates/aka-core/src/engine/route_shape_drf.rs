use std::collections::BTreeSet;

use super::route_shape_python::{python_model_name, PythonModelFields};

pub(super) fn extract_drf_serializer_keys_from_models(
    text: &str,
    models: &PythonModelFields,
) -> Vec<String> {
    let mut keys = BTreeSet::new();
    for serializer in drf_serializer_class_names(text) {
        if let Some(fields) = models.get(&serializer) {
            keys.extend(fields.iter().cloned());
        }
    }
    keys.into_iter().collect()
}

fn drf_serializer_class_names(text: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some((left, right)) = trimmed.split_once('=') else {
            continue;
        };
        if left.trim() != "serializer_class" {
            continue;
        }
        let name = right.trim().split('#').next().unwrap_or("").trim();
        if let Some(simple) = python_model_name(name) {
            out.insert(simple);
        }
    }
    out.into_iter().collect()
}
