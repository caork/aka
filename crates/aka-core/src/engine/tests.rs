use super::*;
use std::collections::BTreeSet;

mod semantic_cache;
mod semantic_commands;
mod semantic_events;
mod semantic_graphql;
mod semantic_messaging;
mod semantic_persistence;
mod semantic_process;
mod semantic_resources;
mod semantic_routes;
mod semantic_stream;
mod source_symbols;

fn test_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE nodes (
                id INTEGER PRIMARY KEY,
                project TEXT NOT NULL,
                label TEXT NOT NULL,
                name TEXT,
                qualified_name TEXT,
                file_path TEXT,
                start_line INTEGER,
                end_line INTEGER,
                properties TEXT NOT NULL DEFAULT '{}'
            );
            CREATE TABLE edges (
                id INTEGER PRIMARY KEY,
                project TEXT NOT NULL,
                source_id INTEGER NOT NULL,
                target_id INTEGER NOT NULL,
                type TEXT NOT NULL,
                properties TEXT NOT NULL DEFAULT '{}'
            );
            CREATE TABLE file_hashes (
                project TEXT NOT NULL,
                file_path TEXT NOT NULL
            );",
    )
    .unwrap();
    conn
}

fn insert_node(conn: &Connection, id: i64, label: &str, name: &str, qn: &str, file: &str) {
    conn.execute(
            "INSERT INTO nodes (id, project, label, name, qualified_name, file_path, start_line, end_line, properties)
             VALUES (?1, 'demo', ?2, ?3, ?4, ?5, 1, 3, '{}')",
            rusqlite::params![id, label, name, qn, file],
        )
        .unwrap();
}

fn insert_node_props(
    conn: &Connection,
    id: i64,
    label: &str,
    name: &str,
    qn: &str,
    file: &str,
    props: Value,
) {
    conn.execute(
            "INSERT INTO nodes (id, project, label, name, qualified_name, file_path, start_line, end_line, properties)
             VALUES (?1, 'demo', ?2, ?3, ?4, ?5, 1, 3, ?6)",
            rusqlite::params![id, label, name, qn, file, props.to_string()],
        )
        .unwrap();
}

fn insert_node_props_at(
    conn: &Connection,
    id: i64,
    spec: (&str, &str, &str, &str),
    lines: (i64, i64),
    props: Value,
) {
    let (label, name, qn, file) = spec;
    conn.execute(
            "INSERT INTO nodes (id, project, label, name, qualified_name, file_path, start_line, end_line, properties)
             VALUES (?1, 'demo', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![id, label, name, qn, file, lines.0, lines.1, props.to_string()],
        )
        .unwrap();
}

fn insert_function_node_props_at(
    conn: &Connection,
    id: i64,
    name: &str,
    qn: &str,
    file: &str,
    lines: (i64, i64),
    props: Value,
) {
    conn.execute(
            "INSERT INTO nodes (id, project, label, name, qualified_name, file_path, start_line, end_line, properties)
             VALUES (?1, 'demo', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![id, "Function", name, qn, file, lines.0, lines.1, props.to_string()],
        )
        .unwrap();
}

fn insert_edge(conn: &Connection, id: i64, src: i64, dst: i64, ty: &str) {
    conn.execute(
        "INSERT INTO edges (id, project, source_id, target_id, type, properties)
             VALUES (?1, 'demo', ?2, ?3, ?4, '{}')",
        rusqlite::params![id, src, dst, ty],
    )
    .unwrap();
}

fn insert_file_hash(conn: &Connection, file_path: &str) {
    conn.execute(
        "INSERT INTO file_hashes (project, file_path) VALUES ('demo', ?1)",
        [file_path],
    )
    .unwrap();
}

fn exported_edge_types(conn: &Connection) -> Vec<String> {
    let dir = temp_repo("edges");
    let path = dir.join("edges.ndjson");
    let synth = SynthGraph::default();
    export_edges(conn, "demo", &path, &synth, 0, &mut |_| {}).unwrap();
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<EdgeRec>(line).unwrap().edge_type)
        .collect()
}

fn temp_repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aka-core-engine-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_git(repo: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn synthesize_graph_quiet(conn: &Connection, repo: &Path) -> Result<SynthGraph, EngineError> {
    synthesize_graph(conn, "demo", repo)
}

#[test]
fn synthesizes_call_chain_processes_from_cbm_calls() {
    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Function",
        "main",
        "src/main.ts::main",
        "src/main.ts",
    );
    insert_node(
        &conn,
        2,
        "Function",
        "handleRequest",
        "src/handler.ts::handleRequest",
        "src/handler.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "parseConfig",
        "src/config.ts::parseConfig",
        "src/config.ts",
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let processes = synthesize_graph_quiet(&conn, std::path::Path::new("."))
        .unwrap()
        .processes;
    assert_eq!(processes.len(), 1);
    let p = &processes[0];
    assert_eq!(p.name, "main → parseConfig");
    assert_eq!(
        p.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["main", "handleRequest", "parseConfig"]
    );

    let node = p.node_rec();
    assert_eq!(node.label, "Process");
    assert_eq!(node.properties["processType"], "intra_community");
    assert_eq!(node.properties["stepCount"], 3);
    assert_eq!(node.properties["entryPointId"], p.steps[0].aka_id);
    assert_eq!(node.properties["terminalId"], p.steps[2].aka_id);
    assert_eq!(node.properties["trace"].as_array().expect("trace").len(), 3);
    assert_eq!(
        node.properties["communities"]
            .as_array()
            .expect("communities")
            .len(),
        1
    );

    let edges = p.edge_recs();
    assert_eq!(
        edges
            .iter()
            .filter(|e| e.edge_type == "ENTRY_POINT_OF")
            .count(),
        1
    );
    let steps: Vec<u32> = edges
        .iter()
        .filter(|e| e.edge_type == "STEP_IN_PROCESS")
        .filter_map(|e| e.step)
        .collect();
    assert_eq!(steps, [1, 2, 3]);
}

#[test]
fn synthesizes_community_nodes_and_member_edges() {
    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Function",
        "main",
        "src/main.ts::main",
        "src/main.ts",
    );
    insert_node(
        &conn,
        2,
        "Function",
        "next",
        "src/main.ts::next",
        "src/main.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "store",
        "src/store.ts::store",
        "src/store.ts",
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, std::path::Path::new(".")).unwrap();
    assert_eq!(synth.communities.len(), 1);
    let main = synth
        .communities
        .iter()
        .find(|c| c.heuristic_label == "Src")
        .expect("src community");
    assert_eq!(main.members.len(), 3);

    let node = main.node_rec();
    assert_eq!(node.label, "Community");
    assert_eq!(node.properties["heuristicLabel"], "Src");
    assert_eq!(node.properties["symbolCount"], 3);
    assert_eq!(node.properties["source"], "aka-cbm-synth");
    assert_eq!(main.edge_recs().len(), 3);
    assert!(main
        .edge_recs()
        .iter()
        .all(|edge| edge.edge_type == "MEMBER_OF"));

    let process = synth.processes.first().expect("process");
    assert_eq!(process.process_type, "intra_community");
    assert_eq!(process.communities.len(), 1);
}

#[test]
fn marks_cross_module_processes_as_cross_community() {
    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Function",
        "main",
        "src/api/main.ts::main",
        "src/api/main.ts",
    );
    insert_node(
        &conn,
        2,
        "Function",
        "handle",
        "src/api/handler.ts::handle",
        "src/api/handler.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "save",
        "src/db/store.ts::save",
        "src/db/store.ts",
    );
    insert_node(
        &conn,
        4,
        "Function",
        "commit",
        "src/db/store.ts::commit",
        "src/db/store.ts",
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");
    insert_edge(&conn, 3, 3, 4, "CALLS");

    let synth = synthesize_graph_quiet(&conn, std::path::Path::new(".")).unwrap();
    assert_eq!(synth.communities.len(), 2);
    let process = synth.processes.first().expect("process");
    assert_eq!(process.process_type, "cross_community");
    assert_eq!(process.communities.len(), 2);
}

#[test]
fn marks_single_community_processes_as_intra_community() {
    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Function",
        "main",
        "src/main.ts::main",
        "src/main.ts",
    );
    insert_node(
        &conn,
        2,
        "Function",
        "next",
        "src/main.ts::next",
        "src/main.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "done",
        "src/main.ts::done",
        "src/main.ts",
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let processes = synthesize_graph_quiet(&conn, std::path::Path::new("."))
        .unwrap()
        .processes;
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].process_type, "intra_community");
    assert_eq!(processes[0].communities.len(), 1);
    let node = processes[0].node_rec();
    assert_eq!(node.properties["processType"], "intra_community");
}

