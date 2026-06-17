use super::{infra_config, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn extract_secret_provider_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_secret_context(text) {
        out.extend(extract_python_secret_providers(text, nodes));
    }
    if has_java_secret_context(text) {
        out.extend(extract_java_secret_providers(text, nodes));
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

pub(super) fn extract_secret_provider_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some(provider) = secret_provider_for_config(&key, &value) else {
            continue;
        };
        if !secret_config_value_is_present(&value) {
            continue;
        }
        out.push(ResourceDetection::secret_provider(
            provider.into(),
            infra_config::config_id(&key),
            secret_config_strategy(provider),
        ));
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

fn has_python_secret_context(text: &str) -> bool {
    text.contains("secretsmanager")
        || text.contains("ssm")
        || text.contains("hvac")
        || text.contains("secretmanager")
        || text.contains("SecretManagerServiceClient")
        || text.contains("azure.keyvault.secrets")
}

fn extract_python_secret_providers(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "aws-secrets-manager",
        &[
            "boto3.client(\"secretsmanager\"",
            "boto3.client('secretsmanager'",
        ],
        &[
            (".get_secret_value", "python-aws-secrets-manager-get"),
            (".put_secret_value", "python-aws-secrets-manager-put"),
            (".create_secret", "python-aws-secrets-manager-create"),
            (".update_secret", "python-aws-secrets-manager-update"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "aws-ssm",
        &["boto3.client(\"ssm\"", "boto3.client('ssm'"],
        &[
            (".get_parameter", "python-aws-ssm-get-parameter"),
            (".get_parameters", "python-aws-ssm-get-parameters"),
            (".put_parameter", "python-aws-ssm-put-parameter"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "vault",
        &["hvac.Client("],
        &[
            (".secrets.kv.v2.read_secret_version", "python-vault-kv-read"),
            (
                ".secrets.kv.v2.create_or_update_secret",
                "python-vault-kv-write",
            ),
            (".read", "python-vault-read"),
            (".write", "python-vault-write"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "gcp-secret-manager",
        &["secretmanager.SecretManagerServiceClient("],
        &[
            (".access_secret_version", "python-gcp-secret-access"),
            (".add_secret_version", "python-gcp-secret-add-version"),
            (".create_secret", "python-gcp-secret-create"),
        ],
    ));
    if text.contains("azure.keyvault.secrets") {
        out.extend(extract_python_provider_calls(
            text,
            nodes,
            "azure-key-vault",
            &["SecretClient("],
            &[
                (".get_secret", "python-azure-key-vault-get"),
                (".set_secret", "python-azure-key-vault-set"),
                (".begin_delete_secret", "python-azure-key-vault-delete"),
            ],
        ));
    }
    out
}

fn extract_python_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    constructors: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for call in find_call_args(text, callee) {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !python_receiver_is_provider(text, receiver, provider, constructors) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::secret_provider(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn has_java_secret_context(text: &str) -> bool {
    text.contains("SecretsManagerClient")
        || text.contains("SsmClient")
        || text.contains("VaultTemplate")
        || text.contains("SecretManagerServiceClient")
        || text.contains("com.azure.security.keyvault.secrets")
}

fn extract_java_secret_providers(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "aws-secrets-manager",
        &["SecretsManagerClient", "AWSSecretsManager"],
        &[
            (".getSecretValue", "java-aws-secrets-manager-get"),
            (".putSecretValue", "java-aws-secrets-manager-put"),
            (".createSecret", "java-aws-secrets-manager-create"),
            (".updateSecret", "java-aws-secrets-manager-update"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "aws-ssm",
        &["SsmClient", "AWSSimpleSystemsManagement"],
        &[
            (".getParameter", "java-aws-ssm-get-parameter"),
            (".getParameters", "java-aws-ssm-get-parameters"),
            (".putParameter", "java-aws-ssm-put-parameter"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "vault",
        &["VaultTemplate", "VaultOperations"],
        &[
            (".read", "java-vault-read"),
            (".write", "java-vault-write"),
            (".opsForVersionedKeyValue", "java-vault-kv"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "gcp-secret-manager",
        &["SecretManagerServiceClient"],
        &[
            (".accessSecretVersion", "java-gcp-secret-access"),
            (".addSecretVersion", "java-gcp-secret-add-version"),
            (".createSecret", "java-gcp-secret-create"),
        ],
    ));
    if text.contains("com.azure.security.keyvault.secrets") {
        out.extend(extract_java_provider_calls(
            text,
            nodes,
            "azure-key-vault",
            &["SecretClient"],
            &[
                (".getSecret", "java-azure-key-vault-get"),
                (".setSecret", "java-azure-key-vault-set"),
                (".beginDeleteSecret", "java-azure-key-vault-delete"),
            ],
        ));
    }
    out
}

fn extract_java_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    types: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for call in find_call_args(text, callee) {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            let Some(body) = node_text(text, node) else {
                continue;
            };
            if !java_receiver_has_type(body, receiver, types) {
                continue;
            }
            out.push(ResourceDetection::secret_provider(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn python_receiver_is_provider(
    text: &str,
    receiver: &str,
    provider: &str,
    constructors: &[&str],
) -> bool {
    let receiver = receiver_tail(receiver);
    python_receiver_assigned_to(text, receiver, constructors)
        || python_receiver_from_boto3_client(text, receiver, provider)
}

fn python_receiver_assigned_to(text: &str, receiver: &str, constructors: &[&str]) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            return false;
        };
        let lhs = lhs
            .trim()
            .trim_start_matches("self.")
            .trim_start_matches("cls.");
        lhs == receiver && constructors.iter().any(|ctor| rhs.contains(ctor))
    })
}

fn python_receiver_from_boto3_client(text: &str, receiver: &str, provider: &str) -> bool {
    let service = match provider {
        "aws-secrets-manager" => "secretsmanager",
        "aws-ssm" => "ssm",
        _ => return false,
    };
    text.lines().any(|line| {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            return false;
        };
        lhs.trim()
            .trim_start_matches("self.")
            .trim_start_matches("cls.")
            == receiver
            && rhs.contains("boto3.client")
            && (rhs.contains(&format!("\"{service}\"")) || rhs.contains(&format!("'{service}'")))
    })
}

fn java_receiver_has_type(text: &str, receiver: &str, types: &[&str]) -> bool {
    let receiver = receiver_tail(receiver);
    text.lines().any(|line| {
        let line = line.trim();
        types
            .iter()
            .any(|ty| java_declares_receiver_with_type(line, receiver, ty))
    })
}

fn java_declares_receiver_with_type(line: &str, receiver: &str, ty: &str) -> bool {
    for receiver_pos in receiver_positions(line, receiver) {
        let before = line[..receiver_pos].trim_end();
        let Some(type_end) = before.rfind(|ch: char| !java_type_char(ch)) else {
            continue;
        };
        let found = before[type_end + 1..].trim_end_matches("...");
        if found.rsplit('.').next().unwrap_or(found) == ty {
            return true;
        }
    }
    false
}

fn receiver_positions(line: &str, receiver: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = line[offset..].find(receiver) {
        let start = offset + rel;
        let end = start + receiver.len();
        let before_ok = line[..start]
            .chars()
            .next_back()
            .is_none_or(|ch| !java_ident_char(ch));
        let after_ok = line[end..]
            .chars()
            .next()
            .is_none_or(|ch| !java_ident_char(ch));
        if before_ok && after_ok {
            out.push(start);
        }
        offset = end;
    }
    out
}

fn java_type_char(ch: char) -> bool {
    java_ident_char(ch) || matches!(ch, '.' | '<' | '>' | '?' | '[' | ']')
}

fn java_ident_char(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
}

fn node_text<'a>(text: &'a str, node: &SynthNode) -> Option<&'a str> {
    let start_line = node.start_line_key().max(1) as usize;
    let end_line = node.end_line_key().max(start_line as i64) as usize;
    if start_line > end_line {
        return None;
    }
    let mut line = 1usize;
    let mut start = 0usize;
    let mut end = text.len();
    for (idx, ch) in text.char_indices() {
        if line == start_line {
            start = idx;
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    line = 1;
    for (idx, ch) in text.char_indices() {
        if line > end_line {
            end = idx;
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    Some(&text[start..end])
}

fn receiver_before_dot(text: &str, dot_start: usize) -> Option<&str> {
    if text.as_bytes().get(dot_start) != Some(&b'.') {
        return None;
    }
    let mut start = dot_start;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if ch == '.' || ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let receiver = text[start..dot_start].trim_matches('.');
    (!receiver.is_empty()).then_some(receiver)
}

fn receiver_tail(receiver: &str) -> &str {
    receiver.rsplit('.').next().unwrap_or(receiver)
}

fn secret_provider_for_config(key: &str, value: &str) -> Option<&'static str> {
    let value = value.to_ascii_lowercase();
    if key.contains("secretsmanager")
        || key.contains("secrets.manager")
        || value.contains("secretsmanager.")
        || value.contains("arn:aws:secretsmanager")
    {
        Some("aws-secrets-manager")
    } else if key.contains("parameter.store")
        || key.contains("aws.ssm")
        || key.ends_with(".ssm.path")
        || value.contains("arn:aws:ssm")
    {
        Some("aws-ssm")
    } else if key.contains("keyvault")
        || key.contains("key.vault")
        || value.contains(".vault.azure.net")
    {
        Some("azure-key-vault")
    } else if key.contains("secretmanager")
        || key.contains("secret.manager")
        || value.contains("secretmanager.googleapis.com")
    {
        Some("gcp-secret-manager")
    } else if key.contains("vault") || value.contains("vault://") || value.contains("vault.") {
        Some("vault")
    } else {
        None
    }
}

fn secret_config_value_is_present(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '`']);
    !value.is_empty()
        && !value.starts_with("${")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "none" | "null" | "0"
        )
}

fn secret_config_strategy(provider: &str) -> &'static str {
    match provider {
        "aws-secrets-manager" => "aws-secrets-manager-config",
        "aws-ssm" => "aws-ssm-config",
        "vault" => "vault-config",
        "gcp-secret-manager" => "gcp-secret-manager-config",
        "azure-key-vault" => "azure-key-vault-config",
        _ => "secret-provider-config",
    }
}
