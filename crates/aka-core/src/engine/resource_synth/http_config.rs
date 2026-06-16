use super::{infra_config, ResourceDetection};

pub(super) fn extract_http_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        if !is_http_service_key(&key) {
            continue;
        }
        let Some(url) = normalize_http_config_url(&value) else {
            continue;
        };
        out.push(ResourceDetection::http(
            url,
            infra_config::config_id(&key),
            "http-config-url",
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

fn is_http_service_key(key: &str) -> bool {
    if is_identity_key(key) || is_storage_key(key) {
        return false;
    }
    matches!(
        key,
        "url"
            | "uri"
            | "base.url"
            | "base.uri"
            | "baseurl"
            | "endpoint"
            | "endpoint.url"
            | "service.url"
            | "api.url"
    ) || key.ends_with(".url")
        || key.ends_with(".uri")
        || key.ends_with(".base.url")
        || key.ends_with(".base.uri")
        || key.ends_with(".baseurl")
        || key.ends_with(".base-url")
        || key.ends_with(".endpoint")
        || key.ends_with(".endpoint.url")
        || key.ends_with(".api.url")
        || key.ends_with(".service.url")
}

fn is_identity_key(key: &str) -> bool {
    key.contains("issuer")
        || key.contains("jwk")
        || key.contains("jwks")
        || key.contains("oauth")
        || key.contains("openid")
        || key.contains("auth.server")
        || key.contains("auth0")
        || key.contains("keycloak")
        || key.contains("cognito")
}

fn is_storage_key(key: &str) -> bool {
    key.contains("s3.")
        || key.contains(".s3")
        || key.contains("bucket")
        || key.contains("blob")
        || key.contains("gcs")
        || key.contains("storage")
}

fn normalize_http_config_url(value: &str) -> Option<String> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    let prefix_len = if lower.starts_with("https://") {
        "https://".len()
    } else if lower.starts_with("http://") {
        "http://".len()
    } else {
        return None;
    };
    let scheme = &value[..prefix_len];
    let rest = &value[prefix_len..];
    let endpoint = rest
        .split_once('?')
        .map(|(value, _)| value)
        .unwrap_or(rest)
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | ',' | ';'))
        .trim_end_matches('/');
    if endpoint.is_empty() || endpoint.starts_with("${") || endpoint.contains(char::is_whitespace) {
        return None;
    }
    Some(format!("{scheme}{endpoint}"))
}
