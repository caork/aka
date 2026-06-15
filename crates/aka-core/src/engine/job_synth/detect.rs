use std::collections::BTreeSet;

use super::super::{
    find_call_args, find_matching_paren, read_string_literal, split_top_level_commas,
    string_literals, SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct JobDetection {
    pub(super) name: String,
    pub(super) job_type: String,
    pub(super) schedule: Option<String>,
    pub(super) strategy: String,
}

pub(super) fn detect_node_jobs(text: Option<&str>, node: &SynthNode) -> Vec<JobDetection> {
    let mut out = Vec::new();
    let lower_path = node.file_path.to_ascii_lowercase();
    if lower_path.ends_with(".java")
        || matches!(
            node.language.to_ascii_lowercase().as_str(),
            "java" | "kotlin" | "scala" | "groovy"
        )
    {
        out.extend(detect_jvm_jobs(node));
    }
    if lower_path.ends_with(".py") || node.language.eq_ignore_ascii_case("python") {
        out.extend(detect_python_jobs(text, node));
    }
    out.sort_by(|a, b| {
        a.job_type
            .cmp(&b.job_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.schedule.cmp(&b.schedule))
    });
    out
}

fn detect_jvm_jobs(node: &SynthNode) -> Vec<JobDetection> {
    let mut out = Vec::new();
    for decorator in &node.decorators {
        if decorator.contains("Scheduled") {
            let schedule = scheduled_annotation_schedule(decorator);
            out.push(JobDetection {
                name: schedule
                    .as_ref()
                    .map(|s| format!("{} ({s})", node.display_name()))
                    .unwrap_or_else(|| node.display_name().to_string()),
                job_type: "spring-scheduled".into(),
                schedule,
                strategy: "java-spring-scheduled".into(),
            });
            continue;
        }
        if decorator.contains("Async") {
            out.push(JobDetection {
                name: node.display_name().to_string(),
                job_type: "spring-async".into(),
                schedule: None,
                strategy: "java-spring-async".into(),
            });
        }
    }
    out
}

fn detect_python_jobs(text: Option<&str>, node: &SynthNode) -> Vec<JobDetection> {
    let mut out = Vec::new();
    for decorator in &node.decorators {
        let normalized = decorator.trim().trim_start_matches('@');
        if is_huey_task_decorator(normalized) {
            let schedule = normalized
                .contains("periodic_task")
                .then(|| python_schedule_summary(normalized))
                .flatten();
            out.push(JobDetection {
                name: python_named_arg(normalized, "name")
                    .or_else(|| schedule.clone())
                    .unwrap_or_else(|| node.display_name().to_string()),
                job_type: if normalized.contains("periodic_task") {
                    "huey-periodic-task".into()
                } else {
                    "huey-task".into()
                },
                schedule,
                strategy: "python-huey-task".into(),
            });
            continue;
        }
        if is_celery_task_decorator(normalized) {
            out.push(JobDetection {
                name: python_named_arg(normalized, "name")
                    .or_else(|| python_named_arg(normalized, "queue"))
                    .unwrap_or_else(|| node.display_name().to_string()),
                job_type: "celery-task".into(),
                schedule: None,
                strategy: "python-celery-task".into(),
            });
            continue;
        }
        if is_rq_job_decorator(normalized) {
            out.push(JobDetection {
                name: python_named_arg(normalized, "id")
                    .or_else(|| python_named_arg(normalized, "queue"))
                    .unwrap_or_else(|| node.display_name().to_string()),
                job_type: "rq-job".into(),
                schedule: None,
                strategy: "python-rq-job".into(),
            });
            continue;
        }
        if is_dramatiq_actor_decorator(normalized) {
            out.push(JobDetection {
                name: python_named_arg(normalized, "actor_name")
                    .or_else(|| python_named_arg(normalized, "queue_name"))
                    .unwrap_or_else(|| node.display_name().to_string()),
                job_type: "dramatiq-actor".into(),
                schedule: None,
                strategy: "python-dramatiq-actor".into(),
            });
            continue;
        }
        if normalized.contains("scheduled_job") {
            let schedule = python_schedule_summary(normalized);
            out.push(JobDetection {
                name: python_named_arg(normalized, "id")
                    .or_else(|| schedule.clone())
                    .unwrap_or_else(|| node.display_name().to_string()),
                job_type: "apscheduler-job".into(),
                schedule,
                strategy: "python-apscheduler-scheduled-job".into(),
            });
        }
    }
    if let Some(text) = text {
        out.extend(detect_python_celery_beat_entries(text, node));
    }
    out
}

fn is_celery_task_decorator(text: &str) -> bool {
    text == "shared_task"
        || text.starts_with("shared_task(")
        || text.ends_with(".task")
        || text.contains(".task(")
}

fn is_rq_job_decorator(text: &str) -> bool {
    text == "job" || text.starts_with("job(") || text.ends_with(".job") || text.contains(".job(")
}

fn is_dramatiq_actor_decorator(text: &str) -> bool {
    text == "actor"
        || text.starts_with("actor(")
        || text.ends_with(".actor")
        || text.contains(".actor(")
}

fn is_huey_task_decorator(text: &str) -> bool {
    text == "task"
        || text.starts_with("task(")
        || text.ends_with(".task")
        || text.contains(".task(")
        || text == "periodic_task"
        || text.starts_with("periodic_task(")
        || text.ends_with(".periodic_task")
        || text.contains(".periodic_task(")
}

fn scheduled_annotation_schedule(annotation: &str) -> Option<String> {
    let args = annotation_args(annotation)?;
    for key in [
        "cron",
        "fixedRate",
        "fixedDelay",
        "fixedRateString",
        "fixedDelayString",
    ] {
        if let Some(value) = annotation_value(args, key) {
            return Some(format!("{key}={value}"));
        }
    }
    string_literals(args).into_iter().next()
}

fn annotation_args(annotation: &str) -> Option<&str> {
    let open = annotation.find('(')?;
    let close = find_matching_paren(annotation, open).unwrap_or(annotation.len());
    Some(&annotation[open + 1..close])
}

fn annotation_value(args: &str, key: &str) -> Option<String> {
    for part in split_top_level_commas(args) {
        let (found, value) = part.split_once('=')?;
        if found.trim().ends_with(key) {
            return string_literals(value)
                .into_iter()
                .next()
                .or_else(|| Some(value.trim().trim_matches('"').to_string()));
        }
    }
    None
}

fn python_named_arg(call_text: &str, key: &str) -> Option<String> {
    let open = call_text.find('(')?;
    let close = find_matching_paren(call_text, open).unwrap_or(call_text.len());
    keyword_string_literal(&call_text[open + 1..close], key)
}

fn python_schedule_summary(call_text: &str) -> Option<String> {
    let open = call_text.find('(')?;
    let close = find_matching_paren(call_text, open).unwrap_or(call_text.len());
    let args = &call_text[open + 1..close];
    let mut parts = BTreeSet::new();
    if let Some(trigger) = first_string_literal(args) {
        parts.insert(format!("trigger={trigger}"));
    }
    for key in [
        "cron",
        "interval",
        "second",
        "seconds",
        "minute",
        "minutes",
        "hour",
        "hours",
        "day",
        "month",
        "day_of_week",
    ] {
        if let Some(value) = keyword_string_literal(args, key).or_else(|| keyword_scalar(args, key))
        {
            parts.insert(format!("{key}={value}"));
        }
    }
    (!parts.is_empty()).then(|| parts.into_iter().collect::<Vec<_>>().join(","))
}

fn detect_python_celery_beat_entries(text: &str, node: &SynthNode) -> Vec<JobDetection> {
    let mut out = Vec::new();
    for callee in ["sender.add_periodic_task", "add_periodic_task"] {
        for call in find_call_args(text, callee) {
            let args = split_top_level_commas(call.args);
            if !args
                .iter()
                .any(|arg| arg.contains(&format!("{}.", node.name)) || arg.contains(&node.name))
            {
                continue;
            }
            let schedule = args
                .first()
                .map(|arg| arg.trim().to_string())
                .filter(|arg| !arg.is_empty());
            out.push(JobDetection {
                name: keyword_string_literal(call.args, "name")
                    .or_else(|| schedule.clone())
                    .unwrap_or_else(|| node.display_name().to_string()),
                job_type: "celery-beat".into(),
                schedule,
                strategy: "python-celery-beat-periodic-task".into(),
            });
        }
    }
    out
}

pub(super) fn first_string_literal(args: &str) -> Option<String> {
    split_top_level_commas(args)
        .first()
        .and_then(|arg| first_raw_string_literal(arg))
}

fn keyword_string_literal(args: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=");
    if let Some(pos) = args.replace(' ', "").find(&needle) {
        let compact = args.replace(' ', "");
        return first_raw_string_literal(&compact[pos + needle.len()..]);
    }
    for part in split_top_level_commas(args) {
        let (found, value) = part.split_once('=')?;
        if found.trim() == key {
            return first_raw_string_literal(value);
        }
    }
    None
}

fn keyword_scalar(args: &str, key: &str) -> Option<String> {
    let compact = args.replace(' ', "");
    let needle = format!("{key}=");
    if let Some(pos) = compact.find(&needle) {
        let value = compact[pos + needle.len()..]
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches(['"', '\'']);
        if is_schedule_scalar(value) {
            return Some(value.to_string());
        }
    }
    for part in split_top_level_commas(args) {
        let (found, value) = part.split_once('=')?;
        if found.trim() != key {
            continue;
        }
        let value = value.trim().trim_end_matches(',');
        if is_schedule_scalar(value) {
            return Some(value.to_string());
        }
    }
    None
}

fn is_schedule_scalar(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '*'))
}

fn first_raw_string_literal(text: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < text.len() {
        let byte = text.as_bytes().get(idx).copied()?;
        if matches!(byte, b'\'' | b'"' | b'`') {
            if let Some((literal, _)) = read_string_literal(text, idx) {
                return Some(literal);
            }
        }
        idx += 1;
    }
    None
}
