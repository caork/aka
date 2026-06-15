use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use super::super::{
    find_call_args, node_at_offset, project_code_nodes_by_file, read_repo_text,
    split_top_level_commas, ProjectSourceSet, SynthNode,
};
use super::detect::{
    first_callable_name, first_string_literal, is_background_task_receiver, is_python_identifier,
};
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
    let spring_batch_jobs: BTreeMap<String, usize> = jobs
        .iter()
        .enumerate()
        .filter(|(_, job)| job.job_type == "spring-batch-job")
        .flat_map(|(idx, job)| {
            [
                (
                    job.handler_name
                        .rsplit('.')
                        .next()
                        .unwrap_or(&job.handler_name)
                        .to_string(),
                    idx,
                ),
                (job.name.clone(), idx),
            ]
        })
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
        if job.job_type == "fastapi-background-task" {
            named_jobs.insert(job.handler_id.clone(), idx);
        }
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
            .chain(detect_spring_batch_job_launcher_calls(
                &text,
                &file_path,
                &file_nodes,
                &spring_batch_jobs,
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

fn detect_spring_batch_job_launcher_calls(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
    batch_jobs: &BTreeMap<String, usize>,
) -> Vec<JobTriggerDetection> {
    let mut out = Vec::new();
    if batch_jobs.is_empty() {
        return out;
    }
    for call in find_call_args(text, ".run") {
        let Some(source) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        let args = split_top_level_commas(call.args);
        let Some(job_name) = args.first().and_then(|arg| first_callable_name(arg)) else {
            continue;
        };
        if let Some(job_index) = batch_jobs.get(&job_name) {
            out.push(JobTriggerDetection {
                job_index: *job_index,
                node_id: source.aka_id.clone(),
                file_path: file_path.to_string(),
                strategy: "java-spring-batch-job-launcher-run".into(),
            });
        }
    }
    out
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
        (".enqueue_call", "python-rq-enqueue-call"),
        (".enqueue_in", "python-rq-enqueue-in"),
        (".enqueue_at", "python-rq-enqueue-at"),
        (".send", "python-dramatiq-send"),
        (".send_with_options", "python-dramatiq-send-with-options"),
        (".schedule", "python-huey-schedule"),
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
    out.extend(detect_python_background_task_dispatches(
        text, file_path, nodes, named_jobs,
    ));
    out
}

fn detect_python_background_task_dispatches(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
    named_jobs: &BTreeMap<String, usize>,
) -> Vec<JobTriggerDetection> {
    let mut out = Vec::new();
    for callee in ["background_tasks.add_task", ".add_task"] {
        for call in find_call_args(text, callee) {
            if callee == ".add_task" && !is_background_task_receiver(text, call.start) {
                continue;
            }
            let Some(source) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            let args = split_top_level_commas(call.args);
            let Some(name) = args.first().and_then(|arg| first_callable_name(arg)) else {
                continue;
            };
            if let Some(job_index) = named_jobs.get(&name) {
                out.push(JobTriggerDetection {
                    job_index: *job_index,
                    node_id: source.aka_id.clone(),
                    file_path: file_path.to_string(),
                    strategy: "python-fastapi-background-tasks-add-task".into(),
                });
            }
        }
    }
    out
}

fn dispatch_names_for_call(text: &str, call_start: usize, args: &str, callee: &str) -> Vec<String> {
    let mut out = Vec::new();
    if matches!(
        callee,
        ".delay" | ".apply_async" | ".send" | ".send_with_options" | ".schedule"
    ) {
        if let Some(receiver) = receiver_ident_before(text, call_start) {
            out.push(receiver);
        }
    } else if matches!(
        callee,
        ".enqueue" | ".enqueue_call" | ".enqueue_in" | ".enqueue_at"
    ) {
        let args = split_top_level_commas(args);
        if let Some(name) = rq_callable_arg_name(&args, callee) {
            out.push(name);
        }
    }
    out
}

fn rq_callable_arg_name(args: &[&str], callee: &str) -> Option<String> {
    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            if key.trim() == "func" {
                return first_callable_name(value);
            }
        }
    }
    let positional_index = if matches!(callee, ".enqueue_in" | ".enqueue_at") {
        1
    } else {
        0
    };
    args.get(positional_index)
        .and_then(|arg| first_callable_name(arg))
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
