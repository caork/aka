use super::super::*;
use crate::engine::resource_synth::infra_config::config_id;
use serde_json::json;

#[test]
fn synthesizes_configured_secret_provider_resources() {
    let repo = temp_repo("configured-secret-provider-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"aws:
  secretsmanager:
    secret-id: prod/orders/db
  parameter-store:
    path: /prod/orders
spring:
  cloud:
    vault:
      uri: https://vault.internal
gcp:
  secret-manager:
    project-id: aka-prod
azure:
  key-vault:
    endpoint: https://aka.vault.azure.net/
ordinary:
  api-secret: literal-secret-value
disabled:
  vault:
    uri: ${VAULT_ADDR}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 18),
        json!({"language": "yaml"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_secret_provider_edge(
        &synth,
        "secret-provider:aws-secrets-manager",
        &config_id("aws.secretsmanager.secret.id"),
        "aws-secrets-manager-config",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:aws-ssm",
        &config_id("aws.parameter.store.path"),
        "aws-ssm-config",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:vault",
        &config_id("spring.cloud.vault.uri"),
        "vault-config",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:gcp-secret-manager",
        &config_id("gcp.secret.manager.project.id"),
        "gcp-secret-manager-config",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:azure-key-vault",
        &config_id("azure.key.vault.endpoint"),
        "azure-key-vault-config",
    );
    assert!(!synth.resources.iter().any(|resource| {
        resource.url.starts_with("secret-provider:")
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("ordinary.api.secret"))
    }));
    assert!(!synth.resources.iter().any(|resource| {
        resource.url == "secret-provider:vault"
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("disabled.vault.uri"))
    }));
}

#[test]
fn synthesizes_python_secret_provider_resources() {
    let repo = temp_repo("python-secret-provider-resources");
    std::fs::write(
        repo.join("secret_ops.py"),
        r#"import boto3
import hvac
from google.cloud import secretmanager
from azure.keyvault.secrets import SecretClient

aws_secrets = boto3.client("secretsmanager")
ssm = boto3.client("ssm")
vault = hvac.Client(url="https://vault.internal")
gcp = secretmanager.SecretManagerServiceClient()
azure = SecretClient(vault_url="https://aka.vault.azure.net/", credential=credential)

def load_aws_secret():
    return aws_secrets.get_secret_value(SecretId="prod/orders/db")

def write_ssm_parameter():
    return ssm.put_parameter(Name="/prod/orders/db", Value="dsn", Type="SecureString")

def load_vault_secret():
    return vault.secrets.kv.v2.read_secret_version(path="orders/db")

def load_gcp_secret():
    return gcp.access_secret_version(name="projects/aka/secrets/db/versions/latest")

def load_azure_secret():
    return azure.get_secret("db-password")

def ordinary(secret_client, vault_client):
    secret_client.get_secret_value(SecretId="local")
    return vault_client.read("local/path")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "load_aws_secret",
        "secret_ops.load_aws_secret",
        "secret_ops.py",
        (12, 13),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "write_ssm_parameter",
        "secret_ops.write_ssm_parameter",
        "secret_ops.py",
        (15, 16),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "load_vault_secret",
        "secret_ops.load_vault_secret",
        "secret_ops.py",
        (18, 19),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "load_gcp_secret",
        "secret_ops.load_gcp_secret",
        "secret_ops.py",
        (21, 22),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "load_azure_secret",
        "secret_ops.load_azure_secret",
        "secret_ops.py",
        (24, 25),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        6,
        "ordinary",
        "secret_ops.ordinary",
        "secret_ops.py",
        (27, 29),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_secret_provider_edge(
        &synth,
        "secret-provider:aws-secrets-manager",
        "cbm:1:secret_ops.load_aws_secret",
        "python-aws-secrets-manager-get",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:aws-ssm",
        "cbm:2:secret_ops.write_ssm_parameter",
        "python-aws-ssm-put-parameter",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:vault",
        "cbm:3:secret_ops.load_vault_secret",
        "python-vault-kv-read",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:gcp-secret-manager",
        "cbm:4:secret_ops.load_gcp_secret",
        "python-gcp-secret-access",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:azure-key-vault",
        "cbm:5:secret_ops.load_azure_secret",
        "python-azure-key-vault-get",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:6:secret_ops.ordinary"));
}

#[test]
fn synthesizes_java_secret_provider_resources() {
    let repo = temp_repo("java-secret-provider-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/secrets")).unwrap();
    let file = "src/main/java/com/example/secrets/SecretGateway.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.secrets;

import software.amazon.awssdk.services.secretsmanager.SecretsManagerClient;
import software.amazon.awssdk.services.ssm.SsmClient;
import org.springframework.vault.core.VaultTemplate;
import com.google.cloud.secretmanager.v1.SecretManagerServiceClient;
import com.azure.security.keyvault.secrets.SecretClient;

class SecretGateway {
    Object aws(SecretsManagerClient secrets, Object request) {
        return secrets.getSecretValue(request);
    }

    Object ssm(SsmClient ssm, Object request) {
        return ssm.getParameter(request);
    }

    Object vault(VaultTemplate vault) {
        return vault.read("secret/orders/db");
    }

    Object gcp(SecretManagerServiceClient secretManager, Object name) {
        return secretManager.accessSecretVersion(name);
    }

    Object azure(SecretClient client) {
        return client.getSecret("db-password");
    }

    Object ordinary(Client client, SecretService secretService) {
        client.getSecret("local");
        return secretService.getSecretValue("local");
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "aws",
        "com.example.secrets.SecretGateway.aws",
        file,
        (10, 12),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "ssm",
        "com.example.secrets.SecretGateway.ssm",
        file,
        (14, 16),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "vault",
        "com.example.secrets.SecretGateway.vault",
        file,
        (18, 20),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "gcp",
        "com.example.secrets.SecretGateway.gcp",
        file,
        (22, 24),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "azure",
        "com.example.secrets.SecretGateway.azure",
        file,
        (26, 28),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        6,
        "ordinary",
        "com.example.secrets.SecretGateway.ordinary",
        file,
        (30, 33),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_secret_provider_edge(
        &synth,
        "secret-provider:aws-secrets-manager",
        "cbm:1:com.example.secrets.SecretGateway.aws",
        "java-aws-secrets-manager-get",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:aws-ssm",
        "cbm:2:com.example.secrets.SecretGateway.ssm",
        "java-aws-ssm-get-parameter",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:vault",
        "cbm:3:com.example.secrets.SecretGateway.vault",
        "java-vault-read",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:gcp-secret-manager",
        "cbm:4:com.example.secrets.SecretGateway.gcp",
        "java-gcp-secret-access",
    );
    assert_secret_provider_edge(
        &synth,
        "secret-provider:azure-key-vault",
        "cbm:5:com.example.secrets.SecretGateway.azure",
        "java-azure-key-vault-get",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:6:com.example.secrets.SecretGateway.ordinary"));
}

fn assert_secret_provider_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected secret provider resource {url}"));
    assert_eq!(resource.resource_type, "secret-provider");
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
