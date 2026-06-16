use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_python_boto3_s3_resources() {
    let repo = temp_repo("python-boto3-s3-resources");
    std::fs::write(
        repo.join("storage.py"),
        r#"import boto3

s3 = boto3.client("s3")

def store_receipt(order_id, body):
    s3.put_object(Bucket="order-artifacts", Key=f"receipts/{order_id}.json", Body=body)

def load_manifest():
    return s3.get_object("order-artifacts", "manifests/latest.json")["Body"].read()
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "store_receipt",
        "storage.store_receipt",
        "storage.py",
        (5, 6),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_manifest",
        "storage.load_manifest",
        "storage.py",
        (8, 9),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let receipt = synth
        .resources
        .iter()
        .find(|resource| resource.url == "s3://order-artifacts/receipts/{param}.json")
        .expect("S3 receipt resource");
    assert_eq!(receipt.resource_type, "s3");
    let receipt_edge = receipt
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:1:storage.store_receipt")
        .expect("S3 put edge");
    assert_eq!(receipt_edge.edge_type, "ACCESSES_RESOURCE");
    assert_eq!(
        receipt_edge.evidence.as_ref().unwrap()["strategy"],
        "python-boto3-s3-put-object"
    );

    let manifest = synth
        .resources
        .iter()
        .find(|resource| resource.url == "s3://order-artifacts/manifests/latest.json")
        .expect("S3 manifest resource");
    assert_eq!(manifest.resource_type, "s3");
    assert!(manifest.edge_recs().iter().any(|edge| {
        edge.source_id == "cbm:2:storage.load_manifest"
            && edge.evidence.as_ref().unwrap()["strategy"] == "python-boto3-s3-get-object"
    }));
}

#[test]
fn synthesizes_python_gcs_resources() {
    let repo = temp_repo("python-gcs-resources");
    std::fs::write(
        repo.join("gcs_storage.py"),
        r#"from google.cloud import storage

client = storage.Client()

def store_receipt(order_id, payload):
    blob = client.bucket("order-artifacts").blob(f"receipts/{order_id}.json")
    blob.upload_from_string(payload)

def load_manifest():
    return client.get_bucket("order-artifacts").blob("manifests/latest.json").download_as_text()
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "store_receipt",
        "gcs_storage.store_receipt",
        "gcs_storage.py",
        (5, 7),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_manifest",
        "gcs_storage.load_manifest",
        "gcs_storage.py",
        (9, 10),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let receipt = synth
        .resources
        .iter()
        .find(|resource| resource.url == "gs://order-artifacts/receipts/{param}.json")
        .expect("GCS receipt resource");
    assert_eq!(receipt.resource_type, "gcs");
    let receipt_edge = receipt
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:1:gcs_storage.store_receipt")
        .expect("GCS upload edge");
    assert_eq!(receipt_edge.edge_type, "ACCESSES_RESOURCE");
    assert_eq!(
        receipt_edge.evidence.as_ref().unwrap()["strategy"],
        "python-gcs-upload"
    );

    assert!(synth.resources.iter().any(|resource| {
        resource.url == "gs://order-artifacts/manifests/latest.json"
            && resource.edge_recs().iter().any(|edge| {
                edge.source_id == "cbm:2:gcs_storage.load_manifest"
                    && edge.evidence.as_ref().unwrap()["strategy"] == "python-gcs-download"
            })
    }));
}

#[test]
fn synthesizes_python_azure_blob_resources() {
    let repo = temp_repo("python-azure-blob-resources");
    std::fs::write(
        repo.join("azure_storage.py"),
        r#"from azure.storage.blob import BlobServiceClient, BlobClient

service = BlobServiceClient.from_connection_string("...")

def store_receipt(order_id, payload):
    blob = service.get_container_client("order-artifacts").get_blob_client(f"receipts/{order_id}.json")
    blob.upload_blob(payload, overwrite=True)

def load_manifest():
    return service.get_container_client("order-artifacts").get_blob_client("manifests/latest.json").download_blob().readall()

def delete_legacy():
    client = BlobClient.from_connection_string("...", container_name="order-artifacts", blob_name="legacy/old.json")
    client.delete_blob()
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "store_receipt",
        "azure_storage.store_receipt",
        "azure_storage.py",
        (5, 7),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_manifest",
        "azure_storage.load_manifest",
        "azure_storage.py",
        (9, 10),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "delete_legacy",
        "azure_storage.delete_legacy",
        "azure_storage.py",
        (12, 14),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let receipt = synth
        .resources
        .iter()
        .find(|resource| resource.url == "azblob://order-artifacts/receipts/{param}.json")
        .expect("Azure Blob receipt resource");
    assert_eq!(receipt.resource_type, "azure-blob");
    let receipt_edge = receipt
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:1:azure_storage.store_receipt")
        .expect("Azure Blob upload edge");
    assert_eq!(receipt_edge.edge_type, "ACCESSES_RESOURCE");
    assert_eq!(
        receipt_edge.evidence.as_ref().unwrap()["strategy"],
        "python-azure-blob-upload"
    );

    assert!(synth.resources.iter().any(|resource| {
        resource.url == "azblob://order-artifacts/manifests/latest.json"
            && resource.edge_recs().iter().any(|edge| {
                edge.source_id == "cbm:2:azure_storage.load_manifest"
                    && edge.evidence.as_ref().unwrap()["strategy"] == "python-azure-blob-download"
            })
    }));
    assert!(synth.resources.iter().any(|resource| {
        resource.url == "azblob://order-artifacts/legacy/old.json"
            && resource.edge_recs().iter().any(|edge| {
                edge.source_id == "cbm:3:azure_storage.delete_legacy"
                    && edge.evidence.as_ref().unwrap()["strategy"] == "python-azure-blob-delete"
            })
    }));
}

#[test]
fn synthesizes_java_aws_s3_resources() {
    let repo = temp_repo("java-aws-s3-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/storage")).unwrap();
    let file = "src/main/java/com/example/storage/ReceiptStorage.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.storage;

import com.amazonaws.services.s3.AmazonS3;
import com.amazonaws.services.s3.model.GetObjectRequest;
import software.amazon.awssdk.services.s3.S3Client;
import software.amazon.awssdk.services.s3.model.PutObjectRequest;

class ReceiptStorage {
    private final AmazonS3 amazonS3;
    private final S3Client s3;

    void writeLegacy(String orderId, java.io.File file) {
        amazonS3.putObject("order-artifacts", "legacy/" + orderId + ".json", file);
    }

    Object readLegacy(String key) {
        return amazonS3.getObject(new GetObjectRequest("order-artifacts", "manifests/latest.json"));
    }

    void writeSdk2(String orderId) {
        PutObjectRequest request = PutObjectRequest.builder()
            .bucket("order-artifacts")
            .key("receipts/" + orderId + ".json")
            .build();
        s3.putObject(request, software.amazon.awssdk.core.sync.RequestBody.empty());
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
            "writeLegacy",
            "com.example.storage.ReceiptStorage.writeLegacy",
            file,
        ),
        (12, 14),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "readLegacy",
            "com.example.storage.ReceiptStorage.readLegacy",
            file,
        ),
        (16, 18),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "writeSdk2",
            "com.example.storage.ReceiptStorage.writeSdk2",
            file,
        ),
        (20, 26),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let legacy_write = synth
        .resources
        .iter()
        .find(|resource| resource.url == "s3://order-artifacts/legacy/{param}.json")
        .expect("legacy S3 put resource");
    assert_eq!(legacy_write.resource_type, "s3");
    let legacy_edge = legacy_write
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:1:com.example.storage.ReceiptStorage.writeLegacy")
        .expect("legacy S3 put edge");
    assert_eq!(legacy_edge.edge_type, "ACCESSES_RESOURCE");
    assert_eq!(
        legacy_edge.evidence.as_ref().unwrap()["strategy"],
        "java-aws-s3-put-object"
    );

    assert!(synth.resources.iter().any(|resource| {
        resource.url == "s3://order-artifacts/manifests/latest.json"
            && resource.edge_recs().iter().any(|edge| {
                edge.source_id == "cbm:2:com.example.storage.ReceiptStorage.readLegacy"
                    && edge.evidence.as_ref().unwrap()["strategy"] == "java-aws-s3-get-object"
            })
    }));

    assert!(synth.resources.iter().any(|resource| {
        resource.url == "s3://order-artifacts/receipts/{param}.json"
            && resource.edge_recs().iter().any(|edge| {
                edge.source_id == "cbm:3:com.example.storage.ReceiptStorage.writeSdk2"
                    && edge.evidence.as_ref().unwrap()["strategy"] == "java-aws-s3-put-object"
            })
    }));
}

#[test]
fn synthesizes_java_azure_blob_resources() {
    let repo = temp_repo("java-azure-blob-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/storage")).unwrap();
    let file = "src/main/java/com/example/storage/AzureReceiptStorage.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.storage;

import com.azure.storage.blob.BlobClient;
import com.azure.storage.blob.BlobClientBuilder;
import com.azure.storage.blob.BlobContainerClient;
import com.azure.storage.blob.BlobServiceClient;

class AzureReceiptStorage {
    private final BlobServiceClient service;

    void storeReceipt(String orderId, java.io.InputStream payload) {
        service.getBlobContainerClient("order-artifacts")
            .getBlobClient("receipts/" + orderId + ".json")
            .upload(payload);
    }

    byte[] loadManifest() {
        BlobContainerClient container = service.getBlobContainerClient("order-artifacts");
        BlobClient blob = container.getBlobClient("manifests/latest.json");
        return blob.downloadContent().toBytes();
    }

    void deleteLegacy() {
        BlobClient client = new BlobClientBuilder()
            .connectionString("...")
            .containerName("order-artifacts")
            .blobName("legacy/old.json")
            .buildClient();
        client.delete();
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
            "storeReceipt",
            "com.example.storage.AzureReceiptStorage.storeReceipt",
            file,
        ),
        (11, 14),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "loadManifest",
            "com.example.storage.AzureReceiptStorage.loadManifest",
            file,
        ),
        (16, 20),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "deleteLegacy",
            "com.example.storage.AzureReceiptStorage.deleteLegacy",
            file,
        ),
        (22, 29),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let receipt = synth
        .resources
        .iter()
        .find(|resource| resource.url == "azblob://order-artifacts/receipts/{param}.json")
        .expect("Azure Blob receipt resource");
    assert_eq!(receipt.resource_type, "azure-blob");
    let receipt_edge = receipt
        .edge_recs()
        .into_iter()
        .find(|edge| edge.source_id == "cbm:1:com.example.storage.AzureReceiptStorage.storeReceipt")
        .expect("Azure Blob upload edge");
    assert_eq!(receipt_edge.edge_type, "ACCESSES_RESOURCE");
    assert_eq!(
        receipt_edge.evidence.as_ref().unwrap()["strategy"],
        "java-azure-blob-upload"
    );

    assert!(synth.resources.iter().any(|resource| {
        resource.url == "azblob://order-artifacts/manifests/latest.json"
            && resource.edge_recs().iter().any(|edge| {
                edge.source_id == "cbm:2:com.example.storage.AzureReceiptStorage.loadManifest"
                    && edge.evidence.as_ref().unwrap()["strategy"] == "java-azure-blob-download"
            })
    }));
    assert!(synth.resources.iter().any(|resource| {
        resource.url == "azblob://order-artifacts/legacy/old.json"
            && resource.edge_recs().iter().any(|edge| {
                edge.source_id == "cbm:3:com.example.storage.AzureReceiptStorage.deleteLegacy"
                    && edge.evidence.as_ref().unwrap()["strategy"] == "java-azure-blob-delete"
            })
    }));
}
