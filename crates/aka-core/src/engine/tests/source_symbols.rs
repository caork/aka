use super::*;

#[test]
fn synthesizes_java_source_symbols_when_cbm_misses_java_nodes() {
    let repo = temp_repo("java-source-symbol-fallback");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderController.java";
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
class OrderController {
    @GetMapping("/{id}")
    OrderDto getOrder(String id) {
        return loadOrder(id);
    }
}
"#,
    )
    .unwrap();
    std::fs::write(repo.join("application.yml"), "server: {}\n").unwrap();
    run_git(&repo, &["add", "pom.xml", file, "application.yml"]);

    let conn = test_conn();
    insert_file_hash(&conn, "application.yml");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.source_symbols.iter().any(|symbol| {
        symbol.node().label == "Class" && symbol.node().qn == "com.example.orders.OrderController"
    }));
    assert!(synth.source_symbols.iter().any(|symbol| {
        symbol.node().label == "Method"
            && symbol.node().name == "getOrder"
            && symbol.node().parent_class.as_deref() == Some("com.example.orders.OrderController")
    }));
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("Spring route should be synthesized from Java fallback symbols");
    assert_eq!(route.method.as_deref(), Some("GET"));
    assert_eq!(route.handler_name.as_deref(), Some("getOrder"));
}

