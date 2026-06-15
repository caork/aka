use super::*;
use std::collections::BTreeSet;

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
fn synthesizes_route_nodes_consumers_and_entry_flows() {
    let repo = temp_repo("routes");
    std::fs::create_dir_all(repo.join("src/pages/api/config")).unwrap();
    std::fs::create_dir_all(repo.join("src/components")).unwrap();
    std::fs::write(
            repo.join("src/pages/api/config/route.ts"),
            "export async function GET() { return Response.json({ data: [], pagination: {}, error: null }); }",
        )
        .unwrap();
    std::fs::write(
            repo.join("src/components/config-panel.tsx"),
            "export async function ConfigPanel() { const res = await fetch('/api/config'); const data = await res.json(); return data.pagination.total + data.missing; }",
        )
        .unwrap();

    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Function",
        "GET",
        "src/pages/api/config/route.ts::GET",
        "src/pages/api/config/route.ts",
    );
    insert_node(
        &conn,
        2,
        "Function",
        "loadConfig",
        "src/pages/api/config/route.ts::loadConfig",
        "src/pages/api/config/route.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "ConfigPanel",
        "src/components/config-panel.tsx::ConfigPanel",
        "src/components/config-panel.tsx",
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_eq!(synth.routes.len(), 1);
    let route = &synth.routes[0];
    assert_eq!(route.route, "/api/config");
    assert!(route.response_keys.contains(&"data".to_string()));
    assert!(route.response_keys.contains(&"pagination".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
    assert_eq!(route.consumers.len(), 1);
    assert_eq!(route.consumers[0].fetch_count, 1);
    assert!(route.consumers[0].keys.contains(&"pagination".to_string()));

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"HANDLES_ROUTE".to_string()));
    assert!(edge_types.contains(&"FETCHES".to_string()));
}

#[test]
fn links_python_requests_consumers_to_routes() {
    let repo = temp_repo("python-route-consumers");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("workers")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/api/orders")

@router.get("/{id}")
def get_order(id: str):
    return {"id": id, "status": "ok"}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("workers/sync.py"),
        r#"import requests

def sync_order(order_id: str):
    response = requests.get(f"http://orders.internal/api/orders/{order_id}")
    data = response.json()
    return data["status"]
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
    insert_node_props(
        &conn,
        2,
        "Function",
        "sync_order",
        "workers.sync.sync_order",
        "workers/sync.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("parameterized FastAPI route");
    assert_eq!(route.consumers.len(), 1);
    assert_eq!(route.consumers[0].node_id, "cbm:2:workers.sync.sync_order");
    assert!(route.consumers[0].keys.contains(&"status".to_string()));

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"FETCHES".to_string()));
}

#[test]
fn links_python_httpx_client_consumers_to_routes() {
    let repo = temp_repo("python-httpx-route-consumers");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("workers")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/api/orders")

@router.get("/{id}")
def get_order(id: str):
    return {"id": id, "status": "ok"}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("workers/sync.py"),
        r#"import httpx

async def sync_order(order_id: str):
    async with httpx.AsyncClient() as client:
        response = await client.get(f"http://orders.internal/api/orders/{order_id}")
        data = response.json()
        return data["status"]
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
    insert_node_props(
        &conn,
        2,
        "Function",
        "sync_order",
        "workers.sync.sync_order",
        "workers/sync.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("parameterized FastAPI route");
    assert_eq!(route.consumers.len(), 1);
    assert_eq!(route.consumers[0].node_id, "cbm:2:workers.sync.sync_order");
    assert!(route.consumers[0].keys.contains(&"status".to_string()));
}

#[test]
fn synthesizes_spring_routes_with_class_prefix() {
    let repo = temp_repo("spring-routes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public String getOrder(String id) {
        return id;
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Class",
        "OrderController",
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@RestController", "@RequestMapping(\"/api/orders\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "getOrder",
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring route with class prefix");
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:2:com.example.orders.OrderController.getOrder")
    );

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"HANDLES_ROUTE".to_string()));
}

