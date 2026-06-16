use super::*;
use serde_json::json;

#[test]
fn synthesizes_spring_bean_method_dependencies_from_source_annotations_without_metadata() {
    let repo = temp_repo("spring-bean-dependency-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderConfig.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.context.annotation.Bean;

class OrderConfig {
    @Bean(
        name = "orderHandler")
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
            "OrderService",
            "com.example.orders.OrderService",
            file,
        ),
        (13, 13),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Interface",
            "OrderRepository",
            "com.example.orders.OrderRepository",
            file,
        ),
        (14, 14),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "orderHandler",
            "com.example.orders.OrderConfig.orderHandler",
            file,
        ),
        (8, 10),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderConfig",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for target in [
        "cbm:1:com.example.orders.OrderService",
        "cbm:2:com.example.orders.OrderRepository",
    ] {
        assert!(synth.edges.iter().any(|edge| {
            edge.edge_type == "DEPENDS_ON"
                && edge.source_id == "cbm:3:com.example.orders.OrderConfig.orderHandler"
                && edge.target_id == target
                && edge.evidence.as_ref().is_some_and(|evidence| {
                    evidence["kind"] == json!("java-spring-bean-dependency")
                        && evidence["strategy"] == json!("java-bean-method-parameter")
                })
        }));
    }
}
