use super::*;
use serde_json::json;

#[test]
fn synthesizes_httpx_client_base_url_resources() {
    let repo = temp_repo("python-httpx-client-resources");
    std::fs::create_dir_all(repo.join("payments")).unwrap();
    let file = "payments/client.py";
    std::fs::write(
        repo.join(file),
        r#"import httpx

async def charge_order(order_id):
    async with httpx.AsyncClient(base_url="https://payments.example.com") as client:
        response = await client.post(f"/v1/orders/{order_id}/charge")
        return response.json()
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "charge_order",
        "payments.client.charge_order",
        file,
        (3, 6),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == "https://payments.example.com/v1/orders/{param}/charge")
        .expect("httpx base_url resource");
    let edge = resource
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:1:payments.client.charge_order")
        .expect("httpx HTTP_CALLS edge");
    assert_eq!(edge.edge_type, "HTTP_CALLS");
    assert_eq!(
        edge.evidence.as_ref().unwrap()["strategy"],
        "python-httpx-client"
    );
}

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
