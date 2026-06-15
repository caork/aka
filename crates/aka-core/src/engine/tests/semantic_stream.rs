use super::*;
use serde_json::json;

#[test]
fn synthesizes_spring_cloud_stream_topics() {
    let repo = temp_repo("spring-cloud-stream-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/java/com/example/orders/OrderStreams.java";
    std::fs::write(
        repo.join("src/main/resources/application.yml"),
        r#"spring:
  cloud:
    stream:
      bindings:
        orderEvents-out-0:
          destination: orders.events
        legacyOrders:
          destination: orders.legacy
          group: legacy-service
        processOrder-in-0:
          destination: orders.incoming
          group: processor-service
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import java.util.function.Consumer;
import org.springframework.cloud.stream.annotation.StreamListener;
import org.springframework.cloud.stream.function.StreamBridge;
import org.springframework.context.annotation.Bean;

class OrderStreams {
    private final StreamBridge bridge;

    void publish(OrderEvent event) {
        bridge.send("orderEvents-out-0", event);
    }

    @StreamListener("legacyOrders")
    public void legacy(OrderEvent event) {}

    @Bean
    Consumer<OrderEvent> processOrder() {
        return event -> {};
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Method",
            "publish",
            "com.example.orders.OrderStreams.publish",
            file,
        ),
        (10, 12),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "legacy",
            "com.example.orders.OrderStreams.legacy",
            file,
        ),
        (15, 16),
        json!({
            "decorators": ["@StreamListener(\"legacyOrders\")"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "processOrder",
            "com.example.orders.OrderStreams.processOrder",
            file,
        ),
        (18, 21),
        json!({
            "decorators": ["@Bean"],
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let events = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.events")
        .expect("stream bridge destination");
    assert_eq!(events.broker, "spring-cloud-stream");
    assert_eq!(events.producers.len(), 1);
    assert_eq!(
        events.producers[0].node_id,
        "cbm:1:com.example.orders.OrderStreams.publish"
    );
    assert_eq!(
        events.producers[0].strategy,
        "java-spring-cloud-stream-bridge-send"
    );

    let legacy = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.legacy")
        .expect("stream listener destination");
    assert_eq!(legacy.consumers.len(), 1);
    assert_eq!(
        legacy.consumers[0].node_id,
        "cbm:2:com.example.orders.OrderStreams.legacy"
    );
    assert_eq!(legacy.consumer_groups, vec!["legacy-service"]);

    let incoming = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.incoming")
        .expect("functional consumer destination");
    assert_eq!(incoming.consumers.len(), 1);
    assert_eq!(
        incoming.consumers[0].node_id,
        "cbm:3:com.example.orders.OrderStreams.processOrder"
    );
    assert_eq!(
        incoming.consumers[0].strategy,
        "java-spring-cloud-stream-function-consumer"
    );
    assert_eq!(incoming.consumer_groups, vec!["processor-service"]);
}

#[test]
fn synthesizes_spring_cloud_stream_properties_bindings() {
    let repo = temp_repo("spring-cloud-stream-properties-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/java/com/example/orders/OrderStreamBridge.java";
    std::fs::write(
        repo.join("src/main/resources/application.properties"),
        r#"spring.cloud.stream.bindings.auditOut-out-0.destination=orders.audit
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

class OrderStreamBridge {
    private org.springframework.cloud.stream.function.StreamBridge bridge;

    void audit(OrderEvent event) {
        bridge.send("auditOut-out-0", event);
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Method",
            "audit",
            "com.example.orders.OrderStreamBridge.audit",
            file,
        ),
        (6, 8),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let audit = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.audit")
        .expect("properties destination");
    assert_eq!(audit.broker, "spring-cloud-stream");
    assert_eq!(audit.producers.len(), 1);
}

#[test]
fn spring_cloud_stream_synthesis_uses_project_sources_and_excludes_tests() {
    let repo = temp_repo("spring-cloud-stream-project-sources");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("scratch")).unwrap();
    std::fs::write(repo.join(".gitignore"), "scratch/\n").unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    std::fs::write(
        repo.join("src/main/resources/application.properties"),
        r#"spring.cloud.stream.bindings.trackedOut-out-0.destination=orders.tracked
spring.cloud.stream.bindings.untrackedOut-out-0.destination=orders.untracked
spring.cloud.stream.bindings.testOut-out-0.destination=orders.test
spring.cloud.stream.bindings.ignoredOut-out-0.destination=orders.ignored
"#,
    )
    .unwrap();

    let tracked_file = "src/main/java/com/example/orders/TrackedBridge.java";
    let untracked_file = "src/main/java/com/example/orders/UntrackedBridge.java";
    let test_file = "src/test/java/com/example/orders/TestBridge.java";
    let ignored_file = "scratch/IgnoredBridge.java";
    write_stream_bridge_source(&repo, tracked_file, "TrackedBridge", "trackedOut-out-0");
    write_stream_bridge_source(
        &repo,
        untracked_file,
        "UntrackedBridge",
        "untrackedOut-out-0",
    );
    write_stream_bridge_source(&repo, test_file, "TestBridge", "testOut-out-0");
    write_stream_bridge_source(&repo, ignored_file, "IgnoredBridge", "ignoredOut-out-0");
    run_git(
        &repo,
        &[
            "add",
            ".gitignore",
            "pom.xml",
            "src/main/resources/application.properties",
            tracked_file,
            test_file,
        ],
    );

    let conn = test_conn();
    insert_stream_bridge_node(&conn, 1, tracked_file, "TrackedBridge");
    insert_stream_bridge_node(&conn, 2, untracked_file, "UntrackedBridge");
    insert_stream_bridge_node(&conn, 3, test_file, "TestBridge");
    insert_stream_bridge_node(&conn, 4, ignored_file, "IgnoredBridge");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let topics: BTreeSet<_> = synth
        .topics
        .iter()
        .map(|topic| topic.name.as_str())
        .collect();
    assert!(topics.contains("orders.tracked"));
    assert!(topics.contains("orders.untracked"));
    assert!(!topics.contains("orders.test"));
    assert!(!topics.contains("orders.ignored"));
}

fn write_stream_bridge_source(
    repo: &std::path::Path,
    file_path: &str,
    class_name: &str,
    binding: &str,
) {
    std::fs::write(
        repo.join(file_path),
        format!(
            r#"package com.example.orders;

class {class_name} {{
    private org.springframework.cloud.stream.function.StreamBridge bridge;

    void publish(OrderEvent event) {{
        bridge.send("{binding}", event);
    }}
}}
"#
        ),
    )
    .unwrap();
}

fn insert_stream_bridge_node(conn: &Connection, id: i64, file_path: &str, class_name: &str) {
    insert_node_props_at(
        conn,
        id,
        (
            "Method",
            "publish",
            &format!("com.example.orders.{class_name}.publish"),
            file_path,
        ),
        (6, 8),
        json!({
            "language": "java",
        }),
    );
}
