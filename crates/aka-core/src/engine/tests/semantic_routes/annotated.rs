use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_spring_routes_from_source_annotations_without_metadata() {
    let repo = temp_repo("spring-routes-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderController.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping(
    "/api/orders")
public class OrderController {
    @GetMapping(
        "/{id}")
    public String getOrder(String id) {
        return id;
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
            "OrderController",
            "com.example.orders.OrderController",
            file,
        ),
        (10, 16),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "getOrder",
            "com.example.orders.OrderController.getOrder",
            file,
        ),
        (13, 15),
        json!({
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring route from source annotations");
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:2:com.example.orders.OrderController.getOrder")
    );
    assert_eq!(route.method.as_deref(), Some("GET"));
    assert!(route
        .edge_recs()
        .into_iter()
        .any(|edge| edge.edge_type == "HANDLES_ROUTE"));
}

#[test]
fn synthesizes_fastapi_routes_from_source_decorators_without_metadata() {
    let repo = temp_repo("fastapi-routes-source-decorators");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    let file = "api/orders.py";
    std::fs::write(
        repo.join(file),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/api/orders")

@router.api_route("/{id}", methods=["GET", "HEAD"])
def get_order(id: str):
    return {"id": id}

@router.post("/{id}/reserve")
def reserve_order(id: str):
    return {"id": id}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "get_order",
        "api.orders.get_order",
        file,
        (6, 7),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "reserve_order",
        "api.orders.reserve_order",
        file,
        (10, 11),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let get = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}" && route.method.as_deref() == Some("GET"))
        .unwrap_or_else(|| {
            panic!(
                "FastAPI GET route from source decorator; got {:?}",
                synth
                    .routes
                    .iter()
                    .map(|route| (route.route.as_str(), route.method.as_deref()))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        get.handler_id.as_deref(),
        Some("cbm:1:api.orders.get_order")
    );
    let head = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}" && route.method.as_deref() == Some("HEAD"))
        .expect("FastAPI HEAD route from source decorator");
    assert_eq!(
        head.handler_id.as_deref(),
        Some("cbm:1:api.orders.get_order")
    );
    let post = synth
        .routes
        .iter()
        .find(|route| {
            route.route == "/api/orders/{id}/reserve" && route.method.as_deref() == Some("POST")
        })
        .expect("FastAPI POST route from source decorator");
    assert_eq!(
        post.handler_id.as_deref(),
        Some("cbm:2:api.orders.reserve_order")
    );
}
