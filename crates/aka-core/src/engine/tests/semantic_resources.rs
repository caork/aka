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
fn synthesizes_aiohttp_client_base_url_resources() {
    let repo = temp_repo("python-aiohttp-client-resources");
    std::fs::create_dir_all(repo.join("inventory")).unwrap();
    let file = "inventory/client.py";
    std::fs::write(
        repo.join(file),
        r#"import aiohttp

async def reserve_stock(sku):
    async with aiohttp.ClientSession(base_url="https://inventory.example.com") as session:
        async with session.get(f"/api/stock/{sku}/reserve") as response:
            return await response.json()
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "reserve_stock",
        "inventory.client.reserve_stock",
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
        .find(|resource| resource.url == "https://inventory.example.com/api/stock/{param}/reserve")
        .expect("aiohttp base_url resource");
    let edge = resource
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:1:inventory.client.reserve_stock")
        .expect("aiohttp HTTP_CALLS edge");
    assert_eq!(edge.edge_type, "HTTP_CALLS");
    assert_eq!(
        edge.evidence.as_ref().unwrap()["strategy"],
        "python-aiohttp"
    );
}

#[test]
fn synthesizes_python_external_http_resources() {
    let repo = temp_repo("python-resources");
    std::fs::write(
        repo.join("payments.py"),
        r#"import aiohttp
import httpx
import requests
import requests_toolbelt.sessions
import urllib.request

def charge(order_id):
    response = requests.post(f"https://payments.example.com/v1/orders/{order_id}/charge")
    return response.json()

def refund(order_id):
    session = requests_toolbelt.sessions.BaseUrlSession(base_url="https://payments.example.com")
    response = session.post(f"/v1/orders/{order_id}/refund")
    return response.json()

async def reserve(sku):
    async with aiohttp.ClientSession() as session:
        response = await session.get(f"https://inventory.example.com/api/stock/{sku}")
        return await response.json()

async def sync_stock(sku):
    async with aiohttp.ClientSession("https://inventory.example.com") as session:
        response = await session.get(f"/api/stock/{sku}/sync")
        return await response.json()

async def notify(order_id):
    async with httpx.AsyncClient() as client:
        response = await client.post("https://events.example.com/orders", json={"id": order_id})
        return response.status_code

def legacy_webhook(order_id):
    return urllib.request.urlopen(f"https://legacy.example.com/hooks/{order_id}").status
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
        (7, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "refund",
        "payments.refund",
        "payments.py",
        (11, 14),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "reserve",
        "payments.reserve",
        "payments.py",
        (16, 19),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "sync_stock",
        "payments.sync_stock",
        "payments.py",
        (21, 24),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "notify",
        "payments.notify",
        "payments.py",
        (26, 29),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        6,
        "legacy_webhook",
        "payments.legacy_webhook",
        "payments.py",
        (31, 32),
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
                edge.edge_type == "HTTP_CALLS" && edge.source_id == "cbm:3:payments.reserve"
            })
    }));
    assert!(synth.resources.iter().any(|resource| {
        resource.url == "https://payments.example.com/v1/orders/{param}/refund"
            && resource.edge_recs().iter().any(|edge| {
                edge.edge_type == "HTTP_CALLS" && edge.source_id == "cbm:2:payments.refund"
            })
    }));
    assert!(synth.resources.iter().any(|resource| {
        resource.url == "https://inventory.example.com/api/stock/{param}/sync"
            && resource.edge_recs().iter().any(|edge| {
                edge.edge_type == "HTTP_CALLS" && edge.source_id == "cbm:4:payments.sync_stock"
            })
    }));
    assert!(synth.resources.iter().any(|resource| {
        resource.url == "https://events.example.com/orders"
            && resource.edge_recs().iter().any(|edge| {
                edge.edge_type == "HTTP_CALLS" && edge.source_id == "cbm:5:payments.notify"
            })
    }));
    let legacy = synth
        .resources
        .iter()
        .find(|resource| resource.url == "https://legacy.example.com/hooks/{param}")
        .expect("urllib legacy webhook resource");
    let legacy_edge = legacy
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:6:payments.legacy_webhook")
        .expect("urllib HTTP_CALLS edge");
    assert_eq!(legacy_edge.edge_type, "HTTP_CALLS");
    assert_eq!(
        legacy_edge.evidence.as_ref().unwrap()["strategy"],
        "python-urllib"
    );
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

#[test]
fn synthesizes_spring_webclient_external_http_resources() {
    let repo = temp_repo("spring-webclient-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/inventory")).unwrap();
    let file = "src/main/java/com/example/inventory/InventoryGateway.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.inventory;

import org.springframework.web.reactive.function.client.WebClient;
import reactor.core.publisher.Mono;

class InventoryGateway {
    private final WebClient webClient = WebClient.builder()
        .baseUrl("https://inventory.example.com")
        .build();

    Mono<StockDto> fetch(String sku) {
        return webClient.get()
            .uri("/api/stock/{sku}", sku)
            .retrieve()
            .bodyToMono(StockDto.class);
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
        "com.example.inventory.InventoryGateway.fetch",
        file,
        (11, 16),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == "https://inventory.example.com/api/stock/{param}")
        .expect("Spring WebClient uri resource");
    let edge = resource
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:1:com.example.inventory.InventoryGateway.fetch")
        .expect("WebClient HTTP_CALLS edge");
    assert_eq!(edge.edge_type, "HTTP_CALLS");
    assert_eq!(
        edge.evidence.as_ref().unwrap()["strategy"],
        "java-spring-webclient"
    );
}

#[test]
fn synthesizes_spring_feign_external_http_resources() {
    let repo = temp_repo("spring-feign-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/catalog")).unwrap();
    let file = "src/main/java/com/example/catalog/CatalogClient.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.catalog;

import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.GetMapping;

@FeignClient(name = "catalog", url = "https://catalog.example.com", path = "/api/catalog")
public interface CatalogClient {
    @GetMapping("/{sku}")
    CatalogItem getItem(String sku);
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Interface",
        "CatalogClient",
        "com.example.catalog.CatalogClient",
        file,
        json!({
            "decorators": ["@FeignClient(name = \"catalog\", url = \"https://catalog.example.com\", path = \"/api/catalog\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "getItem",
        "com.example.catalog.CatalogClient.getItem",
        file,
        (8, 9),
        json!({
            "decorators": ["@GetMapping(\"/{sku}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.catalog.CatalogClient",
            "route_path": "/{sku}",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == "https://catalog.example.com/api/catalog/{param}")
        .expect("Spring Feign resource");
    let edge = resource
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:2:com.example.catalog.CatalogClient.getItem")
        .expect("Feign HTTP_CALLS edge");
    assert_eq!(edge.edge_type, "HTTP_CALLS");
    assert_eq!(
        edge.evidence.as_ref().unwrap()["strategy"],
        "java-spring-feign"
    );
}
