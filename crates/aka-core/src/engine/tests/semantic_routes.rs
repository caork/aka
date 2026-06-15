use super::*;
use serde_json::json;

#[test]
fn synthesizes_spring_stomp_routes() {
    let repo = temp_repo("spring-stomp-routes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/realtime")).unwrap();
    let file = "src/main/java/com/example/realtime/OrderSocket.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.realtime;

import org.springframework.messaging.handler.annotation.MessageMapping;
import org.springframework.messaging.simp.annotation.SubscribeMapping;

@MessageMapping("/ws")
class OrderSocket {
    @MessageMapping("/orders")
    public OrderAck handleOrder(OrderMessage message) {
        return new OrderAck();
    }

    @SubscribeMapping("/orders/status")
    public OrderStatus subscribeStatus() {
        return new OrderStatus();
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "OrderSocket", "com.example.realtime.OrderSocket", file),
        (6, 16),
        json!({
            "decorators": ["@MessageMapping(\"/ws\")"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "handleOrder",
            "com.example.realtime.OrderSocket.handleOrder",
            file,
        ),
        (8, 10),
        json!({
            "decorators": ["@MessageMapping(\"/orders\")"],
            "language": "java",
            "parent_class": "com.example.realtime.OrderSocket",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "subscribeStatus",
            "com.example.realtime.OrderSocket.subscribeStatus",
            file,
        ),
        (13, 15),
        json!({
            "decorators": ["@SubscribeMapping(\"/orders/status\")"],
            "language": "java",
            "parent_class": "com.example.realtime.OrderSocket",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let message_route = synth
        .routes
        .iter()
        .find(|route| route.route == "/ws/orders")
        .expect("message mapping route");
    assert_eq!(message_route.method.as_deref(), Some("STOMP"));
    assert_eq!(
        message_route.handler_id.as_deref(),
        Some("cbm:2:com.example.realtime.OrderSocket.handleOrder")
    );

    let subscribe_route = synth
        .routes
        .iter()
        .find(|route| route.route == "/ws/orders/status")
        .expect("subscribe mapping route");
    assert_eq!(subscribe_route.method.as_deref(), Some("STOMP_SUBSCRIBE"));
    assert_eq!(
        subscribe_route.handler_id.as_deref(),
        Some("cbm:3:com.example.realtime.OrderSocket.subscribeStatus")
    );
}