#[test]
fn skips_synthesis_when_engine_already_emits_processes() {
    let conn = test_conn();
    insert_node(&conn, 1, "Process", "native", "process:native", "");
    insert_node(
        &conn,
        2,
        "Function",
        "main",
        "src/main.ts::main",
        "src/main.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "next",
        "src/main.ts::next",
        "src/main.ts",
    );
    insert_edge(&conn, 1, 2, 3, "CALLS");
    insert_edge(&conn, 2, 2, 1, "STEP_IN_PROCESS");

    let processes = synthesize_graph_quiet(&conn, std::path::Path::new("."))
        .unwrap()
        .processes;
    assert!(processes.is_empty());
}

#[test]
fn skips_synthesis_when_engine_emits_process_nodes_without_step_edges() {
    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Process",
        "native-empty",
        "process:native-empty",
        "",
    );
    insert_node(
        &conn,
        2,
        "Function",
        "main",
        "src/main.ts::main",
        "src/main.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "next",
        "src/main.ts::next",
        "src/main.ts",
    );
    insert_edge(&conn, 1, 2, 3, "CALLS");

    let processes = synthesize_graph_quiet(&conn, std::path::Path::new("."))
        .unwrap()
        .processes;
    assert!(processes.is_empty());
}

#[test]
fn skips_community_synthesis_when_engine_already_emits_communities() {
    let conn = test_conn();
    insert_node(&conn, 1, "Community", "native", "community:native", "");
    insert_node(
        &conn,
        2,
        "Function",
        "main",
        "src/main.ts::main",
        "src/main.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "next",
        "src/main.ts::next",
        "src/main.ts",
    );
    insert_node(
        &conn,
        4,
        "Function",
        "done",
        "src/main.ts::done",
        "src/main.ts",
    );
    insert_edge(&conn, 1, 2, 3, "CALLS");
    insert_edge(&conn, 2, 3, 4, "CALLS");
    insert_edge(&conn, 3, 2, 1, "MEMBER_OF");
    insert_edge(&conn, 4, 3, 1, "MEMBER_OF");

    let synth = synthesize_graph_quiet(&conn, std::path::Path::new(".")).unwrap();
    assert!(synth.communities.is_empty());
    assert_eq!(synth.processes.len(), 1);
    assert_eq!(synth.processes[0].process_type, "intra_community");
    assert_eq!(synth.processes[0].communities.len(), 1);
}

#[test]
fn process_synthesis_uses_gitnexus_like_entry_scoring_and_dedup() {
    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Function",
        "validateInput",
        "src/api/handler.ts::validateInput",
        "src/api/handler.ts",
    );
    insert_node(
        &conn,
        2,
        "Function",
        "handleLogin",
        "src/api/handler.ts::handleLogin",
        "src/api/handler.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "loadUser",
        "src/auth/user.ts::loadUser",
        "src/auth/user.ts",
    );
    insert_node(
        &conn,
        4,
        "Function",
        "commitSession",
        "src/auth/session.ts::commitSession",
        "src/auth/session.ts",
    );
    insert_node(
        &conn,
        5,
        "Function",
        "handleLoginSpec",
        "src/api/handler.test.ts::handleLoginSpec",
        "src/api/handler.test.ts",
    );
    insert_node(
        &conn,
        6,
        "Function",
        "assertSession",
        "src/api/handler.test.ts::assertSession",
        "src/api/handler.test.ts",
    );
    insert_edge(&conn, 1, 2, 3, "CALLS");
    insert_edge(&conn, 2, 3, 4, "CALLS");
    insert_edge(&conn, 3, 1, 2, "CALLS");
    insert_edge(&conn, 4, 5, 6, "CALLS");

    let synth = synthesize_graph_quiet(&conn, std::path::Path::new(".")).unwrap();
    let processes = synth.processes;
    assert_eq!(processes.len(), 1);
    let process = &processes[0];
    assert_eq!(process.name, "validateInput → commitSession");
    assert_eq!(
        process
            .steps
            .iter()
            .map(|step| step.name.as_str())
            .collect::<Vec<_>>(),
        ["validateInput", "handleLogin", "loadUser", "commitSession"]
    );
    assert!(
        process
            .steps
            .iter()
            .all(|step| !step.file_path.contains(".test.")),
        "test-file entry points should not produce processes"
    );
}

#[test]
fn process_synthesis_uses_spring_runner_source_facts_as_entry_hints() {
    let repo = temp_repo("spring-runner-process-entry-hints");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let main_file = "src/main/java/com/example/ops/MaintenanceConfig.java";
    let test_file = "src/test/java/com/example/ops/FixtureConfig.java";
    std::fs::write(
        repo.join(main_file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationRunner;
import org.springframework.context.annotation.Bean;

class MaintenanceConfig {
    @Bean
    ApplicationRunner ingestOrders(OrderService orders) {
        return args -> orders.loadOrders();
    }
}

class OrderService {
    void loadOrders() {
        persistOrders();
    }

    void persistOrders() {}
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(test_file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationRunner;
import org.springframework.context.annotation.Bean;

class FixtureConfig {
    @Bean
    ApplicationRunner fixtureOrders(OrderService orders) {
        return args -> orders.resetFixtures();
    }
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", main_file, test_file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Method",
            "ingestOrders",
            "com.example.ops.MaintenanceConfig.ingestOrders",
            main_file,
        ),
        (8, 10),
        json!({
            "language": "java",
            "decorators": ["@Bean"],
            "parent_class": "com.example.ops.MaintenanceConfig",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "loadOrders",
            "com.example.ops.OrderService.loadOrders",
            main_file,
        ),
        (14, 16),
        json!({
            "language": "java",
            "parent_class": "com.example.ops.OrderService",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "persistOrders",
            "com.example.ops.OrderService.persistOrders",
            main_file,
        ),
        (18, 18),
        json!({
            "language": "java",
            "parent_class": "com.example.ops.OrderService",
        }),
    );
    insert_node_props_at(
        &conn,
        4,
        (
            "Method",
            "fixtureOrders",
            "com.example.ops.FixtureConfig.fixtureOrders",
            test_file,
        ),
        (8, 10),
        json!({
            "language": "java",
            "decorators": ["@Bean"],
            "parent_class": "com.example.ops.FixtureConfig",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let process = synth
        .processes
        .iter()
        .find(|process| process.name == "ingestOrders → persistOrders")
        .expect("Spring runner bean method should seed process entry");
    assert_eq!(
        process
            .steps
            .iter()
            .map(|step| step.name.as_str())
            .collect::<Vec<_>>(),
        ["ingestOrders", "loadOrders", "persistOrders"]
    );
    assert_eq!(
        process.node_rec().properties["entryReason"],
        "java-spring-runner-bean-source-declaration"
    );
    assert!(!synth
        .processes
        .iter()
        .flat_map(|process| process.steps.iter())
        .any(|step| step.name == "fixtureOrders"));
}

#[test]
fn synthesizes_fastapi_depends_calls_for_processes() {
    let repo = temp_repo("fastapi-depends");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import Depends, APIRouter

router = APIRouter()

def get_current_user():
    return verify_token()

def verify_token():
    return "maya"

def load_order(id: str):
    return {"id": id}

@router.get("/orders/{id}")
def get_order(id: str, user = Depends(get_current_user)):
    order = load_order(id)
    return {"id": order["id"], "user": user}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "get_current_user",
        "api.orders.get_current_user",
        "api/orders.py",
        (5, 6),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "verify_token",
        "api.orders.verify_token",
        "api/orders.py",
        (8, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "load_order",
        "api.orders.load_order",
        "api/orders.py",
        (11, 12),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "get_order",
        "api.orders.get_order",
        "api/orders.py",
        (15, 17),
        json!({
            "decorators": ["@router.get(\"/orders/{id}\")"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/orders/{id}",
        }),
    );
    insert_edge(&conn, 1, 4, 3, "CALLS");
    insert_edge(&conn, 2, 1, 2, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "CALLS"
            && edge.source_id == "cbm:4:api.orders.get_order"
            && edge.target_id == "cbm:1:api.orders.get_current_user"
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str)
                == Some("fastapi-depends")
    }));
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "DEPENDS_ON"
            && edge.source_id == "cbm:4:api.orders.get_order"
            && edge.target_id == "cbm:1:api.orders.get_current_user"
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str)
                == Some("python-fastapi-dependency")
    }));

    let process = synth
        .processes
        .iter()
        .find(|process| process.name == "get_order → verify_token")
        .expect("Depends call should seed route dependency process");
    assert_eq!(
        process
            .steps
            .iter()
            .map(|step| step.name.as_str())
            .collect::<Vec<_>>(),
        ["get_order", "get_current_user", "verify_token"]
    );
}

#[test]
fn synthesizes_fastapi_decorator_dependencies_for_processes() {
    let repo = temp_repo("fastapi-decorator-depends");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter, Depends

router = APIRouter()

def require_user():
    return verify_token()

def verify_token():
    return "maya"

@router.post("/orders", dependencies=[Depends(require_user)])
def create_order():
    return {"ok": True}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "require_user",
        "api.orders.require_user",
        "api/orders.py",
        (5, 6),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "verify_token",
        "api.orders.verify_token",
        "api/orders.py",
        (8, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "create_order",
        "api.orders.create_order",
        "api/orders.py",
        (12, 13),
        json!({
            "decorators": ["@router.post(\"/orders\", dependencies=[Depends(require_user)])"],
            "language": "python",
            "route_method": "POST",
            "route_path": "/orders",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "DEPENDS_ON"
            && edge.source_id == "cbm:3:api.orders.create_order"
            && edge.target_id == "cbm:1:api.orders.require_user"
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str)
                == Some("python-fastapi-dependency")
    }));
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "CALLS"
            && edge.source_id == "cbm:3:api.orders.create_order"
            && edge.target_id == "cbm:1:api.orders.require_user"
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str)
                == Some("fastapi-depends")
    }));

    let process = synth
        .processes
        .iter()
        .find(|process| process.name == "create_order → verify_token")
        .expect("decorator dependency should seed route dependency process");
    assert_eq!(
        process
            .steps
            .iter()
            .map(|step| step.name.as_str())
            .collect::<Vec<_>>(),
        ["create_order", "require_user", "verify_token"]
    );
}

