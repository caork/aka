use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_workflow_engine_resources() {
    let repo = temp_repo("configured-workflow-engine-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"temporal:
  target: temporal:7233
camunda:
  zeebe:
    gateway-address: zeebe:26500
airflow:
  base-url: http://airflow:8080
prefect:
  api-url: http://prefect:4200/api
disabled:
  temporal:
    target: ${TEMPORAL_TARGET}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 13),
        json!({"language": "yaml"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:temporal",
        &config_id("temporal.target"),
        "temporal-config",
    );
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:zeebe",
        &config_id("camunda.zeebe.gateway.address"),
        "zeebe-config",
    );
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:airflow",
        &config_id("airflow.base.url"),
        "airflow-config",
    );
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:prefect",
        &config_id("prefect.api.url"),
        "prefect-config",
    );
    assert!(!synth.resources.iter().any(|resource| {
        resource.url == "workflow-engine:temporal"
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("disabled.temporal.target"))
    }));
}

#[test]
fn synthesizes_python_workflow_engine_resources() {
    let repo = temp_repo("python-workflow-engine-resources");
    std::fs::write(
        repo.join("workflows.py"),
        r#"from temporalio.client import Client
from airflow_client.client.api.dag_run_api import DAGRunApi
from prefect.client.orchestration import PrefectClient

temporal = Client.connect("localhost:7233")
airflow = DAGRunApi()
prefect = PrefectClient(api="http://prefect:4200/api")

async def start_order_workflow(order_id):
    return await temporal.start_workflow("OrderWorkflow", order_id, id=str(order_id), task_queue="orders")

def trigger_airflow(payload):
    return airflow.post_dag_run("orders", payload)

async def trigger_prefect(deployment_id):
    return await prefect.create_flow_run_from_deployment(deployment_id)

def ordinary(client):
    client.start_workflow("local")
    client.post_dag_run("local", {})
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "start_order_workflow",
        "workflows.start_order_workflow",
        "workflows.py",
        (9, 10),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "trigger_airflow",
        "workflows.trigger_airflow",
        "workflows.py",
        (12, 13),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "trigger_prefect",
        "workflows.trigger_prefect",
        "workflows.py",
        (15, 16),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "ordinary",
        "workflows.ordinary",
        "workflows.py",
        (18, 20),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:temporal",
        "cbm:1:workflows.start_order_workflow",
        "python-temporal-start-workflow",
    );
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:airflow",
        "cbm:2:workflows.trigger_airflow",
        "python-airflow-post-dag-run",
    );
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:prefect",
        "cbm:3:workflows.trigger_prefect",
        "python-prefect-create-flow-run",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:4:workflows.ordinary"));
}

#[test]
fn synthesizes_java_workflow_engine_resources() {
    let repo = temp_repo("java-workflow-engine-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/workflows")).unwrap();
    let file = "src/main/java/com/example/workflows/WorkflowGateway.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.workflows;

import io.camunda.zeebe.client.ZeebeClient;
import io.temporal.client.WorkflowClient;

class WorkflowGateway {
    Object startTemporal(WorkflowClient workflowClient, Class<?> workflowType) {
        return workflowClient.newWorkflowStub(workflowType).start();
    }

    Object startZeebe(ZeebeClient zeebeClient) {
        return zeebeClient.newCreateInstanceCommand().bpmnProcessId("orders").latestVersion().send();
    }

    Object triggerAirflow(AirflowClient airflowClient, Object request) {
        return airflowClient.triggerDag("orders", request);
    }

    Object ordinary(Client client) {
        client.start();
        return client.send();
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "startTemporal",
        "com.example.workflows.WorkflowGateway.startTemporal",
        file,
        (7, 9),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "startZeebe",
        "com.example.workflows.WorkflowGateway.startZeebe",
        file,
        (11, 13),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "triggerAirflow",
        "com.example.workflows.WorkflowGateway.triggerAirflow",
        file,
        (15, 17),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "ordinary",
        "com.example.workflows.WorkflowGateway.ordinary",
        file,
        (19, 22),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:temporal",
        "cbm:1:com.example.workflows.WorkflowGateway.startTemporal",
        "java-temporal-new-workflow-stub",
    );
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:temporal",
        "cbm:1:com.example.workflows.WorkflowGateway.startTemporal",
        "java-temporal-start",
    );
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:zeebe",
        "cbm:2:com.example.workflows.WorkflowGateway.startZeebe",
        "java-zeebe-create-instance",
    );
    assert_workflow_engine_edge(
        &synth,
        "workflow-engine:airflow",
        "cbm:3:com.example.workflows.WorkflowGateway.triggerAirflow",
        "java-airflow-trigger-dag",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:4:com.example.workflows.WorkflowGateway.ordinary"));
}

fn assert_workflow_engine_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected workflow engine resource {url}"));
    assert_eq!(resource.resource_type, "workflow-engine");
    let edges = resource.edge_recs();
    assert!(
        edges.iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }),
        "expected edge source={source_id} strategy={strategy}; edges={edges:#?}"
    );
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
