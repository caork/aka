use super::*;
use serde_json::json;

#[test]
fn synthesizes_spring_restclient_external_http_resources() {
    let repo = temp_repo("spring-restclient-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderGateway.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.web.client.RestClient;

class OrderGateway {
    private final RestClient restClient = RestClient.create("https://orders.example.com");

    OrderDto fetch(String id) {
        return restClient.get()
            .uri("/api/orders/{id}", id)
            .retrieve()
            .body(OrderDto.class);
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "fetch",
        "com.example.orders.OrderGateway.fetch",
        file,
        (8, 13),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == "https://orders.example.com/api/orders/{param}")
        .expect("Spring RestClient uri resource");
    let edge = resource
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:1:com.example.orders.OrderGateway.fetch")
        .expect("RestClient HTTP_CALLS edge");
    assert_eq!(edge.edge_type, "HTTP_CALLS");
    assert_eq!(
        edge.evidence.as_ref().unwrap()["strategy"],
        "java-spring-restclient"
    );
}
