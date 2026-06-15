use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use super::{
    process_ids_for_entry, project_code_nodes_by_file, read_repo_text, stable_hash,
    ProjectSourceSet, SynthNode, SynthProcess,
};

mod detect;
mod trigger;
mod types;

use detect::detect_node_jobs;
use trigger::attach_job_triggers;
pub(super) use types::SynthJob;

pub(super) fn synthesize_jobs_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    processes: &[SynthProcess],
) -> Vec<SynthJob> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let text = read_repo_text(repo, &file_path);
        for node in file_nodes
            .iter()
            .copied()
            .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
        {
            for detection in detect_node_jobs(text.as_deref(), node) {
                let key = format!(
                    "{}|{}|{}|{}",
                    node.aka_id,
                    detection.job_type,
                    detection.name,
                    detection.schedule.clone().unwrap_or_default()
                );
                if !seen.insert(key.clone()) {
                    continue;
                }
                out.push(SynthJob {
                    id: format!("job:heuristic:{:016x}", stable_hash(&key)),
                    name: detection.name,
                    job_type: detection.job_type,
                    schedule: detection.schedule,
                    file_path: file_path.clone(),
                    handler_id: node.aka_id.clone(),
                    handler_name: node.display_name().to_string(),
                    strategy: detection.strategy,
                    process_ids: process_ids_for_entry(processes, &file_path, Some(&node.aka_id)),
                    triggers: Vec::new(),
                });
            }
        }
    }
    attach_job_triggers(repo, nodes, &mut out);
    out.sort_by(|a, b| {
        a.job_type
            .cmp(&b.job_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.handler_id.cmp(&b.handler_id))
    });
    out
}