#[test]
fn synthesizes_spring_functional_router_routes() {
    let repo = temp_repo("spring-functional-routes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderRoutes.java"),
        r#"package com.example.orders;

import static org.springframework.web.reactive.function.server.RequestPredicates.GET;
import static org.springframework.web.reactive.function.server.RequestPredicates.path;
import static org.springframework.web.reactive.function.server.RequestPredicates.POST;
import static org.springframework.web.reactive.function.server.RouterFunctions.nest;
import static org.springframework.web.reactive.function.server.RouterFunctions.route;

import org.springframework.context.annotation.Bean;
import org.springframework.web.reactive.function.server.RouterFunction;
import org.springframework.web.reactive.function.server.ServerResponse;

class OrderRoutes {
    @Bean
    RouterFunction<ServerResponse> routes(OrderHandler handler) {
        return nest(path("/api"),
            route(GET("/orders/{id}"), handler::getOrder)
                .andRoute(POST("/orders"), handler::createOrder));
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderHandler.java"),
        r#"package com.example.orders;

class OrderHandler {
    ServerResponse getOrder(ServerRequest request) {
        return ServerResponse.ok().build();
    }

    ServerResponse createOrder(ServerRequest request) {
        return ServerResponse.ok().build();
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "getOrder",
        "com.example.orders.OrderHandler.getOrder",
        "src/main/java/com/example/orders/OrderHandler.java",
        json!({"language": "java"}),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "createOrder",
        "com.example.orders.OrderHandler.createOrder",
        "src/main/java/com/example/orders/OrderHandler.java",
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let get = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring functional GET route");
    assert_eq!(get.method.as_deref(), Some("GET"));
    assert_eq!(
        get.handler_id.as_deref(),
        Some("cbm:1:com.example.orders.OrderHandler.getOrder")
    );
    let post = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders")
        .expect("spring functional POST route");
    assert_eq!(post.method.as_deref(), Some("POST"));
    assert_eq!(
        post.handler_id.as_deref(),
        Some("cbm:2:com.example.orders.OrderHandler.createOrder")
    );
}

#[test]
fn links_java_http_consumers_to_spring_routes() {
    let repo = temp_repo("java-route-consumers");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/java/com/example/workers")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public OrderDto getOrder(String id) {
        return new OrderDto(id, "ok");
    }
}"#,
    )
    .unwrap();
    std::fs::write(
            repo.join("src/main/java/com/example/workers/OrderWorker.java"),
            r#"package com.example.workers;

import org.springframework.web.client.RestTemplate;

public class OrderWorker {
    private final RestTemplate restTemplate = new RestTemplate();

    public String syncOrder(String id) {
        OrderDto order = restTemplate.getForObject("http://orders/api/orders/" + id, OrderDto.class);
        return order.status();
    }
}"#,
        )
        .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Class",
        "OrderController",
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@RestController", "@RequestMapping(\"/api/orders\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "getOrder",
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props(
        &conn,
        3,
        "Method",
        "syncOrder",
        "com.example.workers.OrderWorker.syncOrder",
        "src/main/java/com/example/workers/OrderWorker.java",
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring route with class prefix");
    let parent_route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders")
        .expect("spring class prefix route");
    assert!(
        parent_route.consumers.is_empty(),
        "parameterized detail calls should not also attach to parent collection routes"
    );
    assert_eq!(route.consumers.len(), 1);
    assert_eq!(
        route.consumers[0].node_id,
        "cbm:3:com.example.workers.OrderWorker.syncOrder"
    );
    assert!(route.consumers[0].keys.contains(&"status".to_string()));

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"FETCHES".to_string()));
}

#[test]
fn links_java_builder_http_consumers_to_spring_routes() {
    let repo = temp_repo("java-builder-route-consumers");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/java/com/example/workers")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public OrderDto getOrder(String id) {
        return new OrderDto(id, "ok");
    }
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/workers/OrderWorker.java"),
        r#"package com.example.workers;

import java.net.URI;

public class OrderWorker {
    public String syncOrder(String id) {
        var request = java.net.http.HttpRequest.newBuilder()
            .uri(URI.create("http://orders/api/orders/" + id))
            .build();
        OrderDto order = send(request);
        return order.getStatus();
    }

    public String syncOrderWithOkHttp(String id) {
        var request = new okhttp3.Request.Builder()
            .url("http://orders/api/orders/" + id)
            .build();
        OrderDto order = execute(request);
        return order.status();
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Class",
        "OrderController",
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@RestController", "@RequestMapping(\"/api/orders\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "getOrder",
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "syncOrder",
            "com.example.workers.OrderWorker.syncOrder",
            "src/main/java/com/example/workers/OrderWorker.java",
        ),
        (6, 12),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        4,
        (
            "Method",
            "syncOrderWithOkHttp",
            "com.example.workers.OrderWorker.syncOrderWithOkHttp",
            "src/main/java/com/example/workers/OrderWorker.java",
        ),
        (14, 20),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring route with class prefix");
    let consumers: BTreeSet<_> = route
        .consumers
        .iter()
        .map(|consumer| consumer.node_id.as_str())
        .collect();
    assert!(consumers.contains("cbm:3:com.example.workers.OrderWorker.syncOrder"));
    assert!(consumers.contains("cbm:4:com.example.workers.OrderWorker.syncOrderWithOkHttp"));
    assert!(route.consumers.iter().any(|consumer| {
        consumer
            .keys
            .iter()
            .any(|key| key == "getStatus" || key == "status")
    }));
}

#[test]
fn links_java_feign_consumers_to_spring_routes() {
    let repo = temp_repo("java-feign-consumers");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public OrderDto getOrder(String id) {
        return new OrderDto(id, "ok");
    }
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderClient.java"),
        r#"package com.example.orders;

import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.GetMapping;

@FeignClient(name = "orders", path = "/api/orders")
public interface OrderClient {
    @GetMapping("/{id}")
    OrderDto getOrder(String id);
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Class",
        "OrderController",
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@RestController", "@RequestMapping(\"/api/orders\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "getOrder",
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props(
        &conn,
        3,
        "Interface",
        "OrderClient",
        "com.example.orders.OrderClient",
        "src/main/java/com/example/orders/OrderClient.java",
        json!({
            "decorators": ["@FeignClient(name = \"orders\", path = \"/api/orders\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        4,
        "Method",
        "getOrder",
        "com.example.orders.OrderClient.getOrder",
        "src/main/java/com/example/orders/OrderClient.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:3:com.example.orders.OrderClient",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 2, 3, 4, "DEFINES_METHOD");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| {
            route.route == "/api/orders/{id}"
                && route.handler_id.as_deref()
                    == Some("cbm:2:com.example.orders.OrderController.getOrder")
        })
        .expect("spring provider route");
    assert!(route
        .consumers
        .iter()
        .any(|consumer| consumer.node_id == "cbm:4:com.example.orders.OrderClient.getOrder"));
}

#[test]
fn inherits_spring_routes_from_controller_interfaces() {
    let repo = temp_repo("java-interface-routes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderApi.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;

@RequestMapping("/api/orders")
public interface OrderApi {
    @GetMapping("/{id}")
    OrderDto getOrder(String id);
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.RestController;

@RestController
public class OrderController implements OrderApi {
    @Override
    public OrderDto getOrder(String id) {
        return new OrderDto(id, "ok");
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Interface",
        "OrderApi",
        "com.example.orders.OrderApi",
        "src/main/java/com/example/orders/OrderApi.java",
        json!({
            "decorators": ["@RequestMapping(\"/api/orders\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "getOrder",
        "com.example.orders.OrderApi.getOrder",
        "src/main/java/com/example/orders/OrderApi.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderApi",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props(
        &conn,
        3,
        "Class",
        "OrderController",
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@RestController"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        4,
        "Method",
        "getOrder",
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@Override"],
            "language": "java",
            "parent_class": "cbm:3:com.example.orders.OrderController",
        }),
    );
    insert_edge(&conn, 2, 3, 4, "DEFINES_METHOD");
    insert_edge(&conn, 3, 3, 1, "IMPLEMENTS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| {
            route.route == "/api/orders/{id}"
                && route.handler_id.as_deref()
                    == Some("cbm:4:com.example.orders.OrderController.getOrder")
        })
        .expect("controller implementation should inherit interface route");
    assert_eq!(route.method.as_deref(), Some("GET"));

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"HANDLES_ROUTE".to_string()));
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
        synth.routes.iter().all(|route| {
            route.route != "/orders/{id}" && route.route != "/v1/orders/{id}"
        }),
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
        ("Class", "OrderViewSet", "orders.views.OrderViewSet", "orders/views.py"),
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

    let prefixes = python_router_prefixes_by_file(&repo, ["api/orders.py"].into_iter());
    let file = prefixes.get("api/orders.py").expect("file prefixes");
    assert_eq!(
        file.local_by_router.get("router").map(String::as_str),
        Some("/api/orders")
    );
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
        &["add", "src/main/java/com/example/App.java", "application.yml"],
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
fn reads_cbm_file_hashes_rel_path_column() {
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

    let exts = indexed_source_extensions(&conn, "demo").unwrap();
    assert!(exts.contains("java"));
}

#[test]
fn synthesizes_java_message_topics() {
    let repo = temp_repo("java-message-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderEvents.java"),
        r#"package com.example.orders;

import org.springframework.kafka.annotation.KafkaListener;

class OrderEvents {
    @KafkaListener(topics = "orders.created")
    public void onCreated(String payload) {}

    @KafkaListener("orders.updated")
    public void onUpdated(String payload) {}

    public void publish(Object payload) {
        kafkaTemplate.send("orders.created", payload);
    }
}"#,
    )
    .unwrap();

    let file = "src/main/java/com/example/orders/OrderEvents.java";
    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "onCreated",
        "com.example.orders.OrderEvents.onCreated",
        file,
        json!({
            "decorators": ["@KafkaListener(topics = \"orders.created\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "onUpdated",
        "com.example.orders.OrderEvents.onUpdated",
        file,
        json!({
            "decorators": ["@KafkaListener(\"orders.updated\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "publish",
        "com.example.orders.OrderEvents.publish",
        file,
        (11, 13),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let created = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("orders.created topic");
    assert_eq!(created.broker, "kafka");
    assert_eq!(created.consumers.len(), 1);
    assert_eq!(created.producers.len(), 1);
    assert_eq!(
        created.consumers[0].node_id,
        "cbm:1:com.example.orders.OrderEvents.onCreated"
    );
    assert_eq!(
        created.producers[0].node_id,
        "cbm:3:com.example.orders.OrderEvents.publish"
    );

    let updated = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.updated")
        .expect("orders.updated positional listener topic");
    assert_eq!(updated.broker, "kafka");
    assert_eq!(updated.consumers.len(), 1);

    let edge_types: Vec<_> = created
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"CONSUMES_TOPIC".to_string()));
    assert!(edge_types.contains(&"PUBLISHES_TOPIC".to_string()));
}

#[test]
fn synthesizes_spring_rabbit_topics_from_routing_keys() {
    let repo = temp_repo("java-rabbit-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/RabbitEvents.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.amqp.rabbit.annotation.RabbitListener;
import org.springframework.amqp.rabbit.annotation.QueueBinding;
import org.springframework.amqp.rabbit.annotation.Queue;
import org.springframework.amqp.rabbit.annotation.Exchange;

class RabbitEvents {
    @RabbitListener(queues = "orders.created")
    public void consumeQueue(String payload) {}

    @RabbitListener(bindings = @QueueBinding(
        value = @Queue("orders.created"),
        exchange = @Exchange("orders.exchange"),
        key = "orders.shipped"))
    public void consumeBinding(String payload) {}

    public void publish(Object event) {
        rabbitTemplate.convertAndSend("orders.exchange", "orders.created", event);
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "consumeQueue",
        "com.example.orders.RabbitEvents.consumeQueue",
        file,
        json!({
            "decorators": ["@RabbitListener(queues = \"orders.created\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "consumeBinding",
        "com.example.orders.RabbitEvents.consumeBinding",
        file,
        json!({
            "decorators": ["@RabbitListener(bindings = @QueueBinding(value = @Queue(\"orders.created\"), exchange = @Exchange(\"orders.exchange\"), key = \"orders.shipped\"))"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "publish",
        "com.example.orders.RabbitEvents.publish",
        file,
        (17, 19),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let created = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("orders.created rabbit topic");
    assert_eq!(created.broker, "rabbitmq");
    assert_eq!(created.consumers.len(), 2);
    assert_eq!(created.producers.len(), 1);
    assert_eq!(
        created.producers[0].node_id,
        "cbm:3:com.example.orders.RabbitEvents.publish"
    );
    assert!(!synth
        .topics
        .iter()
        .any(|topic| topic.name == "orders.exchange"));

    let shipped = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.shipped")
        .expect("orders.shipped binding key topic");
    assert_eq!(shipped.broker, "rabbitmq");
    assert_eq!(shipped.consumers.len(), 1);
}

#[test]
fn synthesizes_spring_jms_topics() {
    let repo = temp_repo("java-jms-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/JmsEvents.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.jms.annotation.JmsListener;

class JmsEvents {
    @JmsListener(destination = "orders.created")
    public void consume(String payload) {}

    public void publish(Object event) {
        jmsTemplate.convertAndSend("orders.created", event);
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "consume",
        "com.example.orders.JmsEvents.consume",
        file,
        json!({
            "decorators": ["@JmsListener(destination = \"orders.created\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "publish",
        "com.example.orders.JmsEvents.publish",
        file,
        (9, 11),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("orders.created jms topic");
    assert_eq!(topic.broker, "jms");
    assert_eq!(topic.consumers.len(), 1);
    assert_eq!(topic.producers.len(), 1);
}

#[test]
fn synthesizes_python_message_topics() {
    let repo = temp_repo("python-message-topics");
    std::fs::write(
        repo.join("events.py"),
        r#"from kafka import KafkaConsumer

def consume():
    consumer = KafkaConsumer("orders.created")
    return consumer

def publish(producer):
    producer.send("orders.created", b"{}")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "consume",
        "events.consume",
        "events.py",
        (3, 5),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "publish",
        "events.publish",
        "events.py",
        (7, 8),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("orders.created topic");
    assert_eq!(topic.broker, "kafka");
    assert_eq!(topic.consumers.len(), 1);
    assert_eq!(topic.producers.len(), 1);
    assert_eq!(topic.consumers[0].node_id, "cbm:1:events.consume");
    assert_eq!(topic.producers[0].node_id, "cbm:2:events.publish");

    let edge_types: Vec<_> = topic
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"CONSUMES_TOPIC".to_string()));
    assert!(edge_types.contains(&"PUBLISHES_TOPIC".to_string()));
}

#[test]
fn synthesizes_spring_scheduled_jobs() {
    let repo = temp_repo("spring-scheduled-jobs");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/jobs")).unwrap();
    let file = "src/main/java/com/example/jobs/BillingJobs.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.jobs;

import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.scheduling.annotation.Async;

class BillingJobs {
    @Scheduled(cron = "0 0 * * * *")
    public void settleInvoices() {
        settleOpenInvoices();
    }

    void settleOpenInvoices() {
        writeLedger();
    }

    void writeLedger() {}
}

class BillingController {
    private BillingJobs jobs;

    void submitInvoice() {
        jobs.rebuildInvoiceCache();
    }
}

class AsyncBillingJobs {
    @Async
    void rebuildInvoiceCache() {}
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "settleInvoices",
        "com.example.jobs.BillingJobs.settleInvoices",
        file,
        (7, 9),
        json!({
            "decorators": ["@Scheduled(cron = \"0 0 * * * *\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "settleOpenInvoices",
        "com.example.jobs.BillingJobs.settleOpenInvoices",
        file,
        (11, 13),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "writeLedger",
        "com.example.jobs.BillingJobs.writeLedger",
        file,
        (15, 15),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "submitInvoice",
        "com.example.jobs.BillingController.submitInvoice",
        file,
        (21, 23),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "rebuildInvoiceCache",
        "com.example.jobs.AsyncBillingJobs.rebuildInvoiceCache",
        file,
        (28, 29),
        json!({
            "decorators": ["@Async"],
            "language": "java",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let job = synth
        .jobs
        .iter()
        .find(|job| job.handler_id == "cbm:1:com.example.jobs.BillingJobs.settleInvoices")
        .expect("spring scheduled job");
    assert_eq!(job.job_type, "spring-scheduled");
    assert_eq!(job.schedule.as_deref(), Some("cron=0 0 * * * *"));
    assert_eq!(job.process_ids.len(), 1);

    let edge_types: Vec<_> = job
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"HANDLES_JOB".to_string()));
    assert!(edge_types.contains(&"ENTRY_POINT_OF".to_string()));

    let async_job = synth
        .jobs
        .iter()
        .find(|job| job.handler_id == "cbm:5:com.example.jobs.AsyncBillingJobs.rebuildInvoiceCache")
        .expect("spring async job");
    assert_eq!(async_job.job_type, "spring-async");
    assert!(async_job.edge_recs().iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB"
            && edge.source_id == "cbm:4:com.example.jobs.BillingController.submitInvoice"
    }));
}

#[test]
fn synthesizes_python_task_jobs() {
    let repo = temp_repo("python-task-jobs");
    std::fs::write(
        repo.join("tasks.py"),
        r#"from celery import shared_task
from apscheduler.schedulers.background import BackgroundScheduler

@shared_task(name="orders.sync")
def sync_orders():
    load_orders()

def load_orders():
    write_orders()

def write_orders():
    return []

scheduler = BackgroundScheduler()

@scheduler.scheduled_job("cron", id="orders.cleanup", hour="3")
def cleanup_orders():
    return None

def enqueue_orders():
    sync_orders.delay()
    app.send_task("orders.cleanup")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "sync_orders",
        "tasks.sync_orders",
        "tasks.py",
        (4, 6),
        json!({
            "decorators": ["@shared_task(name=\"orders.sync\")"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_orders",
        "tasks.load_orders",
        "tasks.py",
        (8, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "write_orders",
        "tasks.write_orders",
        "tasks.py",
        (11, 12),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "cleanup_orders",
        "tasks.cleanup_orders",
        "tasks.py",
        (17, 18),
        json!({
            "decorators": ["@scheduler.scheduled_job(\"cron\", id=\"orders.cleanup\", hour=\"3\")"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "enqueue_orders",
        "tasks.enqueue_orders",
        "tasks.py",
        (20, 22),
        json!({
            "language": "python",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let celery = synth
        .jobs
        .iter()
        .find(|job| job.name == "orders.sync")
        .expect("celery task job");
    assert_eq!(celery.job_type, "celery-task");
    assert_eq!(celery.handler_id, "cbm:1:tasks.sync_orders");
    assert_eq!(celery.process_ids.len(), 1);

    let aps = synth
        .jobs
        .iter()
        .find(|job| job.name == "orders.cleanup")
        .expect("apscheduler job");
    assert_eq!(aps.job_type, "apscheduler-job");
    assert!(aps
        .schedule
        .as_deref()
        .is_some_and(|schedule| schedule.contains("trigger=cron") && schedule.contains("hour=3")));
    assert!(celery.edge_recs().iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB" && edge.source_id == "cbm:5:tasks.enqueue_orders"
    }));
    assert!(aps.edge_recs().iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB" && edge.source_id == "cbm:5:tasks.enqueue_orders"
    }));
}

#[test]
fn synthesizes_python_dramatiq_jobs() {
    let repo = temp_repo("python-dramatiq-jobs");
    std::fs::write(
        repo.join("actors.py"),
        r#"import dramatiq

@dramatiq.actor(actor_name="orders.rebuild", queue_name="orders")
def rebuild_orders(order_id):
    return order_id

def enqueue_orders(order_id):
    rebuild_orders.send(order_id)
    rebuild_orders.send_with_options(args=(order_id,), delay=1000)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "rebuild_orders",
        "actors.rebuild_orders",
        "actors.py",
        (4, 5),
        json!({
            "decorators": ["@dramatiq.actor(actor_name=\"orders.rebuild\", queue_name=\"orders\")"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "enqueue_orders",
        "actors.enqueue_orders",
        "actors.py",
        (7, 9),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let job = synth
        .jobs
        .iter()
        .find(|job| job.name == "orders.rebuild")
        .expect("dramatiq actor job");
    assert_eq!(job.job_type, "dramatiq-actor");
    assert_eq!(job.handler_id, "cbm:1:actors.rebuild_orders");
    let edges = job.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "HANDLES_JOB" && edge.source_id == "cbm:1:actors.rebuild_orders"
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB" && edge.source_id == "cbm:2:actors.enqueue_orders"
    }));
}

#[test]
fn synthesizes_jvm_command_entrypoints() {
    let repo = temp_repo("jvm-commands");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    let file = "src/main/java/com/example/ops/ReindexCommand.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;
import org.springframework.stereotype.Component;
import picocli.CommandLine.Command;

@Component
class ReindexOrders implements ApplicationRunner {
    @Override
    public void run(ApplicationArguments args) {
        rebuildOrders();
    }
}

class RunnerConfig {
    @Bean
    CommandLineRunner syncRunner() {
        return args -> rebuildOrders();
    }
}

@Command(name = "orders-reindex", aliases = {"orders-sync"})
class ReindexCli implements Runnable {
    public void run() {
        rebuildOrders();
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
            "ReindexOrders",
            "com.example.ops.ReindexOrders",
            file,
        ),
        (8, 13),
        json!({
            "language": "java",
            "decorators": ["@Component"],
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Method", "run", "com.example.ops.ReindexOrders.run", file),
        (10, 12),
        json!({
            "language": "java",
            "decorators": ["@Override"],
            "parent_class": "com.example.ops.ReindexOrders",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "syncRunner",
            "com.example.ops.RunnerConfig.syncRunner",
            file,
        ),
        (17, 20),
        json!({
            "language": "java",
            "decorators": ["@Bean"],
            "parent_class": "com.example.ops.RunnerConfig",
        }),
    );
    insert_node_props_at(
        &conn,
        4,
        ("Class", "ReindexCli", "com.example.ops.ReindexCli", file),
        (23, 28),
        json!({
            "language": "java",
            "decorators": ["@Command(name = \"orders-reindex\", aliases = {\"orders-sync\"})"],
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let spring = synth
        .commands
        .iter()
        .find(|command| command.command_type == "spring-runner")
        .expect("spring runner command");
    assert_eq!(spring.handler_id, "cbm:1:com.example.ops.ReindexOrders");
    assert_eq!(spring.strategy, "java-spring-runner-source-declaration");
    let picocli = synth
        .commands
        .iter()
        .find(|command| command.command_type == "picocli-command")
        .expect("picocli command");
    assert_eq!(picocli.name, "orders-reindex");
    assert_eq!(picocli.handler_id, "cbm:4:com.example.ops.ReindexCli");
    assert!(synth
        .commands
        .iter()
        .any(|command| command.handler_id == "cbm:3:com.example.ops.RunnerConfig.syncRunner"));

    let edge_types: Vec<_> = synth
        .commands
        .iter()
        .flat_map(SynthCommand::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"HANDLES_COMMAND".to_string()));
}

#[test]
fn spring_runner_detection_uses_source_facts_not_class_names() {
    let repo = temp_repo("spring-runner-source-facts");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    let file = "src/main/java/com/example/ops/StartupMaintenance.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;
import org.springframework.context.annotation.Bean;

class StartupMaintenance implements ApplicationRunner {
    public void run(ApplicationArguments args) {
        warmCache();
    }
}

class MaintenanceConfiguration {
    @Bean
    public org.springframework.boot.CommandLineRunner repairOrders() {
        return args -> {};
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
            "StartupMaintenance",
            "com.example.ops.StartupMaintenance",
            file,
        ),
        (7, 11),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "repairOrders",
            "com.example.ops.MaintenanceConfiguration.repairOrders",
            file,
        ),
        (15, 17),
        json!({
            "language": "java",
            "decorators": ["@Bean"],
            "parent_class": "com.example.ops.MaintenanceConfiguration",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let handlers: BTreeSet<_> = synth
        .commands
        .iter()
        .filter(|command| command.command_type == "spring-runner")
        .map(|command| command.handler_id.as_str())
        .collect();
    assert!(handlers.contains("cbm:1:com.example.ops.StartupMaintenance"));
    assert!(handlers.contains("cbm:2:com.example.ops.MaintenanceConfiguration.repairOrders"));
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
fn ignores_test_source_command_entrypoints() {
    let repo = temp_repo("test-source-commands");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/test/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let file = "src/test/java/com/example/ops/TestCommand.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import org.springframework.boot.CommandLineRunner;

class TestCommand implements CommandLineRunner {
    public void run(String... args) {
        rebuildFixtures();
    }
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "TestCommand", "com.example.ops.TestCommand", file),
        (5, 9),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.commands.is_empty());
}

#[test]
fn command_synthesis_uses_project_sources_not_runner_names() {
    let repo = temp_repo("git-project-command-sources");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let tracked_file = "src/main/java/com/example/ops/StartupMaintenance.java";
    let untracked_file = "src/main/java/com/example/ops/UntrackedMaintenance.java";
    let test_file = "src/test/java/com/example/ops/TestMaintenance.java";
    std::fs::write(
        repo.join(tracked_file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;

class StartupMaintenance implements ApplicationRunner {
    public void run(ApplicationArguments args) {
        warmCache();
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(untracked_file),
        r#"package com.example.ops;

import org.springframework.boot.CommandLineRunner;

class UntrackedMaintenance implements CommandLineRunner {
    public void run(String... args) {
        repairOrders();
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(test_file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;

class TestMaintenance implements ApplicationRunner {
    public void run(ApplicationArguments args) {
        resetFixtures();
    }
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", tracked_file, test_file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "StartupMaintenance",
            "com.example.ops.StartupMaintenance",
            tracked_file,
        ),
        (6, 10),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Class",
            "UntrackedMaintenance",
            "com.example.ops.UntrackedMaintenance",
            untracked_file,
        ),
        (5, 9),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Class",
            "TestMaintenance",
            "com.example.ops.TestMaintenance",
            test_file,
        ),
        (6, 10),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let handlers: Vec<_> = synth
        .commands
        .iter()
        .map(|command| command.handler_name.as_str())
        .collect();
    assert!(handlers.contains(&"StartupMaintenance"));
    assert!(handlers.contains(&"UntrackedMaintenance"));
    assert!(!handlers.contains(&"TestMaintenance"));
}

#[test]
fn command_synthesis_excludes_build_configured_test_roots() {
    let repo = temp_repo("configured-test-source-commands");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::create_dir_all(repo.join("src/fixture/java/com/example/ops")).unwrap();
    std::fs::write(
        repo.join("pom.xml"),
        r#"<project>
  <build>
    <testSourceDirectory>src/fixture/java</testSourceDirectory>
  </build>
</project>
"#,
    )
    .unwrap();
    let main_file = "src/main/java/com/example/ops/StartupMaintenance.java";
    let fixture_file = "src/fixture/java/com/example/ops/FixtureMaintenance.java";
    std::fs::write(
        repo.join(main_file),
        r#"package com.example.ops;
import org.springframework.boot.ApplicationRunner;
class StartupMaintenance implements ApplicationRunner {
    public void run(org.springframework.boot.ApplicationArguments args) {}
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(fixture_file),
        r#"package com.example.ops;
import org.springframework.boot.CommandLineRunner;
class FixtureMaintenance implements CommandLineRunner {
    public void run(String... args) {}
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", main_file, fixture_file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "StartupMaintenance",
            "com.example.ops.StartupMaintenance",
            main_file,
        ),
        (3, 5),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Class",
            "FixtureMaintenance",
            "com.example.ops.FixtureMaintenance",
            fixture_file,
        ),
        (3, 5),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let handlers: BTreeSet<_> = synth
        .commands
        .iter()
        .map(|command| command.handler_name.as_str())
        .collect();
    assert!(handlers.contains("StartupMaintenance"));
    assert!(!handlers.contains("FixtureMaintenance"));
}

#[test]
fn synthesizes_python_command_entrypoints() {
    let repo = temp_repo("python-commands");
    std::fs::create_dir_all(repo.join("orders/management/commands")).unwrap();
    std::fs::write(
        repo.join("orders/management/commands/reindex_orders.py"),
        r#"from django.core.management.base import BaseCommand

class Command(BaseCommand):
    def handle(self, *args, **options):
        rebuild_orders()
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("cli.py"),
        r#"import argparse
import click
import typer

app = typer.Typer()

@click.command(name="sync-orders")
def sync_orders():
    pass

@app.command("ship-orders")
def ship_orders():
    pass

def main():
    parser = argparse.ArgumentParser(prog="orders-admin")
    sub = parser.add_subparsers()
    sub.add_parser("reindex")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "handle",
        "orders.management.commands.reindex_orders.Command.handle",
        "orders/management/commands/reindex_orders.py",
        (4, 5),
        json!({
            "language": "python",
            "parent_class": "Command",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "sync_orders",
        "cli.sync_orders",
        "cli.py",
        (8, 9),
        json!({
            "language": "python",
            "decorators": ["@click.command(name=\"sync-orders\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "ship_orders",
        "cli.ship_orders",
        "cli.py",
        (12, 13),
        json!({
            "language": "python",
            "decorators": ["@app.command(\"ship-orders\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "main",
        "cli.main",
        "cli.py",
        (15, 18),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let names: BTreeSet<_> = synth
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert!(names.contains("reindex_orders"));
    assert!(names.contains("sync-orders"));
    assert!(names.contains("ship-orders"));
    assert!(names.contains("orders-admin"));
    assert!(names.contains("reindex"));
    assert!(synth
        .commands
        .iter()
        .any(|command| command.command_type == "django-management-command"));
    assert!(synth
        .commands
        .iter()
        .any(|command| command.command_type == "click-command"));
    assert!(synth
        .commands
        .iter()
        .any(|command| command.command_type == "typer-command"));
    assert!(synth
        .commands
        .iter()
        .any(|command| command.command_type == "argparse-command"));
}

#[test]
fn command_synthesis_excludes_python_configured_test_roots() {
    let repo = temp_repo("python-configured-test-commands");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("ops")).unwrap();
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(
        repo.join("pyproject.toml"),
        r#"[tool.pytest.ini_options]
testpaths = ["tests"]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("ops/cli.py"),
        r#"import click

@click.command(name="sync-orders")
def sync_orders():
    pass
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("tests/test_cli.py"),
        r#"import click

@click.command(name="fixture-sync")
def fixture_sync():
    pass
"#,
    )
    .unwrap();
    run_git(
        &repo,
        &["add", "pyproject.toml", "ops/cli.py", "tests/test_cli.py"],
    );

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "sync_orders",
        "ops.cli.sync_orders",
        "ops/cli.py",
        (3, 4),
        json!({
            "language": "python",
            "decorators": ["@click.command(name=\"sync-orders\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "fixture_sync",
        "tests.test_cli.fixture_sync",
        "tests/test_cli.py",
        (3, 4),
        json!({
            "language": "python",
            "decorators": ["@click.command(name=\"fixture-sync\")"],
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let names: BTreeSet<_> = synth
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert!(names.contains("sync-orders"));
    assert!(!names.contains("fixture-sync"));
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
fn synthesizes_java_persistence_tables_and_repositories() {
    let repo = temp_repo("java-persistence");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/Order.java"),
        r#"package com.example.orders;

import jakarta.persistence.Column;
import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {
    @Column(name = "status")
    String status;
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderRepository.java"),
        r#"package com.example.orders;

import org.springframework.data.jpa.repository.JpaRepository;

interface OrderRepository extends JpaRepository<Order, Long> {
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "Order",
            "com.example.orders.Order",
            "src/main/java/com/example/orders/Order.java",
        ),
        (8, 12),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Interface",
            "OrderRepository",
            "com.example.orders.OrderRepository",
            "src/main/java/com/example/orders/OrderRepository.java",
        ),
        (5, 6),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let nodes = synth.persistence.node_recs();
    assert!(nodes.iter().any(|node| {
        node.label == "Table"
            && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
    }));
    assert!(nodes.iter().any(|node| {
        node.label == "Repository"
            && node.properties.get("entityName").and_then(Value::as_str) == Some("Order")
    }));
    let edge_types: Vec<_> = synth
        .persistence
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"MAPS_TO_TABLE".to_string()));
    assert!(edge_types.contains(&"MANAGES_ENTITY".to_string()));
    assert!(edge_types.contains(&"REPOSITORY_FOR".to_string()));
}

#[test]
fn synthesizes_java_table_access_edges() {
    let repo = temp_repo("java-table-access");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let entity_file = "src/main/java/com/example/orders/Order.java";
    let repo_file = "src/main/java/com/example/orders/OrderRepository.java";
    let service_file = "src/main/java/com/example/orders/OrderService.java";
    std::fs::write(
        repo.join(entity_file),
        r#"package com.example.orders;

import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(repo_file),
        r#"package com.example.orders;

import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;

interface OrderRepository extends JpaRepository<Order, Long> {
    @Query(value = "select * from orders where status = ?1", nativeQuery = true)
    List<Order> findNative(String status);
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(service_file),
        r#"package com.example.orders;

class OrderService {
    void cancelOrders(EntityManager em) {
        em.createNativeQuery("update orders set status = 'CANCELLED'");
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "com.example.orders.Order", entity_file),
        (6, 9),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "findNative",
        "com.example.orders.OrderRepository.findNative",
        repo_file,
        (7, 8),
        json!({
            "decorators": ["@Query(value = \"select * from orders where status = ?1\", nativeQuery = true)"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "cancelOrders",
        "com.example.orders.OrderService.cancelOrders",
        service_file,
        (4, 6),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_id = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id;
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:2:com.example.orders.OrderRepository.findNative"
            && edge.target_id == table_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:3:com.example.orders.OrderService.cancelOrders"
            && edge.target_id == table_id
    }));
}

#[test]
fn synthesizes_java_migration_tables() {
    let repo = temp_repo("java-migrations");
    std::fs::create_dir_all(repo.join("src/main/resources/db/migration")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/resources/db/changelog")).unwrap();
    std::fs::write(
        repo.join("src/main/resources/db/migration/V1__create_orders.sql"),
        r#"CREATE TABLE orders (
    id bigint primary key,
    status varchar(32)
);

ALTER TABLE order_items ADD COLUMN sku varchar(64);
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/resources/db/changelog/changelog.yaml"),
        r#"- changeSet:
    id: 2
    author: aka
    changes:
      - createTable:
          tableName: invoices
"#,
    )
    .unwrap();

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
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
    assert!(table_names.contains("order_items"));
    assert!(table_names.contains("invoices"));

    let migrations: Vec<_> = synth
        .persistence
        .node_recs()
        .into_iter()
        .filter(|node| node.label == "Migration")
        .collect();
    assert_eq!(migrations.len(), 2);
    assert!(migrations
        .iter()
        .any(|node| node.properties["version"] == "1"));
    let edge_types: Vec<_> = synth
        .persistence
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"MIGRATES_TABLE".to_string()));
}

#[test]
fn synthesizes_python_migration_tables() {
    let repo = temp_repo("python-migrations");
    std::fs::create_dir_all(repo.join("alembic/versions")).unwrap();
    std::fs::create_dir_all(repo.join("orders/migrations")).unwrap();
    std::fs::write(
        repo.join("alembic/versions/20260615_create_shipments.py"),
        r#"from alembic import op
import sqlalchemy as sa

def upgrade():
    op.create_table("shipments", sa.Column("id", sa.Integer()))
    op.add_column("orders", sa.Column("shipped_at", sa.DateTime()))
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/migrations/0002_invoice.py"),
        r#"from django.db import migrations

class Migration(migrations.Migration):
    operations = [
        migrations.CreateModel(name="Invoice", fields=[]),
        migrations.RunSQL("ALTER TABLE orders ADD COLUMN invoice_id integer"),
    ]
"#,
    )
    .unwrap();

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
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
    assert!(table_names.contains("shipments"));
    assert!(table_names.contains("orders"));
    assert!(table_names.contains("invoice"));
    let migration_hits = synth
        .persistence
        .node_recs()
        .into_iter()
        .filter(|node| node.label == "Migration")
        .count();
    assert_eq!(migration_hits, 2);
}

#[test]
fn synthesizes_python_persistence_tables_repositories_and_relationships() {
    let repo = temp_repo("python-persistence");
    std::fs::write(
        repo.join("models.py"),
        r#"from sqlalchemy import Column, ForeignKey, Integer, String
from sqlalchemy.orm import relationship

class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True)
    customer_id = Column(ForeignKey("customers.id"))
    customer = relationship("Customer")

class Customer(Base):
    __tablename__ = "customers"
    id = Column(Integer, primary_key=True)

class OrderRepository:
    model = Order
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "models.Order", "models.py"),
        (4, 8),
        json!({
            "language": "python",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Class", "Customer", "models.Customer", "models.py"),
        (10, 12),
        json!({
            "language": "python",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Class",
            "OrderRepository",
            "models.OrderRepository",
            "models.py",
        ),
        (14, 15),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let nodes = synth.persistence.node_recs();
    assert!(nodes.iter().any(|node| {
        node.label == "Table"
            && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
    }));
    assert!(nodes.iter().any(|node| {
        node.label == "Table"
            && node.properties.get("tableName").and_then(Value::as_str) == Some("customers")
    }));
    assert!(nodes.iter().any(|node| {
        node.label == "Repository"
            && node.properties.get("entityName").and_then(Value::as_str) == Some("Order")
    }));
    let edge_types: Vec<_> = synth
        .persistence
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"MAPS_TO_TABLE".to_string()));
    assert!(edge_types.contains(&"MANAGES_ENTITY".to_string()));
    assert!(edge_types.contains(&"HAS_RELATION".to_string()));
}

#[test]
fn synthesizes_python_table_access_edges() {
    let repo = temp_repo("python-table-access");
    std::fs::write(
        repo.join("models.py"),
        r#"from sqlalchemy import Column, Integer

class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True)

class Customer(Base):
    __tablename__ = "customers"
    id = Column(Integer, primary_key=True)
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("services.py"),
        r#"from sqlalchemy import select, text
from models import Customer, Order

def load_orders(session):
    return session.query(Order).all()

def load_customers(session):
    return session.execute(select(Customer)).all()

def archive_orders(session):
    session.execute(text("update orders set archived = true"))
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "models.Order", "models.py"),
        (3, 5),
        json!({
            "language": "python",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Class", "Customer", "models.Customer", "models.py"),
        (7, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "load_orders",
        "services.load_orders",
        "services.py",
        (4, 5),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "load_customers",
        "services.load_customers",
        "services.py",
        (7, 8),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "archive_orders",
        "services.archive_orders",
        "services.py",
        (10, 11),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let nodes = synth.persistence.node_recs();
    let orders_id = nodes
        .iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id
        .clone();
    let customers_id = nodes
        .iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("customers")
        })
        .expect("customers table")
        .id
        .clone();
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:3:services.load_orders"
            && edge.target_id == orders_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:4:services.load_customers"
            && edge.target_id == customers_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:5:services.archive_orders"
            && edge.target_id == orders_id
    }));
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
fn synthesizes_java_cache_nodes() {
    let repo = temp_repo("java-cache");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/cache")).unwrap();
    let file = "src/main/java/com/example/cache/OrderCache.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.cache;

import org.springframework.cache.annotation.CacheEvict;
import org.springframework.cache.annotation.Cacheable;

class OrderCache {
    @Cacheable(cacheNames = "orders")
    public OrderDto loadOrder(String id) {
        return redisTemplate.opsForValue().get("orders:" + id);
    }

    @CacheEvict(value = "orders")
    public void evictOrder(String id) {
        redisTemplate.delete("orders:" + id);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "loadOrder",
        "com.example.cache.OrderCache.loadOrder",
        file,
        (7, 9),
        json!({
            "decorators": ["@Cacheable(cacheNames = \"orders\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "evictOrder",
        "com.example.cache.OrderCache.evictOrder",
        file,
        (12, 14),
        json!({
            "decorators": ["@CacheEvict(value = \"orders\")"],
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let cache = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders" && cache.backend == "spring-cache")
        .expect("spring orders cache");
    assert_eq!(cache.readers.len(), 1);
    assert_eq!(cache.evictors.len(), 1);
    let redis = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders:" && cache.backend == "redis")
        .expect("redis key prefix");
    assert_eq!(redis.readers.len(), 1);
    assert_eq!(redis.evictors.len(), 1);
    let edge_types: Vec<_> = synth
        .caches
        .iter()
        .flat_map(SynthCache::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"READS_CACHE".to_string()));
    assert!(edge_types.contains(&"EVICTS_CACHE".to_string()));
}

#[test]
fn synthesizes_python_cache_nodes() {
    let repo = temp_repo("python-cache");
    std::fs::write(
        repo.join("cache_ops.py"),
        r#"from django.core.cache import cache

def load_order(order_id, redis):
    value = cache.get("orders:list")
    redis.set("orders:last", order_id)
    return redis.get("orders:last")

def evict_order():
    cache.delete("orders:list")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "load_order",
        "cache_ops.load_order",
        "cache_ops.py",
        (3, 6),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "evict_order",
        "cache_ops.evict_order",
        "cache_ops.py",
        (8, 9),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let django = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders:list" && cache.backend == "django-cache")
        .expect("django cache key");
    assert_eq!(django.readers.len(), 1);
    assert_eq!(django.evictors.len(), 1);
    let redis = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders:last" && cache.backend == "redis")
        .expect("redis cache key");
    assert_eq!(redis.readers.len(), 1);
    assert_eq!(redis.writers.len(), 1);
    let edge_types: Vec<_> = synth
        .caches
        .iter()
        .flat_map(SynthCache::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"READS_CACHE".to_string()));
    assert!(edge_types.contains(&"WRITES_CACHE".to_string()));
    assert!(edge_types.contains(&"EVICTS_CACHE".to_string()));
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
fn synthesizes_python_external_http_resources() {
    let repo = temp_repo("python-resources");
    std::fs::write(
        repo.join("payments.py"),
        r#"import aiohttp
import httpx
import requests

def charge(order_id):
    response = requests.post(f"https://payments.example.com/v1/orders/{order_id}/charge")
    return response.json()

async def reserve(sku):
    async with aiohttp.ClientSession() as session:
        response = await session.get(f"https://inventory.example.com/api/stock/{sku}")
        return await response.json()

async def notify(order_id):
    async with httpx.AsyncClient() as client:
        response = await client.post("https://events.example.com/orders", json={"id": order_id})
        return response.status_code
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "charge",
        "payments.charge",
        "payments.py",
        (5, 7),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "reserve",
        "payments.reserve",
        "payments.py",
        (9, 12),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "notify",
        "payments.notify",
        "payments.py",
        (14, 17),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == "https://payments.example.com/v1/orders/{param}/charge")
        .expect("external payment resource");
    assert_eq!(resource.resource_type, "http");
    assert_eq!(resource.callers.len(), 1);
    let edge_types: Vec<_> = resource
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"HTTP_CALLS".to_string()));
    assert!(synth.resources.iter().any(|resource| {
        resource.url == "https://inventory.example.com/api/stock/{param}"
            && resource.edge_recs().iter().any(|edge| {
                edge.edge_type == "HTTP_CALLS" && edge.source_id == "cbm:2:payments.reserve"
            })
    }));
    assert!(synth.resources.iter().any(|resource| {
        resource.url == "https://events.example.com/orders"
            && resource.edge_recs().iter().any(|edge| {
                edge.edge_type == "HTTP_CALLS" && edge.source_id == "cbm:3:payments.notify"
            })
    }));
}

#[test]
fn synthesizes_java_external_http_resources() {
    let repo = temp_repo("java-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/inventory")).unwrap();
    let file = "src/main/java/com/example/inventory/InventoryClient.java";
    std::fs::write(
            repo.join(file),
            r#"package com.example.inventory;

import org.springframework.web.client.RestTemplate;
import java.net.URI;

class InventoryClient {
    private final RestTemplate restTemplate = new RestTemplate();

    String reserve(String sku) {
        return restTemplate.getForObject("https://inventory.example.com/api/stock/" + sku, String.class);
    }

    java.net.http.HttpRequest reorder(String sku) {
        return java.net.http.HttpRequest.newBuilder()
            .uri(URI.create("https://supply.example.com/api/reorders/" + sku))
            .build();
    }

    okhttp3.Request availability(String sku) {
        return new okhttp3.Request.Builder()
            .url("https://catalog.example.com/api/availability/" + sku)
            .build();
    }
}"#,
        )
        .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "reserve",
        "com.example.inventory.InventoryClient.reserve",
        file,
        (8, 10),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "reorder",
        "com.example.inventory.InventoryClient.reorder",
        file,
        (12, 16),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "availability",
        "com.example.inventory.InventoryClient.availability",
        file,
        (18, 22),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == "https://inventory.example.com/api/stock/")
        .expect("external inventory resource");
    assert_eq!(resource.callers.len(), 1);
    let edge = resource
        .edge_recs()
        .into_iter()
        .next()
        .expect("http call edge");
    assert_eq!(
        edge.source_id,
        "cbm:1:com.example.inventory.InventoryClient.reserve"
    );
    assert!(synth.resources.iter().any(|resource| {
        resource.url == "https://supply.example.com/api/reorders/"
            && resource.edge_recs().iter().any(|edge| {
                edge.edge_type == "HTTP_CALLS"
                    && edge.source_id == "cbm:2:com.example.inventory.InventoryClient.reorder"
            })
    }));
    assert!(synth.resources.iter().any(|resource| {
        resource.url == "https://catalog.example.com/api/availability/"
            && resource.edge_recs().iter().any(|edge| {
                edge.edge_type == "HTTP_CALLS"
                    && edge.source_id == "cbm:3:com.example.inventory.InventoryClient.availability"
            })
    }));
}

#[test]
fn synthesizes_java_graphql_operations() {
    let repo = temp_repo("java-graphql");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/graphql")).unwrap();
    let file = "src/main/java/com/example/graphql/OrderGraphql.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.graphql;

import org.springframework.graphql.data.method.annotation.Argument;
import org.springframework.graphql.data.method.annotation.MutationMapping;
import org.springframework.graphql.data.method.annotation.QueryMapping;

class OrderGraphql {
    @QueryMapping(name = "order")
    Order order(@Argument String id) {
        return service.find(id);
    }

    @MutationMapping("createOrder")
    Order create(OrderInput input) {
        return service.create(input);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "order",
        "com.example.graphql.OrderGraphql.order",
        file,
        (8, 10),
        json!({
            "language": "java",
            "decorators": ["@QueryMapping(name = \"order\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "create",
        "com.example.graphql.OrderGraphql.create",
        file,
        (13, 15),
        json!({
            "language": "java",
            "decorators": ["@MutationMapping(\"createOrder\")"],
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let query = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "order")
        .expect("query operation");
    assert_eq!(query.operation_type, "query");
    assert_eq!(
        query.handler_id,
        "cbm:1:com.example.graphql.OrderGraphql.order"
    );
    let mutation = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "createOrder")
        .expect("mutation operation");
    assert_eq!(mutation.operation_type, "mutation");
    let edge_types: Vec<_> = synth
        .graphql
        .iter()
        .flat_map(SynthGraphqlOperation::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"HANDLES_GRAPHQL".to_string()));
}

#[test]
fn synthesizes_python_graphql_operations() {
    let repo = temp_repo("python-graphql");
    std::fs::write(
        repo.join("schema.py"),
        r#"import strawberry

@strawberry.type
class Query:
    @strawberry.field(name="order")
    def resolve_order(self, id: str):
        return get_order(id)

@strawberry.type
class Mutation:
    @strawberry.mutation
    def create_order(self, name: str):
        return create_order(name)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "resolve_order",
        "schema.Query.resolve_order",
        "schema.py",
        (5, 7),
        json!({
            "language": "python",
            "decorators": ["@strawberry.field(name=\"order\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "create_order",
        "schema.Mutation.create_order",
        "schema.py",
        (11, 13),
        json!({
            "language": "python",
            "decorators": ["@strawberry.mutation"],
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let query = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "order")
        .expect("strawberry query operation");
    assert_eq!(query.operation_type, "query");
    let mutation = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "create_order")
        .expect("strawberry mutation operation");
    assert_eq!(mutation.operation_type, "mutation");
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
