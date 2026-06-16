use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_spring_storage_config_resources() {
    let repo = temp_repo("spring-storage-config-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"storage:
  s3:
    bucket: order-artifacts
  gcs:
    bucket: gcp-order-artifacts
azure:
  storage:
    blob:
      container: order-artifacts
aws:
  s3:
    endpoint: https://s3.internal
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 12),
        json!({
            "language": "yaml",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_storage_config_edge(
        &synth,
        "s3://order-artifacts",
        "s3",
        &config_id("storage.s3.bucket"),
        "s3-config-bucket",
    );
    assert_storage_config_edge(
        &synth,
        "gs://gcp-order-artifacts",
        "gcs",
        &config_id("storage.gcs.bucket"),
        "gcs-config-bucket",
    );
    assert_storage_config_edge(
        &synth,
        "azblob://order-artifacts",
        "azure-blob",
        &config_id("azure.storage.blob.container"),
        "azure-blob-config-container",
    );
    assert!(synth
        .resources
        .iter()
        .all(|resource| resource.url != "s3://s3.internal"));
}

#[test]
fn synthesizes_python_storage_config_resources() {
    let repo = temp_repo("python-storage-config-resources");
    std::fs::write(
        repo.join(".env"),
        r#"AWS_S3_BUCKET=order-artifacts
GCS_BUCKET=gs://gcp-order-artifacts
AZURE_STORAGE_CONTAINER=azblob://order-artifacts
REPORTS_S3_BUCKET=${REPORTS_BUCKET}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"GOOGLE_CLOUD_STORAGE_BUCKET = "gcp-media-artifacts"
AZURE_BLOB_CONTAINER = "media-artifacts"
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", ".env", ".env", ".env"),
        (1, 4),
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
    assert_storage_config_edge(
        &synth,
        "s3://order-artifacts",
        "s3",
        &config_id("aws.s3.bucket"),
        "s3-config-bucket",
    );
    assert_storage_config_edge(
        &synth,
        "gs://gcp-order-artifacts",
        "gcs",
        &config_id("gcs.bucket"),
        "gcs-config-bucket",
    );
    assert_storage_config_edge(
        &synth,
        "azblob://order-artifacts",
        "azure-blob",
        &config_id("azure.storage.container"),
        "azure-blob-config-container",
    );
    assert_storage_config_edge(
        &synth,
        "gs://gcp-media-artifacts",
        "gcs",
        &config_id("google.cloud.storage.bucket"),
        "gcs-config-bucket",
    );
    assert_storage_config_edge(
        &synth,
        "azblob://media-artifacts",
        "azure-blob",
        &config_id("azure.blob.container"),
        "azure-blob-config-container",
    );
    assert!(synth
        .resources
        .iter()
        .all(|resource| resource.url != "s3://${REPORTS_BUCKET}"));
}

fn assert_storage_config_edge(
    synth: &SynthGraph,
    url: &str,
    resource_type: &str,
    source_id: &str,
    strategy: &str,
) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected storage config resource {url}"));
    assert_eq!(resource.resource_type, resource_type);
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == source_id
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == strategy
    }));
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
