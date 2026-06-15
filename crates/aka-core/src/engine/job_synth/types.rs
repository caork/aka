use serde_json::{json, Map, Value};

use super::super::{stable_hash, EdgeRec, NodeRec};

#[derive(Debug, Clone)]
pub(crate) struct SynthJob {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) job_type: String,
    pub(crate) schedule: Option<String>,
    pub(crate) file_path: String,
    pub(crate) handler_id: String,
    pub(crate) handler_name: String,
    pub(crate) strategy: String,
    pub(crate) process_ids: Vec<String>,
    pub(crate) triggers: Vec<SynthJobTrigger>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SynthJobTrigger {
    pub(crate) node_id: String,
    pub(crate) file_path: String,
    pub(crate) strategy: String,
}

impl SynthJob {
    pub(crate) fn node_rec(&self) -> NodeRec {
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

    pub(crate) fn edge_recs(&self) -> Vec<EdgeRec> {
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
        out
    }
}
