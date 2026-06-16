use super::types::SynthJob;
use crate::engine::resource_synth::infra_config;
use crate::engine::stable_hash;

#[derive(Debug, Clone)]
pub(super) struct ConfigJobDetection {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) job_type: String,
    pub(super) schedule: String,
    pub(super) strategy: String,
}

pub(super) fn synthesize_config_jobs(file_path: &str, text: &str) -> Vec<SynthJob> {
    let mut jobs = Vec::new();
    for detection in extract_config_jobs(text) {
        let key = format!(
            "{}|{}|{}|{}",
            file_path, detection.key, detection.job_type, detection.schedule
        );
        jobs.push(SynthJob {
            id: format!("job:heuristic:{:016x}", stable_hash(&key)),
            name: detection.name,
            job_type: detection.job_type,
            schedule: Some(detection.schedule),
            file_path: file_path.to_string(),
            handler_id: None,
            handler_name: None,
            source_config_id: Some(infra_config::config_id(&detection.key)),
            strategy: detection.strategy,
            process_ids: Vec::new(),
            triggers: Vec::new(),
            step_refs: Vec::new(),
        });
    }
    jobs
}

fn extract_config_jobs(text: &str) -> Vec<ConfigJobDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some((job_type, strategy)) = job_kind_for_config(&key) else {
            continue;
        };
        let Some(schedule) = clean_schedule(&value) else {
            continue;
        };
        out.push(ConfigJobDetection {
            name: job_name_from_key(&key, &schedule),
            key,
            job_type: job_type.into(),
            schedule,
            strategy: strategy.into(),
        });
    }
    out.sort_by(|a, b| {
        a.job_type
            .cmp(&b.job_type)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.key.cmp(&b.key))
    });
    out.dedup_by(|a, b| a.key == b.key && a.job_type == b.job_type && a.schedule == b.schedule);
    out
}

fn job_kind_for_config(key: &str) -> Option<(&'static str, &'static str)> {
    if key_contains_any(key, &["celery", "apscheduler", "huey", "rq", "dramatiq"])
        && schedule_key_suffix(key)
    {
        return Some(("python-scheduled-job", "config-python-scheduled-job"));
    }
    if key_contains_any(key, &["cron", "crontab"]) && schedule_key_suffix(key) {
        return Some(("cron-schedule", "config-cron-schedule"));
    }
    if key_contains_any(
        key,
        &["schedule", "scheduler", "scheduled", "periodic", "beat"],
    ) && schedule_key_suffix(key)
    {
        return Some(("scheduled-job", "config-scheduled-job"));
    }
    None
}

fn schedule_key_suffix(key: &str) -> bool {
    key.ends_with(".cron")
        || key.ends_with(".cron.expression")
        || key.ends_with(".crontab")
        || key.ends_with(".schedule")
        || key.ends_with(".scheduler")
        || key.ends_with(".interval")
        || key.ends_with(".fixed.rate")
        || key.ends_with(".fixed.delay")
        || key.ends_with(".fixed.rate.ms")
        || key.ends_with(".fixed.delay.ms")
        || key.ends_with(".period")
        || key.ends_with(".period.seconds")
        || key.ends_with(".every")
        || key == "cron"
        || key == "schedule"
}

fn clean_schedule(value: &str) -> Option<String> {
    let schedule = value
        .trim()
        .trim_matches(['"', '\'', '`'])
        .trim_matches(['[', ']'])
        .trim();
    if schedule.is_empty()
        || schedule.starts_with("${")
        || schedule.contains("://")
        || schedule.len() > 160
    {
        return None;
    }
    if schedule.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || ch.is_ascii_whitespace()
            || matches!(
                ch,
                '*' | '?' | '/' | '-' | '_' | '.' | ',' | ':' | '@' | '(' | ')'
            )
    }) {
        Some(schedule.split_whitespace().collect::<Vec<_>>().join(" "))
    } else {
        None
    }
}

fn job_name_from_key(key: &str, schedule: &str) -> String {
    let parts = key
        .split('.')
        .filter(|part| {
            !matches!(
                *part,
                "spring"
                    | "task"
                    | "tasks"
                    | "scheduling"
                    | "scheduler"
                    | "schedule"
                    | "scheduled"
                    | "cron"
                    | "expression"
                    | "interval"
                    | "fixed"
                    | "rate"
                    | "delay"
                    | "ms"
                    | "seconds"
            )
        })
        .collect::<Vec<_>>();
    let stem = parts
        .last()
        .copied()
        .or_else(|| key.split('.').next_back())
        .unwrap_or("scheduled-job");
    format!("{stem} ({schedule})")
}

fn key_contains_any(key: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        key.split('.')
            .any(|part| part == *needle || part.contains(needle))
    })
}
