use super::{infra_config, ResourceDetection};
use crate::engine::{find_call_args, find_matching_paren, node_at_offset, skip_ws, SynthNode};

pub(super) fn extract_workflow_engine_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_workflow_context(text) {
        out.extend(extract_python_workflow_engines(text, nodes));
    }
    if has_java_workflow_context(text) {
        out.extend(extract_java_workflow_engines(text, nodes));
    }
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

pub(super) fn extract_workflow_engine_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some(provider) = workflow_provider_for_config_key(&key) else {
            continue;
        };
        if !workflow_config_value_is_present(&value) {
            continue;
        }
        out.push(ResourceDetection::workflow_engine(
            provider.into(),
            infra_config::config_id(&key),
            workflow_config_strategy(provider),
        ));
    }
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn has_python_workflow_context(text: &str) -> bool {
    text.contains("temporalio.client")
        || text.contains("Client.connect")
        || text.contains("airflow_client")
        || text.contains("AirflowClient")
        || text.contains("prefect")
        || text.contains("PrefectClient")
        || text.contains("camunda")
        || text.contains("zeebe")
}

fn extract_python_workflow_engines(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "temporal",
        &["Client.connect("],
        &[
            (".start_workflow", "python-temporal-start-workflow"),
            (".execute_workflow", "python-temporal-execute-workflow"),
            (".get_workflow_handle", "python-temporal-workflow-handle"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "airflow",
        &["AirflowClient(", "DAGRunApi("],
        &[
            (".post_dag_run", "python-airflow-post-dag-run"),
            (".trigger_dag", "python-airflow-trigger-dag"),
            (".get_dag_run", "python-airflow-get-dag-run"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "prefect",
        &["PrefectClient(", "get_client("],
        &[
            (
                ".create_flow_run_from_deployment",
                "python-prefect-create-flow-run",
            ),
            (".read_flow_run", "python-prefect-read-flow-run"),
            (".set_flow_run_state", "python-prefect-set-flow-run-state"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "zeebe",
        &["ZeebeClient(", "CamundaClient("],
        &[
            (".run_process", "python-zeebe-run-process"),
            (".publish_message", "python-zeebe-publish-message"),
            (".start_process_instance", "python-zeebe-start-process"),
        ],
    ));
    out
}

fn extract_python_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    constructors: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for call in find_call_args(text, callee) {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !python_receiver_is_provider(text, receiver, provider, constructors) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::workflow_engine(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn has_java_workflow_context(text: &str) -> bool {
    text.contains("WorkflowClient")
        || text.contains("WorkflowOptions")
        || text.contains("ZeebeClient")
        || text.contains("CamundaClient")
        || text.contains("ProcessInstanceEvent")
        || text.contains("AirflowClient")
        || text.contains("PrefectClient")
}

fn extract_java_workflow_engines(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "temporal",
        &["WorkflowClient"],
        &[
            (".newWorkflowStub", "java-temporal-new-workflow-stub"),
            (".start", "java-temporal-start"),
            (".execute", "java-temporal-execute"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "zeebe",
        &["ZeebeClient", "CamundaClient"],
        &[
            (".newCreateInstanceCommand", "java-zeebe-create-instance"),
            (".newPublishMessageCommand", "java-zeebe-publish-message"),
            (".send", "java-zeebe-send-command"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "airflow",
        &["AirflowClient"],
        &[
            (".triggerDag", "java-airflow-trigger-dag"),
            (".getDagRun", "java-airflow-get-dag-run"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "prefect",
        &["PrefectClient"],
        &[
            (".createFlowRun", "java-prefect-create-flow-run"),
            (".getFlowRun", "java-prefect-get-flow-run"),
        ],
    ));
    out
}

fn extract_java_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    types: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for (start, _) in find_dotted_call_offsets(text, callee) {
            let typed_receiver = receiver_before_dot(text, start)
                .is_some_and(|receiver| java_receiver_has_type(text, receiver, types));
            if !typed_receiver
                && !java_temporal_static_call(text, start, provider)
                && !java_zeebe_command_chain(text, start, provider)
            {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, start) else {
                continue;
            };
            out.push(ResourceDetection::workflow_engine(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn find_dotted_call_offsets(text: &str, callee: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find(callee) {
        let start = offset + rel;
        let after = start + callee.len();
        if text.as_bytes().get(after).is_some_and(|byte| {
            let ch = char::from(*byte);
            ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
        }) {
            offset = after;
            continue;
        }
        let open = skip_ws(text, after);
        if text.as_bytes().get(open) != Some(&b'(') {
            offset = after;
            continue;
        }
        let Some(close) = find_matching_paren(text, open) else {
            offset = open + 1;
            continue;
        };
        out.push((start, close.saturating_sub(open + 1)));
        offset = close + 1;
    }
    out
}

fn python_receiver_is_provider(
    text: &str,
    receiver: &str,
    provider: &str,
    constructors: &[&str],
) -> bool {
    let receiver = receiver_tail(receiver);
    receiver.to_ascii_lowercase().contains(provider)
        || python_receiver_assigned_to(text, receiver, constructors)
}

fn python_receiver_assigned_to(text: &str, receiver: &str, constructors: &[&str]) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            return false;
        };
        let lhs = lhs
            .trim()
            .trim_start_matches("self.")
            .trim_start_matches("cls.");
        lhs == receiver && constructors.iter().any(|ctor| rhs.contains(ctor))
    })
}

fn java_receiver_has_type(text: &str, receiver: &str, types: &[&str]) -> bool {
    let receiver = receiver_tail(receiver);
    text.lines().any(|line| {
        let line = line.trim();
        types.iter().any(|ty| {
            line.contains(&format!("{ty} {receiver}"))
                || line.contains(&format!("{ty} {receiver},"))
                || line.contains(&format!("{ty} {receiver})"))
                || line.contains(&format!("{ty} {receiver} ="))
        })
    })
}

fn java_temporal_static_call(text: &str, dot_start: usize, provider: &str) -> bool {
    provider == "temporal"
        && (same_statement_prefix(text, dot_start).contains("WorkflowClient")
            || same_statement_prefix(text, dot_start).contains(".newWorkflowStub"))
}

fn java_zeebe_command_chain(text: &str, dot_start: usize, provider: &str) -> bool {
    provider == "zeebe"
        && (same_statement_prefix(text, dot_start).contains("ZeebeClient")
            || same_statement_prefix(text, dot_start).contains(".newCreateInstanceCommand")
            || same_statement_prefix(text, dot_start).contains(".newPublishMessageCommand"))
}

fn same_statement_prefix(text: &str, start: usize) -> &str {
    let line_start = text[..start]
        .rfind(['\n', ';', '{', '}'])
        .map_or(0, |idx| idx + 1);
    &text[line_start..start]
}

fn receiver_before_dot(text: &str, dot_start: usize) -> Option<&str> {
    if text.as_bytes().get(dot_start) != Some(&b'.') {
        return None;
    }
    let mut start = dot_start;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if ch == '.' || ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let receiver = text[start..dot_start].trim_matches('.');
    (!receiver.is_empty()).then_some(receiver)
}

fn receiver_tail(receiver: &str) -> &str {
    receiver.rsplit('.').next().unwrap_or(receiver)
}

fn workflow_provider_for_config_key(key: &str) -> Option<&'static str> {
    if key_contains_any(key, &["temporal"]) {
        Some("temporal")
    } else if key_contains_any(key, &["camunda", "zeebe"]) {
        Some("zeebe")
    } else if key_contains_any(key, &["airflow"]) {
        Some("airflow")
    } else if key_contains_any(key, &["prefect"]) {
        Some("prefect")
    } else {
        None
    }
}

fn workflow_config_value_is_present(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '`']);
    !value.is_empty()
        && !value.starts_with("${")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "none" | "null" | "0"
        )
}

fn workflow_config_strategy(provider: &str) -> &'static str {
    match provider {
        "temporal" => "temporal-config",
        "zeebe" => "zeebe-config",
        "airflow" => "airflow-config",
        "prefect" => "prefect-config",
        _ => "workflow-engine-config",
    }
}

fn key_contains_any(key: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        if needle.contains('.') && key.contains(needle) {
            return true;
        }
        key.split('.')
            .any(|part| part == *needle || part.contains(needle))
    })
}
