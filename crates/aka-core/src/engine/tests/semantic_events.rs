use super::*;
use serde_json::json;

#[test]
fn records_spring_event_listener_metadata() {
    let repo = temp_repo("spring-event-metadata");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/events")).unwrap();
    let file = "src/main/java/com/example/events/OrderEvents.java";
    std::fs::write(
        repo.join(file),
        r##"package com.example.events;

import org.springframework.context.event.EventListener;
import org.springframework.transaction.event.TransactionPhase;
import org.springframework.transaction.event.TransactionalEventListener;

class OrderPaidEvent {}

class OrderEvents {
    @EventListener(classes = OrderPaidEvent.class, condition = "#event.total > 0")
    public void onPaid(OrderPaidEvent event) {}

    @TransactionalEventListener(classes = OrderPaidEvent.class, phase = TransactionPhase.AFTER_COMMIT)
    public void afterCommit(OrderPaidEvent event) {}
}"##,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "onPaid",
        "com.example.events.OrderEvents.onPaid",
        file,
        (10, 11),
        json!({
            "decorators": ["@EventListener(classes = OrderPaidEvent.class, condition = \"#event.total > 0\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "afterCommit",
        "com.example.events.OrderEvents.afterCommit",
        file,
        (13, 14),
        json!({
            "decorators": ["@TransactionalEventListener(classes = OrderPaidEvent.class, phase = TransactionPhase.AFTER_COMMIT)"],
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let event = synth
        .events
        .iter()
        .find(|event| event.name == "OrderPaidEvent")
        .expect("spring event");
    let edges = event.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.source_id == "cbm:1:com.example.events.OrderEvents.onPaid"
            && edge.evidence.as_ref().is_some_and(|evidence| {
                evidence["metadata"]["condition"] == json!("#event.total > 0")
            })
    }));
    assert!(edges.iter().any(|edge| {
        edge.source_id == "cbm:2:com.example.events.OrderEvents.afterCommit"
            && edge.evidence.as_ref().is_some_and(|evidence| {
                evidence["metadata"]["phase"] == json!("TransactionPhase.AFTER_COMMIT")
            })
    }));
}
