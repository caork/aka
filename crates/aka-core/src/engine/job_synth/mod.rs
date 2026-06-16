use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use super::{
    process_ids_for_entry, project_code_nodes_by_file, read_repo_text, stable_hash,
    ProjectSourceSet, SynthNode, SynthProcess,
};

mod batch;
mod config;
mod detect;
mod trigger;
mod types;

use batch::attach_spring_batch_step_refs;
use config::synthesize_config_jobs;
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
                    handler_id: Some(node.aka_id.clone()),
                    handler_name: Some(node.display_name().to_string()),
                    source_config_id: None,
                    strategy: detection.strategy,
                    process_ids: process_ids_for_entry(processes, &file_path, Some(&node.aka_id)),
                    triggers: Vec::new(),
                    step_refs: Vec::new(),
                });
            }
        }
    }
    for file_path in project_sources
        .project_files(repo)
        .filter(|file_path| is_job_config_file_path(file_path))
    {
        let Some(text) = read_repo_text(repo, file_path) else {
            continue;
        };
        for job in synthesize_config_jobs(file_path, &text) {
            let key = format!("{}|{}|{}", job.id, job.file_path, job.strategy);
            if seen.insert(key) {
                out.push(job);
            }
        }
    }
    attach_spring_batch_step_refs(repo, nodes, &mut out);
    attach_job_triggers(repo, nodes, &mut out);
    out.sort_by(|a, b| {
        a.job_type
            .cmp(&b.job_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.handler_id.cmp(&b.handler_id))
    });
    out
}

fn is_job_config_file_path(file_path: &str) -> bool {
    let lower = file_path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    matches!(
        name,
        "application.yml"
            | "application.yaml"
            | "application.properties"
            | "bootstrap.yml"
            | "bootstrap.yaml"
            | "bootstrap.properties"
            | "settings.py"
            | "config.py"
            | ".env"
    ) || name.starts_with("application-") && name.ends_with(".yml")
        || name.starts_with("application-") && name.ends_with(".yaml")
        || name.starts_with("application-") && name.ends_with(".properties")
}

pub(super) fn job_entry_hints_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> BTreeMap<String, String> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut out = BTreeMap::new();
    for (file_path, file_nodes) in by_file {
        let text = read_repo_text(repo, &file_path);
        for node in file_nodes
            .iter()
            .copied()
            .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
        {
            for detection in detect_node_jobs(text.as_deref(), node) {
                if detection.job_type == "spring-scheduled" {
                    out.entry(node.aka_id.clone()).or_insert(detection.strategy);
                }
            }
        }
    }
    out
}
