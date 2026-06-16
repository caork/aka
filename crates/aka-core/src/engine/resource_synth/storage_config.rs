use super::{infra_config, ResourceDetection};

pub(super) fn extract_storage_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some(name) = clean_storage_name(&value) else {
            continue;
        };
        if is_s3_bucket_key(&key) {
            out.push(ResourceDetection::s3(
                format!("s3://{name}"),
                infra_config::config_id(&key),
                "s3-config-bucket",
            ));
        } else if is_gcs_bucket_key(&key) {
            out.push(ResourceDetection::gcs(
                format!("gs://{name}"),
                infra_config::config_id(&key),
                "gcs-config-bucket",
            ));
        } else if is_azure_blob_container_key(&key) {
            out.push(ResourceDetection::azure_blob(
                format!("azblob://{name}"),
                infra_config::config_id(&key),
                "azure-blob-config-container",
            ));
        }
    }
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn is_s3_bucket_key(key: &str) -> bool {
    key == "s3.bucket"
        || key == "aws.s3.bucket"
        || key == "aws.s3.bucket.name"
        || key == "cloud.aws.s3.bucket"
        || key == "storage.s3.bucket"
        || key.ends_with(".s3.bucket")
        || key.ends_with(".s3.bucket.name")
        || key.ends_with(".aws.s3.bucket")
        || key.ends_with(".aws.s3.bucket.name")
}

fn is_gcs_bucket_key(key: &str) -> bool {
    key == "gcs.bucket"
        || key == "gcp.storage.bucket"
        || key == "google.cloud.storage.bucket"
        || key == "storage.gcs.bucket"
        || key.ends_with(".gcs.bucket")
        || key.ends_with(".gcp.storage.bucket")
        || key.ends_with(".google.cloud.storage.bucket")
}

fn is_azure_blob_container_key(key: &str) -> bool {
    key == "azure.blob.container"
        || key == "azure.storage.container"
        || key == "azure.storage.blob.container"
        || key == "storage.azure.container"
        || key.ends_with(".azure.blob.container")
        || key.ends_with(".azure.storage.container")
        || key.ends_with(".azure.storage.blob.container")
}

fn clean_storage_name(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('/').to_string();
    if value.is_empty()
        || value.starts_with("${")
        || value.contains(char::is_whitespace)
        || value.starts_with("http://")
        || value.starts_with("https://")
    {
        return None;
    }
    let value = value
        .strip_prefix("s3://")
        .or_else(|| value.strip_prefix("gs://"))
        .or_else(|| value.strip_prefix("azblob://"))
        .unwrap_or(&value)
        .trim_matches('/')
        .to_string();
    (!value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/')))
    .then_some(value)
}
