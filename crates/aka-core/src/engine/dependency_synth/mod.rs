use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::json;

use super::{
    project_code_nodes_by_file, read_repo_text, stable_hash, EdgeRec, ProjectSourceSet, SynthNode,
};

mod java;
mod lookup;
mod python;

use java::detect_java_dependency_edges;
use lookup::NodeLookup;
use python::detect_python_dependency_edges;

pub(super) fn synthesize_dependency_edges_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    existing_call_pairs: &HashSet<(String, String)>,
    mut on_progress: impl FnMut(u64, u64),
) -> Vec<EdgeRec> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let total = by_file.len() as u64;
    let lookup = NodeLookup::new(by_file.values().flatten().copied());
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut processed = 0u64;
    on_progress(0, total);
    for (file_path, file_nodes) in by_file {
        processed += 1;
        let Some(text) = read_repo_text(repo, &file_path) else {
            if processed == total || processed % 25 == 0 {
                on_progress(processed, total);
            }
            continue;
        };
        out.extend(detect_java_dependency_edges(
            &text,
            &file_path,
            &file_nodes,
            &lookup,
            existing_call_pairs,
        ));
        out.extend(detect_python_dependency_edges(
            &text,
            &file_path,
            &file_nodes,
            &lookup,
            existing_call_pairs,
        ));
        if processed == total || processed % 25 == 0 {
            on_progress(processed, total);
        }
    }
    out.retain(|edge| {
        seen.insert((
            edge.source_id.clone(),
            edge.target_id.clone(),
            edge.edge_type.clone(),
            edge.reason.clone(),
        ))
    });
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn dependency_edge(
    source_id: &str,
    target_id: &str,
    kind: &str,
    strategy: &str,
    dependency: &str,
    confidence: f64,
) -> EdgeRec {
    EdgeRec {
        id: format!(
            "dependency:heuristic:{:016x}",
            stable_hash(&format!(
                "{source_id}|{target_id}|{kind}|{strategy}|{dependency}"
            ))
        ),
        source_id: source_id.to_string(),
        target_id: target_id.to_string(),
        edge_type: "DEPENDS_ON".into(),
        confidence,
        reason: "aka dependency synthesis".into(),
        step: None,
        evidence: Some(json!({
            "source": "aka-cbm-synth",
            "kind": kind,
            "strategy": strategy,
            "dependency": dependency,
        })),
    }
}
