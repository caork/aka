use super::{read_string_literal, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn extract_identity_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_identity_context(text) {
        out.extend(extract_configured_identity_resources(text, nodes));
        out.extend(extract_python_identity_resources(text, nodes));
        out.extend(extract_java_identity_resources(text, nodes));
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then_with(|| a.node_id.cmp(&b.node_id)));
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn has_identity_context(text: &str) -> bool {
    text.contains("issuer-uri")
        || text.contains("jwk-set-uri")
        || text.contains("jwks_uri")
        || text.contains(".well-known/jwks")
        || text.contains("auth0")
        || text.contains("keycloak")
        || text.contains("cognito")
        || text.contains("OAuth2")
        || text.contains("JwtDecoder")
        || text.contains("jwt.decode")
        || text.contains("PyJWKClient")
}

fn extract_configured_identity_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if !looks_like_identity_config_line(trimmed) {
            continue;
        }
        let Some(url) = first_url_like_value(trimmed) else {
            continue;
        };
        let Some(node) = first_config_node(nodes).or_else(|| nodes.first().copied()) else {
            continue;
        };
        out.push(ResourceDetection::identity(
            identity_provider(&url),
            node.aka_id.clone(),
            "identity-config-url",
        ));
    }
    out
}

fn extract_python_identity_resources(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, "PyJWKClient") {
        let Some(url) = first_literal_url(call.args) else {
            continue;
        };
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        out.push(ResourceDetection::identity(
            identity_provider(&url),
            node.aka_id.clone(),
            "python-pyjwk-client",
        ));
    }
    for call in find_call_args(text, "jwt.decode") {
        let Some(url) = first_literal_url(call.args) else {
            continue;
        };
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        out.push(ResourceDetection::identity(
            identity_provider(&url),
            node.aka_id.clone(),
            "python-jwt-decode-issuer",
        ));
    }
    if text.contains("boto3") || text.contains("cognito") {
        for call in find_call_args(text, "boto3.client") {
            if !call.args.contains("cognito-idp") {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::identity(
                "cognito".into(),
                node.aka_id.clone(),
                "python-boto3-cognito-idp",
            ));
        }
    }
    out
}

fn extract_java_identity_resources(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, "JwtDecoders.fromIssuerLocation") {
        let Some(url) = first_literal_url(call.args) else {
            continue;
        };
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        out.push(ResourceDetection::identity(
            identity_provider(&url),
            node.aka_id.clone(),
            "java-spring-jwt-issuer",
        ));
    }
    for call in find_call_args(text, "NimbusJwtDecoder.withJwkSetUri") {
        let Some(url) = first_literal_url(call.args) else {
            continue;
        };
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        out.push(ResourceDetection::identity(
            identity_provider(&url),
            node.aka_id.clone(),
            "java-spring-jwk-set-uri",
        ));
    }
    if text.contains("CognitoIdentityProviderClient") {
        for call in find_call_args(text, "CognitoIdentityProviderClient.builder") {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::identity(
                "cognito".into(),
                node.aka_id.clone(),
                "java-aws-cognito-client",
            ));
        }
    }
    out
}

fn looks_like_identity_config_line(line: &str) -> bool {
    let Some((key, _)) = line.split_once([':', '=']) else {
        return false;
    };
    matches!(
        key.trim(),
        "issuer-uri" | "jwk-set-uri" | "jwks_uri" | "auth-server-url"
    )
}

fn first_config_node<'a>(nodes: &[&'a SynthNode]) -> Option<&'a SynthNode> {
    nodes
        .iter()
        .copied()
        .find(|node| matches!(node.label.as_str(), "Config" | "File" | "Module"))
}

fn first_url_like_value(text: &str) -> Option<String> {
    if let Some(url_start) = text.find("http://").or_else(|| text.find("https://")) {
        let rest = &text[url_start..];
        let end = rest
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | ')' | ']'))
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches(['/', ';']).to_string();
        if is_identity_url(&url) {
            return Some(url);
        }
    }
    first_literal_url(text)
}

fn first_literal_url(text: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < text.len() {
        let Some((literal, end)) = read_string_literal(text, idx) else {
            idx += text[idx..].chars().next().map(char::len_utf8).unwrap_or(1);
            continue;
        };
        if is_identity_url(&literal) {
            return Some(literal.trim_end_matches('/').to_string());
        }
        idx = end;
    }
    None
}

fn is_identity_url(value: &str) -> bool {
    (value.starts_with("https://") || value.starts_with("http://"))
        && (value.contains("auth0")
            || value.contains("keycloak")
            || value.contains("cognito")
            || value.contains("issuer")
            || value.contains("oauth")
            || value.contains("openid")
            || value.contains("jwks")
            || value.contains(".well-known"))
}

fn identity_provider(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if lower.contains("auth0") {
        "auth0".into()
    } else if lower.contains("keycloak") {
        "keycloak".into()
    } else if lower.contains("cognito") {
        "cognito".into()
    } else if lower.contains("jwks") || lower.contains("jwk") {
        "jwks".into()
    } else {
        "oidc".into()
    }
}
