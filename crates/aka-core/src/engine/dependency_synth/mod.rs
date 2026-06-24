use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;

use super::{
    project_code_nodes_by_file, read_repo_text, stable_hash, EdgeRec, ProjectSourceSet, SynthNode,
};

mod java;
mod lookup;
mod python;

use java::detect_java_dependency_edges;
use lookup::NodeLookup;
use python::detect_python_dependency_edges;

const DETECTOR_BUDGET: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub(super) struct DependencyBudget {
    started_at: Instant,
    limit: Duration,
}

impl DependencyBudget {
    fn new(limit: Duration) -> Self {
        Self {
            started_at: Instant::now(),
            limit,
        }
    }

    pub(super) fn exceeded(self) -> bool {
        self.started_at.elapsed() >= self.limit
    }
}

#[derive(Debug)]
pub(super) struct DependencyDetection {
    pub edges: Vec<EdgeRec>,
    pub timed_out: bool,
}

impl DependencyDetection {
    pub(super) fn new(edges: Vec<EdgeRec>, timed_out: bool) -> Self {
        Self { edges, timed_out }
    }
}

#[derive(Debug, Clone)]
pub(super) struct DependencyProgress {
    pub current: u64,
    pub total: u64,
    pub phase: DependencyProgressPhase,
}

#[derive(Debug, Clone)]
pub(super) enum DependencyProgressPhase {
    Start,
    FileStart {
        file_path: String,
        node_count: usize,
    },
    FileRead {
        file_path: String,
        byte_count: usize,
    },
    FileMissing {
        file_path: String,
    },
    JavaStart {
        file_path: String,
    },
    JavaDone {
        file_path: String,
        edge_count: usize,
        elapsed_ms: u128,
    },
    JavaTimeout {
        file_path: String,
        edge_count: usize,
        elapsed_ms: u128,
    },
    PythonStart {
        file_path: String,
    },
    PythonDone {
        file_path: String,
        edge_count: usize,
        elapsed_ms: u128,
    },
    PythonTimeout {
        file_path: String,
        edge_count: usize,
        elapsed_ms: u128,
    },
    FileDone {
        file_path: String,
        edge_count: usize,
        elapsed_ms: u128,
    },
}

pub(super) fn synthesize_dependency_edges_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
    existing_call_pairs: &HashSet<(String, String)>,
    mut on_progress: impl FnMut(DependencyProgress),
) -> Vec<EdgeRec> {
    let project_sources = ProjectSourceSet::discover(repo);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let total = by_file.len() as u64;
    let lookup = NodeLookup::new(by_file.values().flatten().copied());
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    on_progress(DependencyProgress {
        current: 0,
        total,
        phase: DependencyProgressPhase::Start,
    });
    for (file_index, (file_path, file_nodes)) in by_file.into_iter().enumerate() {
        let processed = file_index as u64 + 1;
        let file_start = Instant::now();
        on_progress(DependencyProgress {
            current: processed,
            total,
            phase: DependencyProgressPhase::FileStart {
                file_path: file_path.clone(),
                node_count: file_nodes.len(),
            },
        });
        let Some(text) = read_repo_text(repo, &file_path) else {
            on_progress(DependencyProgress {
                current: processed,
                total,
                phase: DependencyProgressPhase::FileMissing { file_path },
            });
            continue;
        };
        on_progress(DependencyProgress {
            current: processed,
            total,
            phase: DependencyProgressPhase::FileRead {
                file_path: file_path.clone(),
                byte_count: text.len(),
            },
        });
        on_progress(DependencyProgress {
            current: processed,
            total,
            phase: DependencyProgressPhase::JavaStart {
                file_path: file_path.clone(),
            },
        });
        let java_start = Instant::now();
        let java_edges = detect_java_dependency_edges(
            &text,
            &file_path,
            &file_nodes,
            &lookup,
            existing_call_pairs,
            DependencyBudget::new(DETECTOR_BUDGET),
        );
        let java_timed_out = java_edges.timed_out;
        let java_edge_count = java_edges.edges.len();
        on_progress(DependencyProgress {
            current: processed,
            total,
            phase: if java_timed_out {
                DependencyProgressPhase::JavaTimeout {
                    file_path: file_path.clone(),
                    edge_count: java_edge_count,
                    elapsed_ms: java_start.elapsed().as_millis(),
                }
            } else {
                DependencyProgressPhase::JavaDone {
                    file_path: file_path.clone(),
                    edge_count: java_edge_count,
                    elapsed_ms: java_start.elapsed().as_millis(),
                }
            },
        });
        out.extend(java_edges.edges);
        on_progress(DependencyProgress {
            current: processed,
            total,
            phase: DependencyProgressPhase::PythonStart {
                file_path: file_path.clone(),
            },
        });
        let python_start = Instant::now();
        let python_edges = detect_python_dependency_edges(
            &text,
            &file_path,
            &file_nodes,
            &lookup,
            existing_call_pairs,
            DependencyBudget::new(DETECTOR_BUDGET),
        );
        let python_timed_out = python_edges.timed_out;
        let python_edge_count = python_edges.edges.len();
        on_progress(DependencyProgress {
            current: processed,
            total,
            phase: if python_timed_out {
                DependencyProgressPhase::PythonTimeout {
                    file_path: file_path.clone(),
                    edge_count: python_edge_count,
                    elapsed_ms: python_start.elapsed().as_millis(),
                }
            } else {
                DependencyProgressPhase::PythonDone {
                    file_path: file_path.clone(),
                    edge_count: python_edge_count,
                    elapsed_ms: python_start.elapsed().as_millis(),
                }
            },
        });
        out.extend(python_edges.edges);
        on_progress(DependencyProgress {
            current: processed,
            total,
            phase: DependencyProgressPhase::FileDone {
                file_path,
                edge_count: out.len(),
                elapsed_ms: file_start.elapsed().as_millis(),
            },
        });
    }
    out.retain(|edge| {
        seen.insert((
            edge.source_id.clone(),
            edge.target_id.clone(),
            edge.edge_type.clone(),
            edge.reason.clone(),
        ))
    });
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn dependency_edge(
    source_id: &str,
    target_id: &str,
    kind: &str,
    strategy: &str,
    dependency: &str,
    confidence: f64,
) -> EdgeRec {
    EdgeRec {
        id: format!(
            "dependency:heuristic:{:016x}",
            stable_hash(&format!(
                "{source_id}|{target_id}|{kind}|{strategy}|{dependency}"
            ))
        ),
        source_id: source_id.to_string(),
        target_id: target_id.to_string(),
        edge_type: "DEPENDS_ON".into(),
        confidence,
        reason: "aka dependency synthesis".into(),
        step: None,
        evidence: Some(json!({
            "source": "aka-cbm-synth",
            "kind": kind,
            "strategy": strategy,
            "dependency": dependency,
        })),
    }
}
