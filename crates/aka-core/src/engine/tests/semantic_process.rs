use super::*;
use serde_json::json;

#[test]
fn java_run_methods_need_source_facts_to_seed_process_entries() {
    let repo = temp_repo("java-run-source-fact-entry");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let file = "src/main/java/com/example/ops/Maintenance.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationRunner;
import org.springframework.context.annotation.Bean;

class MaintenanceConfig {
    @Bean
    ApplicationRunner ingestOrders(OrderService orders) {
        return args -> orders.loadOrders();
    }
}

class LocalWorker implements Runnable {
    public void run() {
        loadFixtures();
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
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Method",
            "ingestOrders",
            "com.example.ops.MaintenanceConfig.ingestOrders",
            file,
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
        ("Method", "run", "com.example.ops.LocalWorker.run", file),
        (14, 16),
        json!({
            "language": "java",
            "parent_class": "com.example.ops.LocalWorker",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "loadFixtures",
            "com.example.ops.LocalWorker.loadFixtures",
            file,
        ),
        (18, 18),
        json!({
            "language": "java",
            "parent_class": "com.example.ops.LocalWorker",
        }),
    );
    insert_node_props_at(
        &conn,
        4,
        (
            "Method",
            "loadOrders",
            "com.example.ops.OrderService.loadOrders",
            file,
        ),
        (22, 24),
        json!({
            "language": "java",
            "parent_class": "com.example.ops.OrderService",
        }),
    );
    insert_node_props_at(
        &conn,
        5,
        (
            "Method",
            "persistOrders",
            "com.example.ops.OrderService.persistOrders",
            file,
        ),
        (26, 26),
        json!({
            "language": "java",
            "parent_class": "com.example.ops.OrderService",
        }),
    );
    insert_edge(&conn, 1, 1, 4, "CALLS");
    insert_edge(&conn, 2, 4, 5, "CALLS");
    insert_edge(&conn, 3, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();

    let process = synth
        .processes
        .iter()
        .find(|process| process.name == "ingestOrders → persistOrders")
        .expect("Spring ApplicationRunner bean should seed process entry from source facts");
    assert_eq!(
        process.node_rec().properties["entryReason"],
        "java-spring-runner-bean-source-declaration"
    );
    assert!(!synth
        .processes
        .iter()
        .any(|process| process.name.starts_with("run ")));
}

#[test]
fn java_handler_names_do_not_override_source_fact_process_entries() {
    let repo = temp_repo("java-handler-name-not-entry");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let file = "src/main/java/com/example/ops/Maintenance.java";
    std::fs::write(
        repo.join(file),
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

class AuditHandler {
    void dispatchHandler() {
        enrichAudit();
    }

    void enrichAudit() {}
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Method",
            "ingestOrders",
            "com.example.ops.MaintenanceConfig.ingestOrders",
            file,
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
            file,
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
            file,
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
            "dispatchHandler",
            "com.example.ops.AuditHandler.dispatchHandler",
            file,
        ),
        (22, 24),
        json!({
            "language": "java",
            "parent_class": "com.example.ops.AuditHandler",
        }),
    );
    insert_node_props_at(
        &conn,
        5,
        (
            "Method",
            "enrichAudit",
            "com.example.ops.AuditHandler.enrichAudit",
            file,
        ),
        (26, 26),
        json!({
            "language": "java",
            "parent_class": "com.example.ops.AuditHandler",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");
    insert_edge(&conn, 3, 4, 5, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let process_names: Vec<_> = synth
        .processes
        .iter()
        .map(|process| process.name.as_str())
        .collect();
    assert!(process_names.contains(&"ingestOrders → persistOrders"));
    assert!(
        !process_names.contains(&"dispatchHandler → enrichAudit"),
        "Java Handler suffixes are name-only hints and must not outrank source-fact entries"
    );
}

#[test]
fn java_call_chains_without_source_entry_facts_do_not_fallback_to_name_only_processes() {
    let repo = temp_repo("java-no-name-only-process-fallback");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let file = "src/main/java/com/example/orders/OrderService.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

class OrderService {
    void processOrders() {
        validateOrders();
    }

    void validateOrders() {
        persistOrders();
    }

    void persistOrders() {}
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Method",
            "processOrders",
            "com.example.orders.OrderService.processOrders",
            file,
        ),
        (4, 6),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "validateOrders",
            "com.example.orders.OrderService.validateOrders",
            file,
        ),
        (8, 10),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "persistOrders",
            "com.example.orders.OrderService.persistOrders",
            file,
        ),
        (12, 12),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(
        synth.processes.is_empty(),
        "Git-tracked Java code without runner/job/source entry facts must not use process* names as entry points"
    );
}