#[test]
fn exports_java_source_symbol_nodes_for_search_indexing() {
    let repo = temp_repo("java-source-symbol-export");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderService.java";
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

class OrderService {
    void hydrateOrders() {}
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let out = repo.join("nodes.ndjson");
    let exported = export_nodes(&conn, "demo", &out, &synth, 0, &mut |_| {}).unwrap();
    let text = std::fs::read_to_string(out).unwrap();

    assert!(exported >= 2);
    assert!(text.contains("\"source\":\"aka-source-scan\""));
    assert!(text.contains("com.example.orders.OrderService"));
    assert!(text.contains("hydrateOrders"));
}

#[test]
fn synthesizes_java_direct_call_edges_for_fallback_source_symbols() {
    let repo = temp_repo("java-source-symbol-call-fallback");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderWorkflow.java";
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

class OrderWorkflow {
    void createOrder() {
        validateOrder();
    }

    void validateOrder() {
        persistOrder();
    }

    void persistOrder() {}
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();

    let create = synth
        .source_symbols
        .iter()
        .find(|symbol| symbol.node().qn == "com.example.orders.OrderWorkflow.createOrder")
        .expect("createOrder fallback symbol")
        .node()
        .aka_id
        .clone();
    let validate = synth
        .source_symbols
        .iter()
        .find(|symbol| symbol.node().qn == "com.example.orders.OrderWorkflow.validateOrder")
        .expect("validateOrder fallback symbol")
        .node()
        .aka_id
        .clone();
    let persist = synth
        .source_symbols
        .iter()
        .find(|symbol| symbol.node().qn == "com.example.orders.OrderWorkflow.persistOrder")
        .expect("persistOrder fallback symbol")
        .node()
        .aka_id
        .clone();

    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "CALLS" && edge.source_id == create && edge.target_id == validate
    }));
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "CALLS" && edge.source_id == validate && edge.target_id == persist
    }));
    assert!(
        synth
            .processes
            .iter()
            .any(|process| { process.name.as_str() == "createOrder → persistOrder" }),
        "processes: {:?}",
        synth
            .processes
            .iter()
            .map(|process| process.name.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn synthesizes_python_source_symbols_when_cbm_misses_python_nodes() {
    let repo = temp_repo("python-source-symbol-fallback");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("app/api")).unwrap();
    let file = "app/api/orders.py";
    std::fs::write(
        repo.join("pyproject.toml"),
        r#"[project]
name = "orders"
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(file),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/api")

class OrderService:
    def load_order(self, order_id: str):
        def normalize(raw):
            return raw
        return {"id": order_id}

@router.get("/orders/{order_id}")
async def get_order(order_id: str):
    def local_trace():
        return order_id
    return OrderService().load_order(order_id)
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pyproject.toml", file]);

    let conn = test_conn();
    insert_file_hash(&conn, "pyproject.toml");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.source_symbols.iter().any(|symbol| {
        symbol.node().label == "Class" && symbol.node().qn == "app.api.orders.OrderService"
    }));
    assert!(synth.source_symbols.iter().any(|symbol| {
        symbol.node().label == "Method"
            && symbol.node().name == "load_order"
            && symbol.node().parent_class.as_deref() == Some("app.api.orders.OrderService")
    }));
    assert!(synth.source_symbols.iter().any(|symbol| {
        symbol.node().label == "Function"
            && symbol.node().qn == "app.api.orders.get_order"
            && symbol
                .node()
                .decorators
                .iter()
                .any(|decorator| decorator == "@router.get(\"/orders/{order_id}\")")
    }));
    assert!(!synth
        .source_symbols
        .iter()
        .any(|symbol| matches!(symbol.node().name.as_str(), "normalize" | "local_trace")));
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{order_id}")
        .expect("FastAPI route should be synthesized from Python fallback symbols");
    assert_eq!(route.method.as_deref(), Some("GET"));
    assert_eq!(route.handler_name.as_deref(), Some("get_order"));
}

#[test]
fn synthesizes_python_direct_call_edges_for_fallback_source_symbols() {
    let repo = temp_repo("python-source-symbol-call-fallback");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    let file = "orders/workflow.py";
    std::fs::write(repo.join("pyproject.toml"), "[project]\nname='orders'\n").unwrap();
    std::fs::write(
        repo.join(file),
        r#"def create_order(payload):
    return validate_order(payload)

def validate_order(payload):
    return persist_order(payload)

def persist_order(payload):
    return payload["id"]
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pyproject.toml", file]);

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();

    let create = synth
        .source_symbols
        .iter()
        .find(|symbol| symbol.node().qn == "orders.workflow.create_order")
        .expect("create_order fallback symbol")
        .node()
        .aka_id
        .clone();
    let validate = synth
        .source_symbols
        .iter()
        .find(|symbol| symbol.node().qn == "orders.workflow.validate_order")
        .expect("validate_order fallback symbol")
        .node()
        .aka_id
        .clone();
    let persist = synth
        .source_symbols
        .iter()
        .find(|symbol| symbol.node().qn == "orders.workflow.persist_order")
        .expect("persist_order fallback symbol")
        .node()
        .aka_id
        .clone();

    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "CALLS" && edge.source_id == create && edge.target_id == validate
    }));
    assert!(synth.edges.iter().any(|edge| {
        edge.edge_type == "CALLS" && edge.source_id == validate && edge.target_id == persist
    }));
    assert!(
        synth
            .processes
            .iter()
            .any(|process| { process.name.as_str() == "create_order → persist_order" }),
        "processes: {:?}",
        synth
            .processes
            .iter()
            .map(|process| process.name.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn exports_python_source_symbol_nodes_for_search_indexing() {
    let repo = temp_repo("python-source-symbol-export");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    let file = "orders/service.py";
    std::fs::write(repo.join("pyproject.toml"), "[project]\nname='orders'\n").unwrap();
    std::fs::write(
        repo.join(file),
        r#"class OrderService:
    def hydrate_orders(self):
        return []
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pyproject.toml", file]);

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let out = repo.join("nodes.ndjson");
    let exported = export_nodes(&conn, "demo", &out, &synth, 0, &mut |_| {}).unwrap();
    let text = std::fs::read_to_string(out).unwrap();

    assert!(exported >= 2);
    assert!(text.contains("\"strategy\":\"python-source-symbol-fallback\""));
    assert!(text.contains("orders.service.OrderService"));
    assert!(text.contains("hydrate_orders"));
}
