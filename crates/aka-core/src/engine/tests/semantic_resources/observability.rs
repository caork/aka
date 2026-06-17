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

#[test]
fn synthesizes_python_observability_resources() {
    let repo = temp_repo("python-observability-resources");
    std::fs::write(
        repo.join("observability.py"),
        r#"import sentry_sdk
from opentelemetry import trace

tracer = trace.get_tracer(__name__)

def report_failure(exc):
    sentry_sdk.capture_exception(exc)
    sentry_sdk.capture_message("order failed")

def trace_order(order):
    with tracer.start_as_current_span("order.process"):
        return order.id

def ordinary_capture(exc):
    return capture_exception(exc)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "report_failure",
        "observability.report_failure",
        "observability.py",
        (6, 8),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "trace_order",
        "observability.trace_order",
        "observability.py",
        (10, 12),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "ordinary_capture",
        "observability.ordinary_capture",
        "observability.py",
        (14, 15),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_observability_config_edge(
        &synth,
        "observability:sentry",
        "cbm:1:observability.report_failure",
        "python-sentry-capture-exception",
    );
    assert_observability_config_edge(
        &synth,
        "observability:sentry",
        "cbm:1:observability.report_failure",
        "python-sentry-capture-message",
    );
    assert_observability_config_edge(
        &synth,
        "observability:opentelemetry",
        "cbm:2:observability.trace_order",
        "python-opentelemetry-current-span",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:3:observability.ordinary_capture"));
}

#[test]
fn synthesizes_java_observability_resources() {
    let repo = temp_repo("java-observability-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/observability")).unwrap();
    let file = "src/main/java/com/example/observability/OrderObserver.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.observability;

import io.opentelemetry.api.GlobalOpenTelemetry;
import io.opentelemetry.api.trace.Span;
import io.sentry.Sentry;

class OrderObserver {
    void report(Throwable error) {
        Sentry.captureException(error);
        Sentry.captureMessage("order failed");
    }

    void traceOrder() {
        Span span = GlobalOpenTelemetry.getTracer("orders")
            .spanBuilder("order.process")
            .startSpan();
        span.end();
    }

    void ordinaryCapture(Widget widget, Throwable error) {
        widget.captureException(error);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "report",
        "com.example.observability.OrderObserver.report",
        file,
        (8, 11),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "traceOrder",
        "com.example.observability.OrderObserver.traceOrder",
        file,
        (13, 18),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "ordinaryCapture",
        "com.example.observability.OrderObserver.ordinaryCapture",
        file,
        (20, 22),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_observability_config_edge(
        &synth,
        "observability:sentry",
        "cbm:1:com.example.observability.OrderObserver.report",
        "java-sentry-capture-exception",
    );
    assert_observability_config_edge(
        &synth,
        "observability:sentry",
        "cbm:1:com.example.observability.OrderObserver.report",
        "java-sentry-capture-message",
    );
    assert_observability_config_edge(
        &synth,
        "observability:opentelemetry",
        "cbm:2:com.example.observability.OrderObserver.traceOrder",
        "java-opentelemetry-tracer",
    );
    assert_observability_config_edge(
        &synth,
        "observability:opentelemetry",
        "cbm:2:com.example.observability.OrderObserver.traceOrder",
        "java-opentelemetry-span-builder",
    );
    assert_observability_config_edge(
        &synth,
        "observability:opentelemetry",
        "cbm:2:com.example.observability.OrderObserver.traceOrder",
        "java-opentelemetry-start-span",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id
            == "cbm:3:com.example.observability.OrderObserver.ordinaryCapture"));
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
    let edges = resource.edge_recs();
    assert!(
        edges.iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }),
        "expected edge source={source_id} strategy={strategy}; edges={edges:#?}"
    );
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
