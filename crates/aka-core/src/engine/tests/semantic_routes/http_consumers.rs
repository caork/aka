use super::super::*;
use serde_json::json;
use std::collections::BTreeSet;

#[test]
fn synthesizes_route_nodes_consumers_and_entry_flows() {
    let repo = temp_repo("routes");
    std::fs::create_dir_all(repo.join("src/pages/api/config")).unwrap();
    std::fs::create_dir_all(repo.join("src/components")).unwrap();
    std::fs::write(
            repo.join("src/pages/api/config/route.ts"),
            "export async function GET() { return Response.json({ data: [], pagination: {}, error: null }); }",
        )
        .unwrap();
    std::fs::write(
            repo.join("src/components/config-panel.tsx"),
            "export async function ConfigPanel() { const res = await fetch('/api/config'); const data = await res.json(); return data.pagination.total + data.missing; }",
        )
        .unwrap();

    let conn = test_conn();
    insert_node(
        &conn,
        1,
        "Function",
        "GET",
        "src/pages/api/config/route.ts::GET",
        "src/pages/api/config/route.ts",
    );
    insert_node(
        &conn,
        2,
        "Function",
        "loadConfig",
        "src/pages/api/config/route.ts::loadConfig",
        "src/pages/api/config/route.ts",
    );
    insert_node(
        &conn,
        3,
        "Function",
        "ConfigPanel",
        "src/components/config-panel.tsx::ConfigPanel",
        "src/components/config-panel.tsx",
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_eq!(synth.routes.len(), 1);
    let route = &synth.routes[0];
    assert_eq!(route.route, "/api/config");
    assert!(route.response_keys.contains(&"data".to_string()));
    assert!(route.response_keys.contains(&"pagination".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
    assert_eq!(route.consumers.len(), 1);
    assert_eq!(route.consumers[0].fetch_count, 1);
    assert!(route.consumers[0].keys.contains(&"pagination".to_string()));

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"HANDLES_ROUTE".to_string()));
    assert!(edge_types.contains(&"FETCHES".to_string()));
}

#[test]
fn links_python_requests_consumers_to_routes() {
    let repo = temp_repo("python-route-consumers");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("workers")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/api/orders")

@router.get("/{id}")
def get_order(id: str):
    return {"id": id, "status": "ok"}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("workers/sync.py"),
        r#"import requests

def sync_order(order_id: str):
    response = requests.get(f"http://orders.internal/api/orders/{order_id}")
    data = response.json()
    return data["status"]
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "api.orders.get_order",
        "api/orders.py",
        json!({
            "decorators": ["@router.get(\"/{id}\")"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Function",
        "sync_order",
        "workers.sync.sync_order",
        "workers/sync.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("parameterized FastAPI route");
    assert_eq!(route.consumers.len(), 1);
    assert_eq!(route.consumers[0].node_id, "cbm:2:workers.sync.sync_order");
    assert!(route.consumers[0].keys.contains(&"status".to_string()));

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"FETCHES".to_string()));
}

#[test]
fn links_python_httpx_client_consumers_to_routes() {
    let repo = temp_repo("python-httpx-route-consumers");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("workers")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/api/orders")

@router.get("/{id}")
def get_order(id: str):
    return {"id": id, "status": "ok"}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("workers/sync.py"),
        r#"import httpx

async def sync_order(order_id: str):
    async with httpx.AsyncClient() as client:
        response = await client.get(f"http://orders.internal/api/orders/{order_id}")
        data = response.json()
        return data["status"]
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "api.orders.get_order",
        "api/orders.py",
        json!({
            "decorators": ["@router.get(\"/{id}\")"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Function",
        "sync_order",
        "workers.sync.sync_order",
        "workers/sync.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("parameterized FastAPI route");
    assert_eq!(route.consumers.len(), 1);
    assert_eq!(route.consumers[0].node_id, "cbm:2:workers.sync.sync_order");
    assert!(route.consumers[0].keys.contains(&"status".to_string()));
}

#[test]
fn extracts_fastapi_response_model_shape_keys() {
    let repo = temp_repo("python-fastapi-response-model-shapes");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("workers")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter
from pydantic import BaseModel

router = APIRouter(prefix="/api/orders")

class OrderResponse(BaseModel):
    id: str
    status: str
    total: int
    error: str | None = None

@router.get("/{id}", response_model=OrderResponse)
def get_order(id: str):
    return load_order(id)
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("workers/sync.py"),
        r#"import httpx

async def sync_order(order_id: str):
    async with httpx.AsyncClient() as client:
        response = await client.get(f"http://orders.internal/api/orders/{order_id}")
        data = response.json()
        return data["status"] + data["missing"]
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "api.orders.get_order",
        "api/orders.py",
        json!({
            "decorators": ["@router.get(\"/{id}\", response_model=OrderResponse)"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Function",
        "sync_order",
        "workers.sync.sync_order",
        "workers/sync.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("FastAPI route with Pydantic response model");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
    assert_eq!(route.consumers.len(), 1);
    assert!(route.consumers[0].keys.contains(&"status".to_string()));
    assert!(route.consumers[0].keys.contains(&"missing".to_string()));
}

#[test]
fn extracts_fastapi_imported_response_model_shape_keys() {
    let repo = temp_repo("python-fastapi-imported-response-model-shapes");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("workers")).unwrap();
    std::fs::write(
        repo.join("api/schemas.py"),
        r#"from pydantic import BaseModel

class OrderResponse(BaseModel):
    id: str
    status: str
    total: int
    error: str | None = None
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter
from api.schemas import OrderResponse

router = APIRouter(prefix="/api/orders")

@router.get("/{id}", response_model=OrderResponse)
def get_order(id: str):
    return load_order(id)
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("workers/sync.py"),
        r#"import requests

def sync_order(order_id: str):
    response = requests.get(f"http://orders.internal/api/orders/{order_id}")
    data = response.json()
    return data["status"] + data["missing"]
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "api.orders.get_order",
        "api/orders.py",
        json!({
            "decorators": ["@router.get(\"/{id}\", response_model=OrderResponse)"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Function",
        "sync_order",
        "workers.sync.sync_order",
        "workers/sync.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("FastAPI route with imported Pydantic response model");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
    assert_eq!(route.consumers.len(), 1);
    assert!(route.consumers[0].keys.contains(&"status".to_string()));
    assert!(route.consumers[0].keys.contains(&"missing".to_string()));
}

#[test]
fn links_python_aiohttp_client_consumers_to_routes() {
    let repo = temp_repo("python-aiohttp-route-consumers");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::create_dir_all(repo.join("workers")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from fastapi import APIRouter

router = APIRouter(prefix="/api/orders")

@router.post("/{id}/reserve")
def reserve_order(id: str):
    return {"id": id, "status": "reserved"}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("workers/reserve.py"),
        r#"import aiohttp

async def reserve_order(order_id: str):
    async with aiohttp.ClientSession() as session:
        async with session.request(
            "POST",
            f"http://orders.internal/api/orders/{order_id}/reserve",
        ) as response:
            data = await response.json()
            return data["status"]
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "reserve_order",
        "api.orders.reserve_order",
        "api/orders.py",
        json!({
            "decorators": ["@router.post(\"/{id}/reserve\")"],
            "language": "python",
            "route_method": "POST",
            "route_path": "/{id}/reserve",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Function",
        "reserve_order",
        "workers.reserve.reserve_order",
        "workers/reserve.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}/reserve")
        .expect("parameterized FastAPI route");
    assert_eq!(route.consumers.len(), 1);
    assert_eq!(
        route.consumers[0].node_id,
        "cbm:2:workers.reserve.reserve_order"
    );
    assert!(route.consumers[0].keys.contains(&"status".to_string()));
}

#[test]
fn synthesizes_spring_routes_with_class_prefix() {
    let repo = temp_repo("spring-routes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public String getOrder(String id) {
        return id;
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
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
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
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
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
        .expect("spring route with class prefix");
    assert_eq!(
        route.handler_id.as_deref(),
        Some("cbm:2:com.example.orders.OrderController.getOrder")
    );

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"HANDLES_ROUTE".to_string()));
}

#[test]
fn synthesizes_spring_functional_router_routes() {
    let repo = temp_repo("spring-functional-routes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderRoutes.java"),
        r#"package com.example.orders;

import static org.springframework.web.reactive.function.server.RequestPredicates.GET;
import static org.springframework.web.reactive.function.server.RequestPredicates.path;
import static org.springframework.web.reactive.function.server.RequestPredicates.POST;
import static org.springframework.web.reactive.function.server.RouterFunctions.nest;
import static org.springframework.web.reactive.function.server.RouterFunctions.route;

import org.springframework.context.annotation.Bean;
import org.springframework.web.reactive.function.server.RouterFunction;
import org.springframework.web.reactive.function.server.ServerResponse;

class OrderRoutes {
    @Bean
    RouterFunction<ServerResponse> routes(OrderHandler handler) {
        return nest(path("/api"),
            route(GET("/orders/{id}"), handler::getOrder)
                .andRoute(POST("/orders"), handler::createOrder));
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderHandler.java"),
        r#"package com.example.orders;

class OrderHandler {
    ServerResponse getOrder(ServerRequest request) {
        return ServerResponse.ok().build();
    }

    ServerResponse createOrder(ServerRequest request) {
        return ServerResponse.ok().build();
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "getOrder",
        "com.example.orders.OrderHandler.getOrder",
        "src/main/java/com/example/orders/OrderHandler.java",
        json!({"language": "java"}),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "createOrder",
        "com.example.orders.OrderHandler.createOrder",
        "src/main/java/com/example/orders/OrderHandler.java",
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let get = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring functional GET route");
    assert_eq!(get.method.as_deref(), Some("GET"));
    assert_eq!(
        get.handler_id.as_deref(),
        Some("cbm:1:com.example.orders.OrderHandler.getOrder")
    );
    let post = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders")
        .expect("spring functional POST route");
    assert_eq!(post.method.as_deref(), Some("POST"));
    assert_eq!(
        post.handler_id.as_deref(),
        Some("cbm:2:com.example.orders.OrderHandler.createOrder")
    );
}

#[test]
fn synthesizes_spring_router_function_builder_routes() {
    let repo = temp_repo("spring-router-builder-routes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderRoutes.java"),
        r#"package com.example.orders;

import org.springframework.context.annotation.Bean;
import org.springframework.web.reactive.function.server.RouterFunction;
import org.springframework.web.reactive.function.server.RouterFunctions;
import org.springframework.web.reactive.function.server.ServerResponse;

class OrderRoutes {
    @Bean
    RouterFunction<ServerResponse> routes(OrderHandler handler) {
        return RouterFunctions.route()
            .path("/api", builder -> builder
                .GET("/orders/{id}", handler::getOrder)
                .POST("/orders", handler::createOrder))
            .build();
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderHandler.java"),
        r#"package com.example.orders;

class OrderHandler {
    ServerResponse getOrder(ServerRequest request) {
        return ServerResponse.ok().build();
    }

    ServerResponse createOrder(ServerRequest request) {
        return ServerResponse.ok().build();
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "getOrder",
        "com.example.orders.OrderHandler.getOrder",
        "src/main/java/com/example/orders/OrderHandler.java",
        json!({"language": "java"}),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "createOrder",
        "com.example.orders.OrderHandler.createOrder",
        "src/main/java/com/example/orders/OrderHandler.java",
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let get = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring router builder GET route");
    assert_eq!(get.method.as_deref(), Some("GET"));
    assert_eq!(
        get.handler_id.as_deref(),
        Some("cbm:1:com.example.orders.OrderHandler.getOrder")
    );
    let post = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders")
        .expect("spring router builder POST route");
    assert_eq!(post.method.as_deref(), Some("POST"));
    assert_eq!(
        post.handler_id.as_deref(),
        Some("cbm:2:com.example.orders.OrderHandler.createOrder")
    );
}

#[test]
fn links_java_http_consumers_to_spring_routes() {
    let repo = temp_repo("java-route-consumers");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/java/com/example/workers")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public OrderDto getOrder(String id) {
        return new OrderDto(id, "ok");
    }
}"#,
    )
    .unwrap();
    std::fs::write(
            repo.join("src/main/java/com/example/workers/OrderWorker.java"),
            r#"package com.example.workers;

import org.springframework.web.client.RestTemplate;

public class OrderWorker {
    private final RestTemplate restTemplate = new RestTemplate();

    public String syncOrder(String id) {
        OrderDto order = restTemplate.getForObject("http://orders/api/orders/" + id, OrderDto.class);
        return order.status();
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
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
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
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props(
        &conn,
        3,
        "Method",
        "syncOrder",
        "com.example.workers.OrderWorker.syncOrder",
        "src/main/java/com/example/workers/OrderWorker.java",
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring route with class prefix");
    let parent_route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders")
        .expect("spring class prefix route");
    assert!(
        parent_route.consumers.is_empty(),
        "parameterized detail calls should not also attach to parent collection routes"
    );
    assert_eq!(route.consumers.len(), 1);
    assert_eq!(
        route.consumers[0].node_id,
        "cbm:3:com.example.workers.OrderWorker.syncOrder"
    );
    assert!(route.consumers[0].keys.contains(&"status".to_string()));

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"FETCHES".to_string()));
}

#[test]
fn extracts_spring_response_entity_map_shape_keys() {
    let repo = temp_repo("java-spring-map-response-shapes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/java/com/example/workers")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import java.util.Map;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public ResponseEntity<Map<String, Object>> getOrder(String id) {
        return ResponseEntity.ok(Map.of(
            "id", id,
            "status", "ok",
            "total", 42,
            "error", null
        ));
    }
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/workers/OrderWorker.java"),
        r#"package com.example.workers;

import org.springframework.web.client.RestTemplate;

public class OrderWorker {
    private final RestTemplate restTemplate = new RestTemplate();

    public String syncOrder(String id) {
        OrderDto order = restTemplate.getForObject("http://orders/api/orders/" + id, OrderDto.class);
        return order.status() + order.missing();
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
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
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
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props(
        &conn,
        3,
        "Method",
        "syncOrder",
        "com.example.workers.OrderWorker.syncOrder",
        "src/main/java/com/example/workers/OrderWorker.java",
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring route with Java Map response shape");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
    assert_eq!(route.consumers.len(), 1);
    assert!(route.consumers[0].keys.contains(&"status".to_string()));
    assert!(route.consumers[0].keys.contains(&"missing".to_string()));
}

#[test]
fn extracts_spring_imported_dto_response_shape_keys() {
    let repo = temp_repo("java-spring-imported-dto-response-shapes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/java/com/example/workers")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderDto.java"),
        r#"package com.example.orders;

public record OrderDto(String id, String status, int total, String error) {}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;
import com.example.orders.OrderDto;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public ResponseEntity<OrderDto> getOrder(String id) {
        return ResponseEntity.ok(new OrderDto(id, "ok", 42, null));
    }
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/workers/OrderWorker.java"),
        r#"package com.example.workers;

import org.springframework.web.client.RestTemplate;

public class OrderWorker {
    private final RestTemplate restTemplate = new RestTemplate();

    public String syncOrder(String id) {
        OrderDto order = restTemplate.getForObject("http://orders/api/orders/" + id, OrderDto.class);
        return order.status() + order.missing();
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
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
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
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props(
        &conn,
        3,
        "Method",
        "syncOrder",
        "com.example.workers.OrderWorker.syncOrder",
        "src/main/java/com/example/workers/OrderWorker.java",
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring route with imported DTO response shape");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
    assert_eq!(route.consumers.len(), 1);
    assert!(route.consumers[0].keys.contains(&"status".to_string()));
    assert!(route.consumers[0].keys.contains(&"missing".to_string()));
}

#[test]
fn links_java_builder_http_consumers_to_spring_routes() {
    let repo = temp_repo("java-builder-route-consumers");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/java/com/example/workers")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public OrderDto getOrder(String id) {
        return new OrderDto(id, "ok");
    }
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/workers/OrderWorker.java"),
        r#"package com.example.workers;

import java.net.URI;

public class OrderWorker {
    public String syncOrder(String id) {
        var request = java.net.http.HttpRequest.newBuilder()
            .uri(URI.create("http://orders/api/orders/" + id))
            .build();
        OrderDto order = send(request);
        return order.getStatus();
    }

    public String syncOrderWithOkHttp(String id) {
        var request = new okhttp3.Request.Builder()
            .url("http://orders/api/orders/" + id)
            .build();
        OrderDto order = execute(request);
        return order.status();
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
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
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
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "syncOrder",
            "com.example.workers.OrderWorker.syncOrder",
            "src/main/java/com/example/workers/OrderWorker.java",
        ),
        (6, 12),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        4,
        (
            "Method",
            "syncOrderWithOkHttp",
            "com.example.workers.OrderWorker.syncOrderWithOkHttp",
            "src/main/java/com/example/workers/OrderWorker.java",
        ),
        (14, 20),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}")
        .expect("spring route with class prefix");
    let consumers: BTreeSet<_> = route
        .consumers
        .iter()
        .map(|consumer| consumer.node_id.as_str())
        .collect();
    assert!(consumers.contains("cbm:3:com.example.workers.OrderWorker.syncOrder"));
    assert!(consumers.contains("cbm:4:com.example.workers.OrderWorker.syncOrderWithOkHttp"));
    assert!(route.consumers.iter().any(|consumer| {
        consumer
            .keys
            .iter()
            .any(|key| key == "getStatus" || key == "status")
    }));
}

#[test]
fn links_java_feign_consumers_to_spring_routes() {
    let repo = temp_repo("java-feign-consumers");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/api/orders")
public class OrderController {
    @GetMapping("/{id}")
    public OrderDto getOrder(String id) {
        return new OrderDto(id, "ok");
    }
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderClient.java"),
        r#"package com.example.orders;

import org.springframework.cloud.openfeign.FeignClient;
import org.springframework.web.bind.annotation.GetMapping;

@FeignClient(name = "orders", path = "/api/orders")
public interface OrderClient {
    @GetMapping("/{id}")
    OrderDto getOrder(String id);
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Class",
        "OrderController",
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
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
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderController",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props(
        &conn,
        3,
        "Interface",
        "OrderClient",
        "com.example.orders.OrderClient",
        "src/main/java/com/example/orders/OrderClient.java",
        json!({
            "decorators": ["@FeignClient(name = \"orders\", path = \"/api/orders\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        4,
        "Method",
        "getOrder",
        "com.example.orders.OrderClient.getOrder",
        "src/main/java/com/example/orders/OrderClient.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:3:com.example.orders.OrderClient",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 2, 3, 4, "DEFINES_METHOD");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| {
            route.route == "/api/orders/{id}"
                && route.handler_id.as_deref()
                    == Some("cbm:2:com.example.orders.OrderController.getOrder")
        })
        .expect("spring provider route");
    assert!(route
        .consumers
        .iter()
        .any(|consumer| consumer.node_id == "cbm:4:com.example.orders.OrderClient.getOrder"));
}

#[test]
fn inherits_spring_routes_from_controller_interfaces() {
    let repo = temp_repo("java-interface-routes");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderApi.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;

@RequestMapping("/api/orders")
public interface OrderApi {
    @GetMapping("/{id}")
    OrderDto getOrder(String id);
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderController.java"),
        r#"package com.example.orders;

import org.springframework.web.bind.annotation.RestController;

@RestController
public class OrderController implements OrderApi {
    @Override
    public OrderDto getOrder(String id) {
        return new OrderDto(id, "ok");
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Interface",
        "OrderApi",
        "com.example.orders.OrderApi",
        "src/main/java/com/example/orders/OrderApi.java",
        json!({
            "decorators": ["@RequestMapping(\"/api/orders\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "getOrder",
        "com.example.orders.OrderApi.getOrder",
        "src/main/java/com/example/orders/OrderApi.java",
        json!({
            "decorators": ["@GetMapping(\"/{id}\")"],
            "language": "java",
            "parent_class": "cbm:1:com.example.orders.OrderApi",
            "route_method": "GET",
            "route_path": "/{id}",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "DEFINES_METHOD");
    insert_node_props(
        &conn,
        3,
        "Class",
        "OrderController",
        "com.example.orders.OrderController",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@RestController"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        4,
        "Method",
        "getOrder",
        "com.example.orders.OrderController.getOrder",
        "src/main/java/com/example/orders/OrderController.java",
        json!({
            "decorators": ["@Override"],
            "language": "java",
            "parent_class": "cbm:3:com.example.orders.OrderController",
        }),
    );
    insert_edge(&conn, 2, 3, 4, "DEFINES_METHOD");
    insert_edge(&conn, 3, 3, 1, "IMPLEMENTS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| {
            route.route == "/api/orders/{id}"
                && route.handler_id.as_deref()
                    == Some("cbm:4:com.example.orders.OrderController.getOrder")
        })
        .expect("controller implementation should inherit interface route");
    assert_eq!(route.method.as_deref(), Some("GET"));

    let edge_types: Vec<_> = route.edge_recs().into_iter().map(|e| e.edge_type).collect();
    assert!(edge_types.contains(&"HANDLES_ROUTE".to_string()));
}
