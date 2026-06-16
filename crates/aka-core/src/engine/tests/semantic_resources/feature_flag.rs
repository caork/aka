use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_feature_flag_resources() {
    let repo = temp_repo("configured-feature-flag-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"features:
  checkout:
    flag: checkout-v2
  experiments:
    flags: search-v2,pricing-v3
launchdarkly:
  migration:
    flag-key: ledger-migration
dynamic:
  feature-flag: ${DYNAMIC_FLAG}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"UNLEASH_CHECKOUT_FLAG = "checkout-rollout"
SPLITIO_FLAGS = "catalog-grid,loyalty-offers"
"#,
    )
    .unwrap();
    std::fs::write(repo.join(".env"), "WAFFLE_FLAG=beta-discount\n").unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 10),
        json!({"language": "yaml"}),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Config", "settings.py", "settings.py", "settings.py"),
        (1, 2),
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
    assert_config_flag(
        &synth,
        "feature-flag:checkout-v2",
        &config_id("features.checkout.flag"),
        "feature-flag-config",
    );
    assert_config_flag(
        &synth,
        "feature-flag:search-v2",
        &config_id("features.experiments.flags"),
        "feature-flag-config",
    );
    assert_config_flag(
        &synth,
        "feature-flag:pricing-v3",
        &config_id("features.experiments.flags"),
        "feature-flag-config",
    );
    assert_config_flag(
        &synth,
        "feature-flag:ledger-migration",
        &config_id("launchdarkly.migration.flag.key"),
        "launchdarkly-config-flag",
    );
    assert_config_flag(
        &synth,
        "feature-flag:checkout-rollout",
        &config_id("unleash.checkout.flag"),
        "unleash-config-flag",
    );
    assert_config_flag(
        &synth,
        "feature-flag:catalog-grid",
        &config_id("splitio.flags"),
        "split-config-flag",
    );
    assert_config_flag(
        &synth,
        "feature-flag:loyalty-offers",
        &config_id("splitio.flags"),
        "split-config-flag",
    );
    assert_config_flag(
        &synth,
        "feature-flag:beta-discount",
        &config_id("waffle.flag"),
        "waffle-config-flag",
    );
    assert!(synth
        .resources
        .iter()
        .all(|resource| resource.url != "feature-flag:${DYNAMIC_FLAG}"));
}

#[test]
fn synthesizes_python_feature_flag_resources() {
    let repo = temp_repo("python-feature-flag-resources");
    std::fs::write(
        repo.join("flags.py"),
        r#"import ldclient
from django_waffle import flag_is_active
from UnleashClient import UnleashClient

unleash = UnleashClient("http://flags.internal")

def checkout_enabled(user, request):
    client = ldclient.get()
    return client.variation("checkout-v2", user, False) and flag_is_active(request, "beta-discount")

def loyalty_enabled(user_id):
    return unleash.is_enabled("loyalty.rollout", context={"userId": user_id})
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "checkout_enabled",
        "flags.checkout_enabled",
        "flags.py",
        (7, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "loyalty_enabled",
        "flags.loyalty_enabled",
        "flags.py",
        (11, 12),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (url, source_id, strategy) in [
        (
            "feature-flag:checkout-v2",
            "cbm:1:flags.checkout_enabled",
            "python-launchdarkly-variation",
        ),
        (
            "feature-flag:beta-discount",
            "cbm:1:flags.checkout_enabled",
            "python-django-waffle",
        ),
        (
            "feature-flag:loyalty.rollout",
            "cbm:2:flags.loyalty_enabled",
            "python-unleash-enabled",
        ),
    ] {
        let resource = synth
            .resources
            .iter()
            .find(|resource| resource.url == url)
            .unwrap_or_else(|| panic!("expected feature flag resource {url}"));
        assert_eq!(resource.resource_type, "feature-flag");
        assert!(resource.edge_recs().iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }));
    }
}

fn assert_config_flag(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected feature flag resource {url}"));
    assert_eq!(resource.resource_type, "feature-flag");
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == source_id
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == strategy
    }));
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}

#[test]
fn synthesizes_java_feature_flag_resources() {
    let repo = temp_repo("java-feature-flag-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/flags")).unwrap();
    let file = "src/main/java/com/example/flags/FeatureGate.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.flags;

import com.launchdarkly.sdk.server.LDClient;
import io.getunleash.Unleash;
import org.togglz.core.manager.FeatureManager;

class FeatureGate {
    private final LDClient ldClient;
    private final Unleash unleash;
    private final FeatureManager featureManager;

    boolean checkout(Object user) {
        return ldClient.boolVariation("checkout-v2", user, false);
    }

    boolean loyalty() {
        return unleash.isEnabled("loyalty.rollout");
    }

    boolean invoiceExport() {
        return featureManager.isActive("invoice-export");
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "checkout",
        "com.example.flags.FeatureGate.checkout",
        file,
        (12, 14),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "loyalty",
        "com.example.flags.FeatureGate.loyalty",
        file,
        (16, 18),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "invoiceExport",
        "com.example.flags.FeatureGate.invoiceExport",
        file,
        (20, 22),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (url, source_id, strategy) in [
        (
            "feature-flag:checkout-v2",
            "cbm:1:com.example.flags.FeatureGate.checkout",
            "java-launchdarkly-variation",
        ),
        (
            "feature-flag:loyalty.rollout",
            "cbm:2:com.example.flags.FeatureGate.loyalty",
            "java-unleash-enabled",
        ),
        (
            "feature-flag:invoice-export",
            "cbm:3:com.example.flags.FeatureGate.invoiceExport",
            "java-togglz-active",
        ),
    ] {
        let resource = synth
            .resources
            .iter()
            .find(|resource| resource.url == url)
            .unwrap_or_else(|| panic!("expected feature flag resource {url}"));
        assert_eq!(resource.resource_type, "feature-flag");
        assert!(resource.edge_recs().iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }));
    }
}
