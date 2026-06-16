use serde_json::{json, Map, Value};

use super::super::{stable_hash, EdgeRec, NodeRec};

#[derive(Debug, Clone)]
pub(crate) struct SynthJob {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) job_type: String,
    pub(crate) schedule: Option<String>,
    pub(crate) file_path: String,
    pub(crate) handler_id: Option<String>,
    pub(crate) handler_name: Option<String>,
    pub(crate) source_config_id: Option<String>,
    pub(crate) strategy: String,
    pub(crate) process_ids: Vec<String>,
    pub(crate) triggers: Vec<SynthJobTrigger>,
    pub(crate) step_refs: Vec<SynthJobStepRef>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SynthJobTrigger {
    pub(crate) node_id: String,
    pub(crate) file_path: String,
    pub(crate) strategy: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SynthJobStepRef {
    pub(crate) node_id: String,
    pub(crate) step_name: String,
    pub(crate) strategy: String,
}

impl SynthJob {
    pub(crate) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("jobType".into(), Value::String(self.job_type.clone()));
        properties.insert("filePath".into(), Value::String(self.file_path.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert(
            "jobSource".into(),
            Value::String(
                if self.source_config_id.is_some() {
                    "config-scan"
                } else {
                    "source-scan"
                }
                .into(),
            ),
        );
        properties.insert("strategy".into(), Value::String(self.strategy.clone()));
        if let Some(handler_id) = &self.handler_id {
            properties.insert("handlerId".into(), Value::String(handler_id.clone()));
        }
        if let Some(handler_name) = &self.handler_name {
            properties.insert("handlerName".into(), Value::String(handler_name.clone()));
        }
        if let Some(config_id) = &self.source_config_id {
            properties.insert("sourceConfigId".into(), Value::String(config_id.clone()));
        }
        if let Some(schedule) = &self.schedule {
            properties.insert("schedule".into(), Value::String(schedule.clone()));
        }
        NodeRec {
            id: self.id.clone(),
            label: "Job".into(),
            properties,
        }
    }

    pub(crate) fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::new();
        if let Some(handler_id) = &self.handler_id {
            out.push(EdgeRec {
                id: format!("{}:handles:{:016x}", self.id, stable_hash(handler_id)),
                source_id: handler_id.clone(),
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
            });
        }
        if let Some(config_id) = &self.source_config_id {
            out.push(EdgeRec {
                id: format!("{}:declares:{:016x}", self.id, stable_hash(config_id)),
                source_id: config_id.clone(),
                target_id: self.id.clone(),
                edge_type: "DECLARES_JOB".into(),
                confidence: 0.58,
                reason: "aka job config synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "job-config",
                    "job": self.name,
                    "jobType": self.job_type,
                    "schedule": self.schedule,
                    "strategy": self.strategy,
                    "filePath": self.file_path,
                })),
            });
        }
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
        for trigger in &self.triggers {
            out.push(EdgeRec {
                id: format!(
                    "{}:enqueue:{:016x}",
                    self.id,
                    stable_hash(&format!("{}|{}", trigger.node_id, trigger.strategy))
                ),
                source_id: trigger.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: "ENQUEUES_JOB".into(),
                confidence: 0.64,
                reason: "aka job trigger synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "job-trigger",
                    "job": self.name,
                    "jobType": self.job_type,
                    "strategy": trigger.strategy,
                    "filePath": trigger.file_path,
                })),
            });
        }
        for step_ref in &self.step_refs {
            out.push(EdgeRec {
                id: format!(
                    "{}:step-ref:{:016x}",
                    self.id,
                    stable_hash(&format!("{}|{}", step_ref.node_id, step_ref.strategy))
                ),
                source_id: self.id.clone(),
                target_id: step_ref.node_id.clone(),
                edge_type: "USES_STEP".into(),
                confidence: 0.66,
                reason: "aka spring batch step synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": "aka-cbm-synth",
                    "kind": "spring-batch-step-ref",
                    "job": self.name,
                    "jobType": self.job_type,
                    "step": step_ref.step_name,
                    "strategy": step_ref.strategy,
                })),
            });
        }
        out
    }
}
