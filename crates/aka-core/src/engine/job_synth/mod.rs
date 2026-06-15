use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use super::{
    find_matching_paren, process_ids_for_entry, project_code_nodes_by_file, read_repo_text,
    stable_hash, ProjectSourceSet, SynthNode, SynthProcess,
};

mod detect;
mod trigger;
mod types;

use detect::detect_node_jobs;
use trigger::attach_job_triggers;
pub(super) use types::{SynthJob, SynthJobStepRef};

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
                    step_refs: Vec::new(),
                });
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

fn attach_spring_batch_step_refs(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    jobs: &mut [SynthJob],
) {
    let step_jobs: BTreeMap<String, (String, String)> = jobs
        .iter()
        .filter(|job| job.job_type == "spring-batch-step")
        .flat_map(|job| {
            [
                (
                    job.handler_name
                        .rsplit('.')
                        .next()
                        .unwrap_or(&job.handler_name)
                        .to_string(),
                    (job.handler_id.clone(), job.name.clone()),
                ),
                (job.name.clone(), (job.handler_id.clone(), job.name.clone())),
            ]
        })
        .collect();
    if step_jobs.is_empty() {
        return;
    }
    let nodes_by_id: BTreeMap<_, _> = nodes.iter().map(|(id, node)| (id.as_str(), node)).collect();
    for job in jobs
        .iter_mut()
        .filter(|job| job.job_type == "spring-batch-job")
    {
        let Some(handler) = nodes_by_id.get(job.handler_id.as_str()) else {
            continue;
        };
        let Some(text) = read_repo_text(repo, &handler.file_path) else {
            continue;
        };
        for step_name in spring_batch_step_names_in_job(&text, handler) {
            let Some((node_id, display_name)) = step_jobs.get(&step_name) else {
                continue;
            };
            job.step_refs.push(SynthJobStepRef {
                node_id: node_id.clone(),
                step_name: display_name.clone(),
                strategy: "java-spring-batch-step-ref".into(),
            });
        }
        job.step_refs.sort();
        job.step_refs.dedup();
    }
}

fn spring_batch_step_names_in_job(text: &str, handler: &SynthNode) -> Vec<String> {
    let body = node_source_window(text, handler);
    let mut out = Vec::new();
    for method in ["start", "next", "flow"] {
        for args in java_chain_call_args(&body, method) {
            if let Some(name) = first_java_reference_name(args) {
                out.push(name);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn java_chain_call_args<'a>(text: &'a str, method: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let needle = format!(".{method}");
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find(&needle) {
        let start = offset + rel;
        let name_end = start + needle.len();
        if text
            .as_bytes()
            .get(name_end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
        {
            offset = name_end;
            continue;
        }
        let open = text[name_end..].char_indices().find_map(|(idx, ch)| {
            if ch.is_whitespace() {
                None
            } else {
                (ch == '(').then_some(name_end + idx)
            }
        });
        let Some(open) = open else {
            offset = name_end;
            continue;
        };
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        out.push(&text[open + 1..close]);
        offset = close + 1;
    }
    out
}

fn node_source_window(text: &str, node: &SynthNode) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start_line = node.start_line_key().max(1);
    let end_line = node.end_line_key().max(start_line);
    let start_idx = start_line.saturating_sub(1) as usize;
    let end_idx = end_line.min(lines.len() as i64) as usize;
    lines
        .get(start_idx..end_idx)
        .map(|slice| slice.join("\n"))
        .unwrap_or_default()
}

fn first_java_reference_name(args: &str) -> Option<String> {
    let raw = args
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches("this.")
        .trim_start_matches("() ->")
        .trim();
    let name = raw
        .split(|ch: char| !(ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()))
        .find(|part| !part.is_empty())?;
    Some(name.to_string())
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
