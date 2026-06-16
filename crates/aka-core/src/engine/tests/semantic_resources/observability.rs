use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_observability_resources() {
    let repo = temp_repo("configured-observability-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"sentry:
  dsn: https://public@example.ingest.sentry.io/42
management:
  datadog:
    metrics:
      export:
        api-key: dd-redacted
newrelic:
  config:
    license-key: nr-redacted
otel:
  exporter:
    otlp:
      endpoint: http://collector:4317
prometheus:
  pushgateway:
    enabled: true
disabled:
  sentry:
    dsn: ${SENTRY_DSN}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"SENTRY_DSN = "https://public@example.ingest.sentry.io/43"
DD_SERVICE = "orders"
OTEL_SERVICE_NAME = "orders-api"
"#,
    )
    .unwrap();
    std::fs::write(repo.join(".env"), "NEW_RELIC_APP_NAME=orders\n").unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 20),
        json!({"language": "yaml"}),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Config", "settings.py", "settings.py", "settings.py"),
        (1, 3),
        json!({"language": "python"}),
    );
    insert_node_props_at(
        &conn,
        3,
        ("Config", ".env", ".env", ".env"),
        (1, 1),
        json!({"language": "dotenv"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_observability_config_edge(
        &synth,
        "observability:sentry",
        &config_id("sentry.dsn"),
        "sentry-config",
    );
    assert_observability_config_edge(
        &synth,
        "observability:datadog",
        &config_id("management.datadog.metrics.export.api.key"),
        "datadog-config",
    );
    assert_observability_config_edge(
        &synth,
        "observability:newrelic",
        &config_id("newrelic.config.license.key"),
        "newrelic-config",
    );
    assert_observability_config_edge(
        &synth,
        "observability:opentelemetry",
        &config_id("otel.exporter.otlp.endpoint"),
        "opentelemetry-config",
    );
    assert_observability_config_edge(
        &synth,
        "observability:prometheus",
        &config_id("prometheus.pushgateway.enabled"),
        "prometheus-config",
    );
    assert_observability_config_edge(
        &synth,
        "observability:datadog",
        &config_id("dd.service"),
        "datadog-config",
    );
    assert_observability_config_edge(
        &synth,
        "observability:opentelemetry",
        &config_id("otel.service.name"),
        "opentelemetry-config",
    );
    assert_observability_config_edge(
        &synth,
        "observability:newrelic",
        &config_id("new.relic.app.name"),
        "newrelic-config",
    );
    assert!(!synth.resources.iter().any(|resource| {
        resource.url == "observability:sentry"
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("disabled.sentry.dsn"))
    }));
}

fn assert_observability_config_edge(
    synth: &SynthGraph,
    url: &str,
    source_id: &str,
    strategy: &str,
) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected observability config resource {url}"));
    assert_eq!(resource.resource_type, "observability");
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == source_id
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == strategy
    }));
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