#[test]
fn synthesizes_fastapi_annotated_dependency_aliases_for_processes() {
    let repo = temp_repo("fastapi-annotated-depends");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from typing import Annotated
from fastapi import APIRouter, Depends

router = APIRouter()

def get_current_user():
    return verify_token()

def verify_token():
    return "maya"

CurrentUser = Annotated[
    str,
    Depends(get_current_user),
]

@router.get("/orders")
def list_orders(user: CurrentUser):
    return {"user": user}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "get_current_user",
        "api.orders.get_current_user",
        "api/orders.py",
        (6, 7),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "verify_token",
        "api.orders.verify_token",
        "api/orders.py",
        (9, 10),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "list_orders",
        "api.orders.list_orders",
        "api/orders.py",
        (18, 19),
        json!({
            "decorators": ["@router.get(\"/orders\")"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/orders",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "DEPENDS_ON"
            && edge.source_id == "cbm:3:api.orders.list_orders"
            && edge.target_id == "cbm:1:api.orders.get_current_user"
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("python-fastapi-annotated-alias")
    }));
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "CALLS"
            && edge.source_id == "cbm:3:api.orders.list_orders"
            && edge.target_id == "cbm:1:api.orders.get_current_user"
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("kind"))
                .and_then(Value::as_str)
                == Some("fastapi-depends")
    }));

    let process = synth
        .processes
        .iter()
        .find(|process| process.name == "list_orders → verify_token")
        .expect("Annotated dependency alias should seed dependency process");
    assert_eq!(
        process
            .steps
            .iter()
            .map(|step| step.name.as_str())
            .collect::<Vec<_>>(),
        ["list_orders", "get_current_user", "verify_token"]
    );
}

#[test]
fn process_cap_uses_whole_graph_symbol_count_like_gitnexus() {
    let conn = test_conn();
    let mut id = 1_i64;
    let mut edge_id = 1_i64;
    for idx in 0..25 {
        let entry = id;
        insert_node(
            &conn,
            entry,
            "Function",
            &format!("entry{idx}"),
            &format!("src/api/flow{idx}.ts::entry{idx}"),
            &format!("src/api/flow{idx}.ts"),
        );
        id += 1;
        let middle = id;
        insert_node(
            &conn,
            middle,
            "Function",
            &format!("service{idx}"),
            &format!("src/service/flow{idx}.ts::service{idx}"),
            &format!("src/service/flow{idx}.ts"),
        );
        id += 1;
        let terminal = id;
        insert_node(
            &conn,
            terminal,
            "Function",
            &format!("save{idx}"),
            &format!("src/db/flow{idx}.ts::save{idx}"),
            &format!("src/db/flow{idx}.ts"),
        );
        id += 1;
        insert_edge(&conn, edge_id, entry, middle, "CALLS");
        edge_id += 1;
        insert_edge(&conn, edge_id, middle, terminal, "CALLS");
        edge_id += 1;
    }
    for idx in 0..175 {
        insert_node(
            &conn,
            id,
            "Property",
            &format!("field{idx}"),
            &format!("src/models/order.ts::Order::field{idx}"),
            "src/models/order.ts",
        );
        id += 1;
    }

    let processes = synthesize_graph_quiet(&conn, std::path::Path::new("."))
        .unwrap()
        .processes;
    assert_eq!(processes.len(), 25);
}

