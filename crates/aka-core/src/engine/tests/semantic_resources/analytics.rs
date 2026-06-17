use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_analytics_resources() {
    let repo = temp_repo("configured-analytics-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"analytics:
  segment:
    write-key: seg-redacted
  amplitude:
    api-key: amp-redacted
  mixpanel:
    token: mp-redacted
  posthog:
    host: https://app.posthog.com
disabled:
  segment:
    write-key: ${SEGMENT_WRITE_KEY}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 13),
        json!({"language": "yaml"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_analytics_edge(
        &synth,
        "analytics:segment",
        &config_id("analytics.segment.write.key"),
        "segment-config",
    );
    assert_analytics_edge(
        &synth,
        "analytics:amplitude",
        &config_id("analytics.amplitude.api.key"),
        "amplitude-config",
    );
    assert_analytics_edge(
        &synth,
        "analytics:mixpanel",
        &config_id("analytics.mixpanel.token"),
        "mixpanel-config",
    );
    assert_analytics_edge(
        &synth,
        "analytics:posthog",
        &config_id("analytics.posthog.host"),
        "posthog-config",
    );
    assert!(!synth.resources.iter().any(|resource| {
        resource.url == "analytics:segment"
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("disabled.segment.write.key"))
    }));
}

#[test]
fn synthesizes_python_analytics_resources() {
    let repo = temp_repo("python-analytics-resources");
    std::fs::write(
        repo.join("events.py"),
        r#"import analytics
from amplitude import Amplitude
from mixpanel import Mixpanel
from posthog import Posthog

amplitude = Amplitude("amp-key")
mixpanel = Mixpanel("mp-token")
posthog = Posthog("ph-key", host="https://app.posthog.com")

def track_signup(user_id):
    analytics.track(user_id, "Signed Up")
    analytics.identify(user_id, {"plan": "pro"})

def track_checkout(user_id):
    amplitude.track(user_id, "Checkout Started")

def track_mixpanel(user_id):
    mixpanel.track(user_id, "Order Completed")

def track_posthog(user_id):
    posthog.capture(user_id, "Viewed Product")

def ordinary(client):
    client.track("not analytics")
    client.capture("not analytics")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "track_signup",
        "events.track_signup",
        "events.py",
        (10, 12),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "track_checkout",
        "events.track_checkout",
        "events.py",
        (14, 15),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "track_mixpanel",
        "events.track_mixpanel",
        "events.py",
        (17, 18),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "track_posthog",
        "events.track_posthog",
        "events.py",
        (20, 21),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "ordinary",
        "events.ordinary",
        "events.py",
        (23, 25),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_analytics_edge(
        &synth,
        "analytics:segment",
        "cbm:1:events.track_signup",
        "python-segment-track",
    );
    assert_analytics_edge(
        &synth,
        "analytics:segment",
        "cbm:1:events.track_signup",
        "python-segment-identify",
    );
    assert_analytics_edge(
        &synth,
        "analytics:amplitude",
        "cbm:2:events.track_checkout",
        "python-amplitude-track",
    );
    assert_analytics_edge(
        &synth,
        "analytics:mixpanel",
        "cbm:3:events.track_mixpanel",
        "python-mixpanel-track",
    );
    assert_analytics_edge(
        &synth,
        "analytics:posthog",
        "cbm:4:events.track_posthog",
        "python-posthog-capture",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:5:events.ordinary"));
}

#[test]
fn synthesizes_java_analytics_resources() {
    let repo = temp_repo("java-analytics-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/events")).unwrap();
    let file = "src/main/java/com/example/events/EventGateway.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.events;

import com.segment.analytics.Analytics;

class EventGateway {
    void segment(Analytics analytics, Object message) {
        analytics.enqueue(message);
    }

    void amplitude(Amplitude amplitude, String userId) {
        amplitude.track(userId, "Checkout Started");
    }

    void mixpanel(MixpanelAPI mixpanel, String userId) {
        mixpanel.track(userId, "Order Completed");
    }

    void posthog(PostHog postHog, String userId) {
        postHog.capture(userId, "Viewed Product");
    }

    void ordinary(Client client, String userId) {
        client.track(userId, "Nope");
        client.capture(userId, "Nope");
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "segment",
        "com.example.events.EventGateway.segment",
        file,
        (6, 8),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "amplitude",
        "com.example.events.EventGateway.amplitude",
        file,
        (10, 12),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "mixpanel",
        "com.example.events.EventGateway.mixpanel",
        file,
        (14, 16),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "posthog",
        "com.example.events.EventGateway.posthog",
        file,
        (18, 20),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "ordinary",
        "com.example.events.EventGateway.ordinary",
        file,
        (22, 25),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_analytics_edge(
        &synth,
        "analytics:segment",
        "cbm:1:com.example.events.EventGateway.segment",
        "java-segment-enqueue",
    );
    assert_analytics_edge(
        &synth,
        "analytics:amplitude",
        "cbm:2:com.example.events.EventGateway.amplitude",
        "java-amplitude-track",
    );
    assert_analytics_edge(
        &synth,
        "analytics:mixpanel",
        "cbm:3:com.example.events.EventGateway.mixpanel",
        "java-mixpanel-track",
    );
    assert_analytics_edge(
        &synth,
        "analytics:posthog",
        "cbm:4:com.example.events.EventGateway.posthog",
        "java-posthog-capture",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:5:com.example.events.EventGateway.ordinary"));
}

fn assert_analytics_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected analytics resource {url}"));
    assert_eq!(resource.resource_type, "analytics");
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
