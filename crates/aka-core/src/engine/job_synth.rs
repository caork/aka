use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, find_matching_paren, nodes_by_file, process_ids_for_entry, read_repo_text,
    read_string_literal, split_top_level_commas, stable_hash, string_literals, EdgeRec, NodeRec,
    SynthNode, SynthProcess,
};

#[derive(Debug, Clone)]
pub(super) struct SynthJob {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) job_type: String,
    pub(super) schedule: Option<String>,
    pub(super) file_path: String,
    pub(super) handler_id: String,
    pub(super) handler_name: String,
    pub(super) strategy: String,
    pub(super) process_ids: Vec<String>,
}

impl SynthJob {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("jobType".into(), Value::String(self.job_type.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("handlerId".into(), Value::String(self.handler_id.clone()));
        properties.insert(
            "handlerName".into(),
            Value::String(self.handler_name.clone()),
        );
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("jobSource".into(), Value::String("source-scan".into()));
        properties.insert("strategy".into(), Value::String(self.strategy.clone()));
        if let Some(schedule) = &self.schedule {
            properties.insert("schedule".into(), Value::String(schedule.clone()));
        }
        NodeRec {
            id: self.id.clone(),
            label: "Job".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = vec![EdgeRec {
            id: format!("{}:handles:{:016x}", self.id, stable_hash(&self.handler_id)),
            source_id: self.handler_id.clone(),
            target_id: self.id.clone(),
            edge_type: "HANDLES_JOB".into(),
            confidence: 0.68,
            reason: "aka job synthesis".into(),
            step: None,
            evidence: Some(json!({
                "source": "aka-cbm-synth",
                "kind": "job-handler",
                "job": self.name,
                "jobType": self.job_type,
                "schedule": self.schedule,
                "strategy": self.strategy,
            })),
        }];
        for process_id in &self.process_ids {
            out.push(EdgeRec {
                id: format!("{}:entry-process:{:016x}", self.id, stable_hash(process_id)),
                source_id: self.id.clone(),
                target_id: process_id.clone(),
                edge_type: "ENTRY_POINT_OF".into(),
                confidence: 0.52,
                reason: "aka job process linkage".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "job-entry-process",
                    "job": self.name,
                    "jobType": self.job_type,
                })),
            });
        }
        out
    }
}

pub(super) fn synthesize_jobs_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    processes: &[SynthProcess],
) -> Vec<SynthJob> {
    let by_file = job_nodes_by_file(nodes);
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
                });
            }
        }
    }
    out.sort_by(|a, b| {
        a.job_type
            .cmp(&b.job_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.handler_id.cmp(&b.handler_id))
    });
    out
}

#[derive(Debug, Clone)]
struct JobDetection {
    name: String,
    job_type: String,
    schedule: Option<String>,
    strategy: String,
}

fn job_nodes_by_file(nodes: &BTreeMap<String, SynthNode>) -> BTreeMap<String, Vec<&SynthNode>> {
    nodes_by_file(nodes)
        .into_iter()
        .filter(|(file_path, file_nodes)| {
            is_job_source_path(file_path)
                || file_nodes.iter().any(|node| {
                    matches!(
                        node.language.to_ascii_lowercase().as_str(),
                        "java" | "kotlin" | "scala" | "groovy" | "python"
                    )
                })
        })
        .collect()
}

fn is_job_source_path(file_path: &str) -> bool {
    matches!(
        Path::new(&file_path.to_ascii_lowercase())
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("java" | "kt" | "kts" | "scala" | "groovy" | "py")
    )
}

fn detect_node_jobs(text: Option<&str>, node: &SynthNode) -> Vec<JobDetection> {
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
        if !decorator.contains("Scheduled") {
            continue;
        }
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
    }
    out
}

fn detect_python_jobs(text: Option<&str>, node: &SynthNode) -> Vec<JobDetection> {
    let mut out = Vec::new();
    for decorator in &node.decorators {
        let normalized = decorator.trim().trim_start_matches('@');
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

fn first_string_literal(args: &str) -> Option<String> {
    split_top_level_commas(args)
        .first()
        .and_then(|arg| first_raw_string_literal(arg))
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
