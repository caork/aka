use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use super::super::{
    find_call_args, node_at_offset, project_code_nodes_by_file, read_repo_text,
    split_top_level_commas, ProjectSourceSet, SynthNode,
};
use super::detect::first_string_literal;
use super::types::{SynthJob, SynthJobTrigger};

pub(super) fn attach_job_triggers(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    jobs: &mut [SynthJob],
) {
    if jobs.is_empty() {
        return;
    }
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let async_jobs: Vec<(usize, String, String)> = jobs
        .iter()
        .enumerate()
        .filter(|(_, job)| job.job_type == "spring-async")
        .map(|(idx, job)| (idx, job.handler_id.clone(), job.handler_name.clone()))
        .collect();
    let mut named_jobs: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, job) in jobs.iter().enumerate() {
        named_jobs.insert(job.name.clone(), idx);
        named_jobs.insert(job.handler_name.clone(), idx);
        named_jobs.insert(
            job.handler_name
                .rsplit('.')
                .next()
                .unwrap_or(&job.handler_name)
                .to_string(),
            idx,
        );
    }
    let mut seen: HashSet<(usize, String, String)> = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for trigger in detect_async_handler_calls(&text, &file_path, &file_nodes, &async_jobs)
            .into_iter()
            .chain(detect_named_job_dispatches(
                &text,
                &file_path,
                &file_nodes,
                &named_jobs,
            ))
        {
            if !seen.insert((
                trigger.job_index,
                trigger.node_id.clone(),
                trigger.strategy.clone(),
            )) {
                continue;
            }
            jobs[trigger.job_index].triggers.push(SynthJobTrigger {
                node_id: trigger.node_id,
                file_path: trigger.file_path,
                strategy: trigger.strategy,
            });
        }
    }
    for job in jobs {
        job.triggers.sort();
        job.triggers.dedup();
    }
}

#[derive(Debug, Clone)]
struct JobTriggerDetection {
    job_index: usize,
    node_id: String,
    file_path: String,
    strategy: String,
}

fn detect_async_handler_calls(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
    jobs: &[(usize, String, String)],
) -> Vec<JobTriggerDetection> {
    let mut out = Vec::new();
    for (job_index, handler_id, handler_name) in jobs {
        let method = handler_name.rsplit('.').next().unwrap_or(handler_name);
        if method.is_empty() {
            continue;
        }
        for callee in [format!(".{method}"), method.to_string()] {
            for call in find_call_args(text, &callee) {
                let Some(source) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                if source.aka_id == *handler_id {
                    continue;
                }
                out.push(JobTriggerDetection {
                    job_index: *job_index,
                    node_id: source.aka_id.clone(),
                    file_path: file_path.to_string(),
                    strategy: "async-handler-call".into(),
                });
            }
        }
    }
    out
}

fn detect_named_job_dispatches(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
    named_jobs: &BTreeMap<String, usize>,
) -> Vec<JobTriggerDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in [
        (".delay", "python-celery-delay"),
        (".apply_async", "python-celery-apply-async"),
        (".enqueue", "python-rq-enqueue"),
        (".send", "python-dramatiq-send"),
        (".send_with_options", "python-dramatiq-send-with-options"),
    ] {
        for call in find_call_args(text, callee) {
            let Some(source) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            for name in dispatch_names_for_call(text, call.start, call.args, callee) {
                if let Some(job_index) = named_jobs.get(&name) {
                    out.push(JobTriggerDetection {
                        job_index: *job_index,
                        node_id: source.aka_id.clone(),
                        file_path: file_path.to_string(),
                        strategy: strategy.into(),
                    });
                }
            }
        }
    }
    for callee in ["send_task", ".send_task"] {
        for call in find_call_args(text, callee) {
            let Some(source) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            if let Some(name) = first_string_literal(call.args) {
                if let Some(job_index) = named_jobs.get(&name) {
                    out.push(JobTriggerDetection {
                        job_index: *job_index,
                        node_id: source.aka_id.clone(),
                        file_path: file_path.to_string(),
                        strategy: "python-celery-send-task".into(),
                    });
                }
            }
        }
    }
    out
}

fn dispatch_names_for_call(text: &str, call_start: usize, args: &str, callee: &str) -> Vec<String> {
    let mut out = Vec::new();
    if matches!(
        callee,
        ".delay" | ".apply_async" | ".send" | ".send_with_options"
    ) {
        if let Some(receiver) = receiver_ident_before(text, call_start) {
            out.push(receiver);
        }
    } else if callee == ".enqueue" {
        for arg in split_top_level_commas(args).into_iter().take(1) {
            if let Some(name) = first_callable_name(arg) {
                out.push(name);
            }
        }
    }
    out
}

fn receiver_ident_before(text: &str, dot_start: usize) -> Option<String> {
    let before = text.get(..dot_start)?;
    let mut end = before.len();
    while end > 0 && before.as_bytes()[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 {
        let ch = before[..start].chars().next_back()?;
        if ch == '_' || ch.is_ascii_alphanumeric() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let ident = before[start..end].trim();
    is_python_identifier(ident).then(|| ident.to_string())
}

fn first_callable_name(arg: &str) -> Option<String> {
    let trimmed = arg.trim();
    let name = trimmed
        .split_once('(')
        .map(|(name, _)| name)
        .unwrap_or(trimmed)
        .rsplit('.')
        .next()
        .unwrap_or(trimmed)
        .trim();
    is_python_identifier(name).then(|| name.to_string())
}

fn is_python_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
