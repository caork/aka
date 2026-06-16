use std::collections::BTreeMap;
use std::path::Path;

use super::super::{find_matching_paren, read_repo_text, SynthNode};
use super::types::{SynthJob, SynthJobStepRef};

pub(super) fn attach_spring_batch_step_refs(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    jobs: &mut [SynthJob],
) {
    let step_jobs: BTreeMap<String, (String, String)> = jobs
        .iter()
        .filter(|job| job.job_type == "spring-batch-step")
        .filter_map(|job| Some((job, job.handler_id.as_ref()?, job.handler_name.as_ref()?)))
        .flat_map(|job| {
            let (job, handler_id, handler_name) = job;
            [
                (
                    handler_name
                        .rsplit('.')
                        .next()
                        .unwrap_or(handler_name)
                        .to_string(),
                    (handler_id.clone(), job.name.clone()),
                ),
                (job.name.clone(), (handler_id.clone(), job.name.clone())),
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
        let Some(handler_id) = &job.handler_id else {
            continue;
        };
        let Some(handler) = nodes_by_id.get(handler_id.as_str()) else {
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
    for method in ["start", "next", "flow", "from", "to"] {
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
