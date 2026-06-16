use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_spring_http_config_resources() {
    let repo = temp_repo("spring-http-config-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"clients:
  payments:
    base-url: https://payments.example.com/api/
  inventory:
    endpoint: https://inventory.example.com/v1?token=${TOKEN}
feign:
  client:
    config:
      billing:
        url: https://billing.example.com
spring:
  security:
    oauth2:
      resourceserver:
        jwt:
          issuer-uri: https://login.example.com/oauth2/default
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 15),
        json!({
            "language": "yaml",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_http_config_edge(
        &synth,
        "https://payments.example.com/api",
        &config_id("clients.payments.base.url"),
    );
    assert_http_config_edge(
        &synth,
        "https://inventory.example.com/v1",
        &config_id("clients.inventory.endpoint"),
    );
    assert_http_config_edge(
        &synth,
        "https://billing.example.com",
        &config_id("feign.client.config.billing.url"),
    );
    assert!(synth.resources.iter().all(|resource| {
        resource.resource_type != "http"
            || resource.url != "https://login.example.com/oauth2/default"
    }));
}

#[test]
fn synthesizes_python_http_config_resources() {
    let repo = temp_repo("python-http-config-resources");
    std::fs::write(
        repo.join(".env"),
        r#"PAYMENTS_BASE_URL=https://payments.example.com
INVENTORY_API_URL=https://inventory.example.com/api
OIDC_ISSUER_URI=https://login.example.com/oauth2/default
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"BILLING_ENDPOINT = "https://billing.example.com/v2/"
STORAGE_ENDPOINT_URL = "https://storage.example.com/bucket"
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", ".env", ".env", ".env"),
        (1, 3),
        json!({
            "language": "dotenv",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Config", "settings.py", "settings.py", "settings.py"),
        (1, 2),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_http_config_edge(
        &synth,
        "https://payments.example.com",
        &config_id("payments.base.url"),
    );
    assert_http_config_edge(
        &synth,
        "https://inventory.example.com/api",
        &config_id("inventory.api.url"),
    );
    assert_http_config_edge(
        &synth,
        "https://billing.example.com/v2",
        &config_id("billing.endpoint"),
    );
    assert!(synth
        .resources
        .iter()
        .all(|resource| resource.url != "https://storage.example.com/bucket"));
    assert!(synth
        .resources
        .iter()
        .all(|resource| resource.url != "https://login.example.com/oauth2/default"));
}

fn assert_http_config_edge(synth: &SynthGraph, url: &str, source_id: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected http config resource {url}"));
    assert_eq!(resource.resource_type, "http");
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == source_id
            && edge.edge_type == "HTTP_CALLS"
            && edge.evidence.as_ref().unwrap()["strategy"] == "http-config-url"
    }));
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