#[test]
fn synthesizes_fastapi_include_router_prefixes() {
    let repo = temp_repo("fastapi-routes");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::write(
        repo.join("main.py"),
        r#"from fastapi import FastAPI
from api import orders

app = FastAPI()
app.include_router(orders.router, prefix="/api")
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/orders")

@router.get("/{id}")
def get_order(id: str):
    return {"id": id}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "api.orders.get_order",
        "api/orders.py",
        json!({
            "decorators": ["@router.get(\"/{id}\")"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("fastapi route with include_router prefix");
    assert!(
        synth.routes.iter().all(|route| route.route != "/{id}"),
        "Python decorator literals should not create unprefixed duplicate routes"
    );
    assert_eq!(route.method.as_deref(), Some("GET"));
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:1:api.orders.get_order")
    );

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"HANDLES_ROUTE".to_string()));
}

#[test]
fn synthesizes_nested_fastapi_include_router_prefixes() {
    let repo = temp_repo("fastapi-nested-routes");
    std::fs::create_dir_all(repo.join("api/v1")).unwrap();
    std::fs::write(
        repo.join("main.py"),
        r#"from fastapi import FastAPI
from api import v1

app = FastAPI()
app.include_router(v1.router, prefix="/api")
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("api/v1/__init__.py"),
        r#"from fastapi import APIRouter
from api.v1 import orders

router = APIRouter(prefix="/v1")
router.include_router(orders.router)
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("api/v1/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/orders")

@router.get("/{id}")
def get_order(id: str):
    return {"id": id}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "api.v1.orders.get_order",
        "api/v1/orders.py",
        json!({
            "decorators": ["@router.get(\"/{id}\")"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/v1/orders/{id}")
        .unwrap_or_else(|| {
            panic!(
                "fastapi route with nested include_router prefixes; got {:?}",
                synth
                    .routes
                    .iter()
                    .map(|route| route.route.as_str())
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(route.method.as_deref(), Some("GET"));
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:1:api.v1.orders.get_order")
    );
    assert!(synth.routes.iter().all(|route| {
        route.route != "/v1/orders/{id}" && route.route != "/orders/{id}" && route.route != "/{id}"
    }));
}

#[test]
fn synthesizes_django_urlconf_routes() {
    let repo = temp_repo("django-urlconf-routes");
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from django.urls import path, re_path
from . import views

urlpatterns = [
    path("orders/<int:id>/", views.get_order, name="order-detail"),
    re_path(r"^legacy/orders/(?P<id>\d+)/$", views.legacy_order, name="legacy-order"),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"def get_order(request, id):
    return {"id": id}

def legacy_order(request, id):
    return {"id": id}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "get_order",
        "orders.views.get_order",
        "orders/views.py",
        (1, 2),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "legacy_order",
        "orders.views.legacy_order",
        "orders/views.py",
        (4, 5),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/orders/{id}")
        .expect("django path route");
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:1:orders.views.get_order")
    );
    assert!(route
        .edge_recs()
        .iter()
        .any(|edge| edge.edge_type == "HANDLES_ROUTE"));

    let legacy = synth
        .routes
        .iter()
        .find(|route| route.route == "/legacy/orders/{id}")
        .expect("django re_path route");
    assert_eq!(
        legacy.handler_id.as_deref(),
        Some("cbm:2:orders.views.legacy_order")
    );
}

#[test]
fn django_urlconf_routes_use_project_sources_and_exclude_configured_tests() {
    let repo = temp_repo("django-urlconf-project-sources");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(
        repo.join("pyproject.toml"),
        "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from django.urls import path
from . import views

urlpatterns = [
    path("orders/<int:id>/", views.get_order, name="order-detail"),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("tests/urls.py"),
        r#"from django.urls import path
from orders import views

urlpatterns = [
    path("fixture-orders/<int:id>/", views.fixture_order, name="fixture-order"),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"def get_order(request, id):
    return {"id": id}

def fixture_order(request, id):
    return {"id": id}
"#,
    )
    .unwrap();
    run_git(
        &repo,
        &[
            "add",
            "pyproject.toml",
            "orders/urls.py",
            "tests/urls.py",
            "orders/views.py",
        ],
    );

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "get_order",
        "orders.views.get_order",
        "orders/views.py",
        (1, 2),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "fixture_order",
        "orders.views.fixture_order",
        "orders/views.py",
        (4, 5),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth
        .routes
        .iter()
        .any(|route| route.route == "/orders/{id}"));
    assert!(synth
        .routes
        .iter()
        .all(|route| route.route != "/fixture-orders/{id}"));
}

#[test]
fn synthesizes_django_include_urlconf_prefixes() {
    let repo = temp_repo("django-include-urlconf-routes");
    std::fs::create_dir_all(repo.join("project")).unwrap();
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(
        repo.join("project/urls.py"),
        r#"from django.urls import include, path

urlpatterns = [
    path("api/", include(("orders.urls", "orders"), namespace="orders")),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from django.urls import path
from . import views

urlpatterns = [
    path("orders/<int:id>/", views.get_order, name="order-detail"),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"def get_order(request, id):
    return {"id": id}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "get_order",
        "orders.views.get_order",
        "orders/views.py",
        (1, 2),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("django included route");
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:1:orders.views.get_order")
    );
    assert!(
        synth
            .routes
            .iter()
            .all(|route| route.route != "/orders/{id}"),
        "included URLConf should not emit an unprefixed duplicate"
    );
}

#[test]
fn synthesizes_nested_django_include_urlconf_prefixes() {
    let repo = temp_repo("django-nested-include-urlconf-routes");
    std::fs::create_dir_all(repo.join("project")).unwrap();
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(
        repo.join("project/urls.py"),
        r#"from django.urls import include, path

urlpatterns = [
    path("api/", include("api.urls")),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("api/urls.py"),
        r#"from django.urls import include, path

urlpatterns = [
    path("v1/", include("orders.urls")),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from django.urls import path
from . import views

urlpatterns = [
    path("orders/<int:id>/", views.get_order, name="order-detail"),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"def get_order(request, id):
    return {"id": id}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "get_order",
        "orders.views.get_order",
        "orders/views.py",
        (1, 2),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/v1/orders/{id}")
        .expect("nested django included route");
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:1:orders.views.get_order")
    );
    assert!(
        synth
            .routes
            .iter()
            .all(|route| { route.route != "/orders/{id}" && route.route != "/v1/orders/{id}" }),
        "nested include should only emit fully prefixed routes"
    );
}

#[test]
fn synthesizes_drf_router_registered_viewset_routes() {
    let repo = temp_repo("django-drf-router-routes");
    std::fs::create_dir_all(repo.join("project")).unwrap();
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(
        repo.join("project/urls.py"),
        r#"from django.urls import include, path
from orders.urls import router

urlpatterns = [
    path("api/", include(router.urls)),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from rest_framework.routers import DefaultRouter
from . import views

router = DefaultRouter()
router.register("orders", views.OrderViewSet, basename="order")

urlpatterns = router.urls
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"class OrderViewSet:
    def list(self, request):
        return []

    def retrieve(self, request, pk=None):
        return {"id": pk}

    @action(detail=True, methods=["post"], url_path="cancel")
    def cancel(self, request, pk=None):
        return {"id": pk}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderViewSet",
            "orders.views.OrderViewSet",
            "orders/views.py",
        ),
        (1, 6),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "list",
        "orders.views.OrderViewSet.list",
        "orders/views.py",
        (2, 3),
        json!({
            "language": "python",
            "parent_class": "OrderViewSet",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "retrieve",
        "orders.views.OrderViewSet.retrieve",
        "orders/views.py",
        (5, 6),
        json!({
            "language": "python",
            "parent_class": "OrderViewSet",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "cancel",
        "orders.views.OrderViewSet.cancel",
        "orders/views.py",
        (8, 10),
        json!({
            "decorators": ["@action(detail=True, methods=[\"post\"], url_path=\"cancel\")"],
            "language": "python",
            "parent_class": "OrderViewSet",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let collection = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders")
        .expect("drf router collection route");
    assert_eq!(
        collection.handler_id.as_deref(),
        Some("cbm:2:orders.views.OrderViewSet.list")
    );
    let detail = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("drf router detail route");
    assert_eq!(
        detail.handler_id.as_deref(),
        Some("cbm:3:orders.views.OrderViewSet.retrieve")
    );
    let cancel = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}/cancel")
        .expect("drf detail action route");
    assert_eq!(
        cancel.handler_id.as_deref(),
        Some("cbm:4:orders.views.OrderViewSet.cancel")
    );
    assert_eq!(cancel.method.as_deref(), Some("POST"));
}

#[test]
fn synthesizes_fastapi_local_apirouter_prefixes() {
    let repo = temp_repo("fastapi-local-router");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/api/orders")

@router.get("/{id}")
def get_order(id: str):
    return {"id": id}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "api.orders.get_order",
        "api/orders.py",
        json!({
            "decorators": ["@router.get(\"/{id}\")"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("fastapi route with local APIRouter prefix");
    assert!(
        synth.routes.iter().all(|route| route.route != "/{id}"),
        "Python decorator literals should not create unprefixed duplicate routes"
    );
    assert_eq!(route.method.as_deref(), Some("GET"));
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:1:api.orders.get_order")
    );
}

#[test]
fn scans_fastapi_local_apirouter_prefixes() {
    let repo = temp_repo("fastapi-prefix-scan");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/api/orders")
"#,
    )
    .unwrap();

    let prefixes = python_router_prefixes_by_file(
        &repo,
        ["api/orders.py", "tests/app_test.py", "scratch/app.py"].into_iter(),
    );
    let file = prefixes.get("api/orders.py").expect("file prefixes");
    assert_eq!(
        file.local_by_router.get("router").map(String::as_str),
        Some("/api/orders")
    );
}

#[test]
fn python_router_prefix_scan_uses_project_sources_and_excludes_tests() {
    let repo = temp_repo("fastapi-prefix-project-sources");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::create_dir_all(repo.join("scratch")).unwrap();
    std::fs::write(repo.join(".gitignore"), "scratch/\n").unwrap();
    std::fs::write(
        repo.join("pyproject.toml"),
        "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("app.py"),
        r#"from api import orders

app.include_router(orders.router, prefix="/api")
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("tests/app_test.py"),
        r#"from api import orders

app.include_router(orders.router, prefix="/fixture")
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("scratch/app.py"),
        r#"from api import orders

app.include_router(orders.router, prefix="/scratch")
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/orders")
"#,
    )
    .unwrap();
    run_git(
        &repo,
        &[
            "add",
            ".gitignore",
            "pyproject.toml",
            "app.py",
            "tests/app_test.py",
            "api/orders.py",
        ],
    );

    let prefixes = python_router_prefixes_by_file(&repo, ["api/orders.py"].into_iter());
    let file = prefixes.get("api/orders.py").expect("file prefixes");
    assert_eq!(
        file.local_by_router.get("router").map(String::as_str),
        Some("/orders")
    );
    assert_eq!(file.include, ["/api"]);
}

#[test]
fn synthesizes_flask_blueprint_prefixes() {
    let repo = temp_repo("flask-blueprint-routes");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::write(
        repo.join("app.py"),
        r#"from flask import Flask
from api import orders

app = Flask(__name__)
app.register_blueprint(orders.bp, url_prefix="/api")
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from flask import Blueprint

bp = Blueprint("orders", __name__, url_prefix="/orders")

@bp.get("/<id>")
def get_order(id: str):
    return {"id": id}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "api.orders.get_order",
        "api/orders.py",
        json!({
            "decorators": ["@bp.get(\"/<id>\")"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/<id>",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("flask blueprint route with app and blueprint prefixes");
    assert!(
        synth.routes.iter().all(|route| route.route != "/{id}"),
        "Flask decorator literals should not create unprefixed duplicate routes"
    );
    assert_eq!(route.method.as_deref(), Some("GET"));
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:1:api.orders.get_order")
    );
}

#[test]
fn scans_flask_imported_blueprint_prefixes() {
    let repo = temp_repo("flask-blueprint-prefix-scan");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::write(
        repo.join("app.py"),
        r#"from flask import Flask
from api.orders import bp

app = Flask(__name__)
app.register_blueprint(bp, url_prefix="/api")
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from flask import Blueprint

bp = Blueprint("orders", __name__, url_prefix="/orders")
"#,
    )
    .unwrap();

    let prefixes = python_router_prefixes_by_file(&repo, ["api/orders.py"].into_iter());
    let file = prefixes.get("api/orders.py").expect("file prefixes");
    assert_eq!(
        file.local_by_router.get("bp").map(String::as_str),
        Some("/orders")
    );
    assert_eq!(file.include, ["/api"]);
}

#[test]
fn synthesizes_tool_nodes_and_handler_edges() {
    let repo = temp_repo("tools");
    std::fs::create_dir_all(repo.join("src/mcp")).unwrap();
    std::fs::write(
            repo.join("src/mcp/server.ts"),
            "export function handleIndexRepo() {}\nserver.tool('index_repo', { description: 'Index a repository' }, handleIndexRepo);",
        )
        .unwrap();

    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Function",
        "handleIndexRepo",
        "src/mcp/server.ts::handleIndexRepo",
        "src/mcp/server.ts",
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_eq!(synth.tools.len(), 1);
    let tool = &synth.tools[0];
    assert_eq!(tool.name, "index_repo");
    assert_eq!(tool.description, "Index a repository");
    assert!(tool.handler_id.is_some());
    assert_eq!(tool.edge_recs()[0].edge_type, "HANDLES_TOOL");
}

#[test]
fn scans_tool_properties_across_non_ascii_text() {
    let text = r#"const label = "cấu hình";
const tool = {
  name: "sync_orders",
  description: "Đồng bộ đơn hàng"
};"#;

    assert_eq!(
        property_name_offsets(text, "name"),
        vec![text.find("name").unwrap()]
    );
    let tools = tool_synth::extract_tool_defs_for_tests(text);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].0, "sync_orders");
    assert_eq!(tools[0].1, "Đồng bộ đơn hàng");
}

#[test]
fn synthesizes_python_class_properties_from_schema_and_orm_fields() {
    let text = r#"class UserBase(BaseModel):
    id: int
    username: str
    token_type: str = "Bearer"

    class Config:
        pass

class User(Base):
    __tablename__ = "users"
    email = Column(String, unique=True)
    carts = relationship("Cart")

    def full_name(self):
        return self.email
"#;

    let props = extract_python_class_properties(
        text,
        "app/schemas/auth.py",
        "class:userbase",
        "UserBase",
        1,
        7,
    );
    let names: Vec<_> = props.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["id", "username", "token_type"]);
    assert_eq!(props[0].declared_type.as_deref(), Some("int"));
    assert_eq!(props[2].declared_type.as_deref(), Some("str"));
    assert_eq!(props[0].start_line, 1);

    let orm =
        extract_python_class_properties(text, "app/models/models.py", "class:user", "User", 9, 15);
    let names: Vec<_> = orm.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["email", "carts"]);
}

#[test]
fn warns_when_engine_misses_repo_source_language() {
    let repo = temp_repo("missing-java-warning");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/App.java"),
        "class App {}\n",
    )
    .unwrap();
    std::fs::write(repo.join("application.yml"), "server: {}\n").unwrap();
    run_git(
        &repo,
        &[
            "add",
            "src/main/java/com/example/App.java",
            "application.yml",
        ],
    );
    let conn = test_conn();
    insert_file_hash(&conn, "application.yml");

    let mut warnings = Vec::new();
    warn_missing_source_extensions(&repo, &conn, "demo", &mut |ev| {
        if let EngineEvent::Warning { message } = ev {
            warnings.push(message.clone());
        }
    })
    .unwrap();

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("0 Java source files"));
}

#[test]
fn source_language_warning_ignores_build_configured_test_sources() {
    let repo = temp_repo("missing-java-warning-test-source");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/test/java/com/example")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    std::fs::write(
        repo.join("src/test/java/com/example/AppTest.java"),
        "class AppTest {}\n",
    )
    .unwrap();
    std::fs::write(repo.join("application.yml"), "server: {}\n").unwrap();
    run_git(
        &repo,
        &[
            "add",
            "pom.xml",
            "src/test/java/com/example/AppTest.java",
            "application.yml",
        ],
    );
    let conn = test_conn();
    insert_file_hash(&conn, "application.yml");

    let mut warnings = Vec::new();
    warn_missing_source_extensions(&repo, &conn, "demo", &mut |ev| {
        if let EngineEvent::Warning { message } = ev {
            warnings.push(message.clone());
        }
    })
    .unwrap();

    assert!(warnings.is_empty());
}

#[test]
fn source_language_warning_does_not_count_indexed_test_sources_as_java_coverage() {
    let repo = temp_repo("missing-main-java-warning-test-indexed");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/java/com/example")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/App.java"),
        "class App {}\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("src/test/java/com/example/AppTest.java"),
        "class AppTest {}\n",
    )
    .unwrap();
    run_git(
        &repo,
        &[
            "add",
            "pom.xml",
            "src/main/java/com/example/App.java",
            "src/test/java/com/example/AppTest.java",
        ],
    );
    let conn = test_conn();
    insert_file_hash(&conn, "src/test/java/com/example/AppTest.java");

    let mut warnings = Vec::new();
    warn_missing_source_extensions(&repo, &conn, "demo", &mut |ev| {
        if let EngineEvent::Warning { message } = ev {
            warnings.push(message.clone());
        }
    })
    .unwrap();

    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("0 Java source files"));
}

#[test]
fn reads_cbm_file_hashes_rel_path_column() {
    let repo = temp_repo("file-hashes-rel-path");
    std::fs::create_dir_all(repo.join("src/main/java")).unwrap();
    std::fs::write(repo.join("src/main/java/App.java"), "class App {}\n").unwrap();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE nodes (
                id INTEGER PRIMARY KEY,
                project TEXT NOT NULL,
                label TEXT NOT NULL,
                name TEXT,
                qualified_name TEXT,
                file_path TEXT,
                start_line INTEGER,
                end_line INTEGER,
                properties TEXT NOT NULL DEFAULT '{}'
            );
            CREATE TABLE file_hashes (
                project TEXT NOT NULL,
                rel_path TEXT NOT NULL
            );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO file_hashes (project, rel_path) VALUES ('demo', 'src/main/java/App.java')",
        [],
    )
    .unwrap();

    let project_sources = ProjectSourceSet::discover(&repo);
    let exts = indexed_project_source_extensions(&conn, "demo", &repo, &project_sources).unwrap();
    assert!(exts.contains("java"));
}

#[test]
fn synthesizes_spring_dependency_injection_edges() {
    let repo = temp_repo("spring-dependencies");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderController.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.beans.factory.annotation.Autowired;
import org.springframework.context.annotation.Bean;
import org.springframework.stereotype.Component;

@Component
class OrderController {
    private final OrderService service;

    OrderController(OrderService service) {
        this.service = service;
    }

    @Autowired
    private OrderRepository repository;
}

class OrderConfig {
    @Bean
    OrderHandler orderHandler(OrderService service, OrderRepository repository) {
        return new OrderHandler(service, repository);
    }
}

class OrderService {}
interface OrderRepository {}
class OrderHandler {}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderController",
            "com.example.orders.OrderController",
            file,
        ),
        (7, 17),
        json!({
            "language": "java",
            "decorators": ["@Component"],
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Class",
            "OrderService",
            "com.example.orders.OrderService",
            file,
        ),
        (27, 27),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Interface",
            "OrderRepository",
            "com.example.orders.OrderRepository",
            file,
        ),
        (28, 28),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        4,
        (
            "Method",
            "orderHandler",
            "com.example.orders.OrderConfig.orderHandler",
            file,
        ),
        (21, 24),
        json!({
            "language": "java",
            "decorators": ["@Bean"],
            "parent_class": "com.example.orders.OrderConfig",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "DEPENDS_ON"
            && edge.source_id == "cbm:1:com.example.orders.OrderController"
            && edge.target_id == "cbm:2:com.example.orders.OrderService"
    }));
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "DEPENDS_ON"
            && edge.source_id == "cbm:1:com.example.orders.OrderController"
            && edge.target_id == "cbm:3:com.example.orders.OrderRepository"
    }));
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "DEPENDS_ON"
            && edge.source_id == "cbm:4:com.example.orders.OrderConfig.orderHandler"
            && edge.target_id == "cbm:2:com.example.orders.OrderService"
    }));
}

#[test]
fn synthesizes_spring_config_nodes_and_consumers() {
    let repo = temp_repo("spring-configs");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    std::fs::write(
        repo.join("src/main/resources/application.yml"),
        r#"orders:
  retry:
    max-attempts: 3
  payments:
    timeout: 30s
server:
  port: 8080
"#,
    )
    .unwrap();
    let file = "src/main/java/com/example/orders/OrderSettings.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.beans.factory.annotation.Value;
import org.springframework.boot.context.properties.ConfigurationProperties;

@ConfigurationProperties(prefix = "orders.retry")
class OrderSettings {
    @Value("${orders.payments.timeout:10s}")
    String timeout;

    void load(org.springframework.core.env.Environment env) {
        env.getProperty("orders.retry.max-attempts");
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderSettings",
            "com.example.orders.OrderSettings",
            file,
        ),
        (6, 14),
        json!({
            "language": "java",
            "decorators": ["@ConfigurationProperties(prefix = \"orders.retry\")"],
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "load",
            "com.example.orders.OrderSettings.load",
            file,
        ),
        (11, 13),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderSettings",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Field",
            "timeout",
            "com.example.orders.OrderSettings.timeout",
            file,
        ),
        (9, 9),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderSettings",
            "decorators": ["@Value(\"${orders.payments.timeout:10s}\")"],
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let keys: BTreeSet<_> = synth
        .configs
        .iter()
        .map(|config| config.key.as_str())
        .collect();
    assert!(keys.contains("orders.retry.max-attempts"));
    assert!(keys.contains("orders.payments.timeout"));
    assert!(keys.contains("server.port"));
    assert!(keys.contains("orders.retry"));
    let edge_types: Vec<_> = synth
        .configs
        .iter()
        .flat_map(SynthConfig::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"USES_CONFIG".to_string()));
    let timeout = synth
        .configs
        .iter()
        .find(|config| config.key == "orders.payments.timeout")
        .expect("@Value config node");
    assert!(timeout
        .edge_recs()
        .iter()
        .any(|edge| edge.source_id == "cbm:3:com.example.orders.OrderSettings.timeout"));
}

#[test]
fn synthesizes_spring_configs_from_source_annotations_without_metadata() {
    let repo = temp_repo("spring-config-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderSettings.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.beans.factory.annotation.Value;
import org.springframework.boot.context.properties.ConfigurationProperties;

@ConfigurationProperties(
    prefix = "orders.retry")
class OrderSettings {
    @Value(
        "${orders.payments.timeout:10s}")
    String timeout;
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderSettings",
            "com.example.orders.OrderSettings",
            file,
        ),
        (8, 12),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Field",
            "timeout",
            "com.example.orders.OrderSettings.timeout",
            file,
        ),
        (11, 11),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderSettings",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let retry = synth
        .configs
        .iter()
        .find(|config| config.key == "orders.retry")
        .expect("class-level @ConfigurationProperties source annotation");
    assert_eq!(retry.config_type, "spring-property-prefix");
    assert!(retry
        .edge_recs()
        .iter()
        .any(|edge| edge.source_id == "cbm:1:com.example.orders.OrderSettings"));

    let timeout = synth
        .configs
        .iter()
        .find(|config| config.key == "orders.payments.timeout")
        .expect("field-level @Value source annotation");
    assert_eq!(timeout.config_type, "spring-property");
    assert!(timeout
        .edge_recs()
        .iter()
        .any(|edge| edge.source_id == "cbm:2:com.example.orders.OrderSettings.timeout"));
}

#[test]
fn synthesizes_python_config_nodes_and_consumers() {
    let repo = temp_repo("python-configs");
    std::fs::write(
        repo.join("settings.py"),
        r#"PAYMENTS_TIMEOUT = "10s"
ORDERS_RETRY_MAX_ATTEMPTS = 3
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("service.py"),
        r#"import os
from django.conf import settings

def charge():
    timeout = os.getenv("PAYMENTS_TIMEOUT", "5s")
    api_key = os.environ["PAYMENTS_API_KEY"]
    attempts = settings.ORDERS_RETRY_MAX_ATTEMPTS
    return timeout, api_key, attempts
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "charge",
        "service.charge",
        "service.py",
        (4, 8),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let keys: BTreeSet<_> = synth
        .configs
        .iter()
        .map(|config| config.key.as_str())
        .collect();
    assert!(keys.contains("payments.timeout"));
    assert!(keys.contains("orders.retry.max.attempts"));
    assert!(keys.contains("payments.api.key"));
    let edges: Vec<_> = synth
        .configs
        .iter()
        .flat_map(SynthConfig::edge_recs)
        .collect();
    assert!(edges
        .iter()
        .any(|edge| edge.source_id == "cbm:1:service.charge"));
}

#[test]
fn synthesizes_config_files_without_symbol_nodes() {
    let repo = temp_repo("config-only");
    std::fs::write(
        repo.join("application.properties"),
        "orders.retry.max-attempts=5\n",
    )
    .unwrap();

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth
        .configs
        .iter()
        .any(|config| config.key == "orders.retry.max-attempts"));
}

#[test]
fn config_synthesis_uses_project_sources_and_excludes_tests() {
    let repo = temp_repo("git-project-config-sources");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/resources")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    std::fs::write(
        repo.join("src/main/resources/application.yml"),
        r#"
orders:
  endpoint: https://orders.example.test
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/test/resources/application.yml"),
        r#"
fixtures:
  endpoint: https://fixtures.example.test
"#,
    )
    .unwrap();
    run_git(
        &repo,
        &[
            "add",
            "pom.xml",
            "src/main/resources/application.yml",
            "src/test/resources/application.yml",
        ],
    );

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let keys: Vec<_> = synth
        .configs
        .iter()
        .map(|config| config.key.as_str())
        .collect();
    assert!(keys.contains(&"orders.endpoint"));
    assert!(!keys.contains(&"fixtures.endpoint"));
}

#[test]
fn semantic_synthesis_excludes_build_configured_test_sources() {
    let repo = temp_repo("git-project-semantic-test-sources");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/resources/db/migration")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/resources/db/migration")).unwrap();
    let service_file = "src/main/java/com/example/orders/OrderService.java";
    let test_file = "src/test/java/com/example/orders/OrderServiceTest.java";
    let migration_file = "src/main/resources/db/migration/V1__create_orders.sql";
    let test_migration_file = "src/test/resources/db/migration/V999__test_fixture.sql";
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    std::fs::write(
        repo.join(service_file),
        r#"package com.example.orders;

import org.springframework.cache.annotation.Cacheable;
import org.springframework.transaction.annotation.Transactional;

class OrderService {
    @Cacheable(cacheNames = "orders")
    @Transactional(readOnly = true)
    public Order loadOrder(String id) {
        return null;
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(test_file),
        r#"package com.example.orders;

import org.springframework.cache.annotation.Cacheable;
import org.springframework.transaction.annotation.Transactional;

class OrderServiceTest {
    @Cacheable(cacheNames = "fixture-orders")
    @Transactional(readOnly = false)
    public Order fixtureOrder(String id) {
        return null;
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(migration_file),
        "CREATE TABLE orders (id bigint);\n",
    )
    .unwrap();
    std::fs::write(
        repo.join(test_migration_file),
        "CREATE TABLE fixture_orders (id bigint);\n",
    )
    .unwrap();
    run_git(
        &repo,
        &[
            "add",
            "pom.xml",
            service_file,
            test_file,
            migration_file,
            test_migration_file,
        ],
    );

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "loadOrder",
        "com.example.orders.OrderService.loadOrder",
        service_file,
        (8, 10),
        json!({
            "decorators": [
                "@Cacheable(cacheNames = \"orders\")",
                "@Transactional(readOnly = true)"
            ],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "fixtureOrder",
        "com.example.orders.OrderServiceTest.fixtureOrder",
        test_file,
        (8, 10),
        json!({
            "decorators": [
                "@Cacheable(cacheNames = \"fixture-orders\")",
                "@Transactional(readOnly = false)"
            ],
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let cache_names: BTreeSet<_> = synth
        .caches
        .iter()
        .map(|cache| cache.name.as_str())
        .collect();
    assert!(cache_names.contains("orders"));
    assert!(!cache_names.contains("fixture-orders"));

    let transaction_handlers: BTreeSet<_> = synth
        .transactions
        .iter()
        .flat_map(|tx| {
            tx.endpoints
                .iter()
                .map(|endpoint| endpoint.node_id.as_str())
        })
        .collect();
    assert!(transaction_handlers.contains("cbm:1:com.example.orders.OrderService.loadOrder"));
    assert!(
        !transaction_handlers.contains("cbm:2:com.example.orders.OrderServiceTest.fixtureOrder")
    );

    let table_names: BTreeSet<_> = synth
        .persistence
        .node_recs()
        .into_iter()
        .filter(|node| node.label == "Table")
        .filter_map(|node| {
            node.properties
                .get("tableName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(table_names.contains("orders"));
    assert!(!table_names.contains("fixture_orders"));
}

#[test]
fn synthesizes_java_transaction_boundaries() {
    let repo = temp_repo("java-transactions");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderService.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.transaction.annotation.Propagation;
import org.springframework.transaction.annotation.Transactional;

@Transactional(readOnly = true)
class OrderService {
    public Order loadOrder(String id) {
        return repo.findById(id).orElseThrow();
    }

    @Transactional(propagation = Propagation.REQUIRES_NEW, readOnly = false)
    public void submitOrder(Order order) {
        repo.save(order);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderService",
            "com.example.orders.OrderService",
            file,
        ),
        (6, 15),
        json!({
            "decorators": ["@Transactional(readOnly = true)"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "loadOrder",
        "com.example.orders.OrderService.loadOrder",
        file,
        (8, 10),
        json!({
            "language": "java",
            "parent_class": "OrderService",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "submitOrder",
        "com.example.orders.OrderService.submitOrder",
        file,
        (13, 15),
        json!({
            "decorators": ["@Transactional(propagation = Propagation.REQUIRES_NEW, readOnly = false)"],
            "language": "java",
            "parent_class": "OrderService",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let load_tx = synth
        .transactions
        .iter()
        .find(|tx| tx.name == "loadOrder transaction")
        .expect("class-level transaction should apply to method");
    assert_eq!(load_tx.manager, "spring-transaction");
    assert_eq!(load_tx.read_only, Some(true));
    assert_eq!(
        load_tx.endpoints[0].node_id,
        "cbm:2:com.example.orders.OrderService.loadOrder"
    );

    let submit_tx = synth
        .transactions
        .iter()
        .find(|tx| tx.name == "submitOrder transaction")
        .expect("method-level transaction");
    assert_eq!(submit_tx.propagation.as_deref(), Some("REQUIRES_NEW"));
    assert_eq!(submit_tx.read_only, Some(false));

    let edge_types: Vec<_> = synth
        .transactions
        .iter()
        .flat_map(SynthTransaction::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert_eq!(
        edge_types
            .iter()
            .filter(|edge| edge.as_str() == "HAS_TRANSACTION_BOUNDARY")
            .count(),
        2
    );
}

#[test]
fn synthesizes_java_transactions_from_source_annotations_without_metadata() {
    let repo = temp_repo("java-transaction-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderService.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.transaction.annotation.Propagation;
import org.springframework.transaction.annotation.Transactional;

@Transactional(readOnly = true)
class OrderService {
    public Order loadOrder(String id) {
        return repo.findById(id).orElseThrow();
    }

    @Transactional(
        propagation = Propagation.REQUIRES_NEW,
        readOnly = false)
    public void submitOrder(Order order) {
        repo.save(order);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderService",
            "com.example.orders.OrderService",
            file,
        ),
        (7, 18),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "loadOrder",
        "com.example.orders.OrderService.loadOrder",
        file,
        (8, 10),
        json!({
            "language": "java",
            "parent_class": "OrderService",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "submitOrder",
        "com.example.orders.OrderService.submitOrder",
        file,
        (15, 17),
        json!({
            "language": "java",
            "parent_class": "OrderService",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let load_tx = synth
        .transactions
        .iter()
        .find(|tx| tx.name == "loadOrder transaction")
        .expect("class-level source annotation transaction should apply to method");
    assert_eq!(load_tx.manager, "spring-transaction");
    assert_eq!(load_tx.read_only, Some(true));
    assert_eq!(
        load_tx.endpoints[0].node_id,
        "cbm:2:com.example.orders.OrderService.loadOrder"
    );
    assert!(load_tx.edge_recs().iter().any(|edge| {
        edge.evidence.as_ref().is_some_and(|evidence| {
            evidence["strategy"] == json!("java-spring-class-transactional")
        })
    }));

    let submit_tx = synth
        .transactions
        .iter()
        .find(|tx| tx.name == "submitOrder transaction")
        .expect("method-level source annotation transaction");
    assert_eq!(submit_tx.propagation.as_deref(), Some("REQUIRES_NEW"));
    assert_eq!(submit_tx.read_only, Some(false));
    assert_eq!(
        submit_tx.endpoints[0].node_id,
        "cbm:3:com.example.orders.OrderService.submitOrder"
    );
    assert!(submit_tx.edge_recs().iter().any(|edge| {
        edge.evidence
            .as_ref()
            .is_some_and(|evidence| evidence["strategy"] == json!("java-spring-transactional"))
    }));
}

#[test]
fn synthesizes_python_transaction_boundaries() {
    let repo = temp_repo("python-transactions");
    std::fs::write(
        repo.join("services.py"),
        r#"from django.db import transaction

@transaction.atomic
def submit_order(order):
    order.save()

def reconcile_order(order):
    with transaction.atomic():
        order.reconcile()
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "submit_order",
        "services.submit_order",
        "services.py",
        (4, 5),
        json!({
            "decorators": ["@transaction.atomic"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "reconcile_order",
        "services.reconcile_order",
        "services.py",
        (7, 9),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let submit_tx = synth
        .transactions
        .iter()
        .find(|tx| tx.name == "submit_order transaction")
        .expect("decorator transaction");
    assert_eq!(submit_tx.manager, "django-transaction");

    let reconcile_tx = synth
        .transactions
        .iter()
        .find(|tx| tx.name == "reconcile_order transaction")
        .expect("context manager transaction");
    assert_eq!(
        reconcile_tx.endpoints[0].node_id,
        "cbm:2:services.reconcile_order"
    );
}

#[test]
fn synthesizes_java_event_nodes() {
    let repo = temp_repo("java-events");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/events")).unwrap();
    let file = "src/main/java/com/example/events/OrderEvents.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.events;

import org.springframework.context.ApplicationEventPublisher;
import org.springframework.context.event.EventListener;

class OrderCreatedEvent {}

class OrderEvents {
    private ApplicationEventPublisher publisher;

    public void publish(String id) {
        publisher.publishEvent(new OrderCreatedEvent());
    }

    @EventListener(OrderCreatedEvent.class)
    public void onCreated(OrderCreatedEvent event) {}
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "publish",
        "com.example.events.OrderEvents.publish",
        file,
        (11, 13),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "onCreated",
        "com.example.events.OrderEvents.onCreated",
        file,
        (16, 16),
        json!({
            "decorators": ["@EventListener(OrderCreatedEvent.class)"],
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let event = synth
        .events
        .iter()
        .find(|event| event.name == "OrderCreatedEvent")
        .expect("spring event");
    assert_eq!(event.bus, "spring-application-event");
    assert_eq!(event.publishers.len(), 1);
    assert_eq!(event.handlers.len(), 1);
    let edge_types: Vec<_> = event
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"PUBLISHES_EVENT".to_string()));
    assert!(edge_types.contains(&"HANDLES_EVENT".to_string()));
}

#[test]
fn synthesizes_python_signal_event_nodes() {
    let repo = temp_repo("python-events");
    std::fs::write(
        repo.join("signals.py"),
        r#"from django.dispatch import Signal, receiver

order_created = Signal()

def publish_order(order_id):
    order_created.send(sender=None, order_id=order_id)

@receiver(order_created)
def handle_order(sender, **kwargs):
    return kwargs
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "publish_order",
        "signals.publish_order",
        "signals.py",
        (5, 6),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "handle_order",
        "signals.handle_order",
        "signals.py",
        (9, 10),
        json!({
            "decorators": ["@receiver(order_created)"],
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let event = synth
        .events
        .iter()
        .find(|event| event.name == "order_created")
        .expect("python signal");
    assert_eq!(event.bus, "python-signal");
    assert_eq!(event.publishers.len(), 1);
    assert_eq!(event.handlers.len(), 1);
    let edge_types: Vec<_> = event
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"PUBLISHES_EVENT".to_string()));
    assert!(edge_types.contains(&"HANDLES_EVENT".to_string()));
}

#[test]
fn synthesizes_java_policy_nodes() {
    let repo = temp_repo("java-policies");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/security")).unwrap();
    let file = "src/main/java/com/example/security/OrderController.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.security;

import org.springframework.security.access.prepost.PreAuthorize;
import org.springframework.security.access.annotation.Secured;

class OrderController {
    @PreAuthorize("hasAuthority('orders:read')")
    public String list() { return "ok"; }

    @Secured({"ROLE_ADMIN"})
    public void delete() {}
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "list",
        "com.example.security.OrderController.list",
        file,
        (7, 8),
        json!({
            "decorators": ["@PreAuthorize(\"hasAuthority('orders:read')\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "delete",
        "com.example.security.OrderController.delete",
        file,
        (10, 11),
        json!({
            "decorators": ["@Secured({\"ROLE_ADMIN\"})"],
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let preauth = synth
        .policies
        .iter()
        .find(|policy| policy.name == "hasAuthority('orders:read')")
        .expect("spring preauthorize policy");
    assert_eq!(preauth.policy_type, "spring-expression");
    assert_eq!(preauth.subjects.len(), 1);
    let role = synth
        .policies
        .iter()
        .find(|policy| policy.name == "ROLE_ADMIN")
        .expect("secured role policy");
    assert_eq!(role.policy_type, "role");
    let edge_types: Vec<_> = synth
        .policies
        .iter()
        .flat_map(SynthPolicy::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"REQUIRES_POLICY".to_string()));
}

#[test]
fn synthesizes_java_policies_from_source_annotations_without_metadata() {
    let repo = temp_repo("java-policy-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/security")).unwrap();
    let file = "src/main/java/com/example/security/OrderController.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.security;

import jakarta.annotation.security.RolesAllowed;
import org.springframework.security.access.prepost.PreAuthorize;

@PreAuthorize("hasAuthority('orders:read')")
class OrderController {
    public String list() { return "ok"; }

    @RolesAllowed({
        "ROLE_ADMIN",
        "ROLE_SUPPORT"})
    public void delete() {}
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderController",
            "com.example.security.OrderController",
            file,
        ),
        (7, 14),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "list",
        "com.example.security.OrderController.list",
        file,
        (8, 8),
        json!({
            "language": "java",
            "parent_class": "OrderController",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "delete",
        "com.example.security.OrderController.delete",
        file,
        (13, 13),
        json!({
            "language": "java",
            "parent_class": "OrderController",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let preauth = synth
        .policies
        .iter()
        .find(|policy| policy.name == "hasAuthority('orders:read')")
        .expect("class-level source annotation policy");
    assert_eq!(preauth.policy_type, "spring-expression");
    assert_eq!(preauth.subjects.len(), 1);
    assert!(preauth
        .edge_recs()
        .iter()
        .any(|edge| edge.source_id == "cbm:1:com.example.security.OrderController"));

    for role_name in ["ROLE_ADMIN", "ROLE_SUPPORT"] {
        let role = synth
            .policies
            .iter()
            .find(|policy| policy.name == role_name)
            .expect("method-level source annotation role");
        assert_eq!(role.policy_type, "role");
        assert!(role
            .edge_recs()
            .iter()
            .any(|edge| edge.source_id == "cbm:3:com.example.security.OrderController.delete"));
    }
}

#[test]
fn synthesizes_python_policy_nodes() {
    let repo = temp_repo("python-policies");
    std::fs::write(
        repo.join("api.py"),
        r#"from django.contrib.auth.decorators import permission_required
from fastapi import Security

def require_user():
    return True

@permission_required("orders.view_order")
def list_orders(request):
    return []

def read_order(user=Security(require_user, scopes=["orders:read"])):
    return {}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "list_orders",
        "api.list_orders",
        "api.py",
        (8, 9),
        json!({
            "decorators": ["@permission_required(\"orders.view_order\")"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "read_order",
        "api.read_order",
        "api.py",
        (11, 12),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let permission = synth
        .policies
        .iter()
        .find(|policy| policy.name == "orders.view_order")
        .expect("django permission policy");
    assert_eq!(permission.policy_type, "permission");
    let dependency = synth
        .policies
        .iter()
        .find(|policy| policy.name == "require_user")
        .expect("fastapi security dependency");
    assert_eq!(dependency.policy_type, "dependency");
    let scope = synth
        .policies
        .iter()
        .find(|policy| policy.name == "orders:read")
        .expect("fastapi security scope");
    assert_eq!(scope.policy_type, "scope");
}

#[test]
fn exports_gitnexus_compatible_semantic_edges() {
    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Class",
        "OrderService",
        "pkg.OrderService",
        "src/OrderService.java",
    );
    insert_node(
        &conn,
        2,
        "Method",
        "save",
        "pkg.OrderService.save",
        "src/OrderService.java",
    );
    insert_node(
        &conn,
        3,
        "Field",
        "repo",
        "pkg.OrderService.repo",
        "src/OrderService.java",
    );
    insert_node(
        &conn,
        4,
        "Interface",
        "CrudService",
        "pkg.CrudService",
        "src/CrudService.java",
    );
    insert_node(
        &conn,
        5,
        "Method",
        "save",
        "pkg.CrudService.save",
        "src/CrudService.java",
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_edge(&conn, 2, 1, 4, "INHERITS");
    insert_edge(&conn, 3, 2, 3, "USAGE");

    let edge_types = exported_edge_types(&conn);
    assert!(edge_types.contains(&"HAS_METHOD".to_string()));
    assert!(edge_types.contains(&"HAS_PROPERTY".to_string()));
    assert!(edge_types.contains(&"IMPLEMENTS".to_string()));
    assert!(edge_types.contains(&"METHOD_IMPLEMENTS".to_string()));
    assert!(edge_types.contains(&"ACCESSES".to_string()));
}
