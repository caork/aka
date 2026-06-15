use super::super::*;
use serde_json::json;

#[test]
fn extracts_spring_constructed_dto_response_shape_keys() {
    let repo = temp_repo("java-spring-constructed-dto-response-shapes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders/dto")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders/web")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/dto/OrderDto.java"),
        r#"package com.example.orders.dto;

public record OrderDto(String id, String status, int total, String error) {}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/web/OrderController.java"),
        r#"package com.example.orders.web;

import com.example.orders.dto.OrderDto;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public ResponseEntity<?> getOrder(String id) {
        return ResponseEntity.ok(new OrderDto(id, "ok", 42, null));
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
        "com.example.orders.web.OrderController",
        "src/main/java/com/example/orders/web/OrderController.java",
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
        "com.example.orders.web.OrderController.getOrder",
        "src/main/java/com/example/orders/web/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.web.OrderController",
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
        .expect("spring route with constructed DTO response shape");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
}
