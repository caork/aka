use super::{stable_hash, ResourceDetection};

pub(super) fn extract_infra_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in config_pairs(text) {
        if let Some(url) = database_resource_url(&key, &value) {
            out.push(ResourceDetection::database(
                url,
                config_id(&key),
                "database-config",
            ));
        }
        if let Some(url) = redis_resource_url(&key, &value) {
            out.push(ResourceDetection::redis(
                url,
                config_id(&key),
                "redis-config",
            ));
        }
        if let Some(url) = kafka_resource_url(&key, &value) {
            out.push(ResourceDetection::kafka(
                url,
                config_id(&key),
                "kafka-config",
            ));
        }
        if let Some(url) = rabbitmq_resource_url(&key, &value) {
            out.push(ResourceDetection::rabbitmq(
                url,
                config_id(&key),
                "rabbitmq-config",
            ));
        }
        if let Some(url) = mongodb_resource_url(&key, &value) {
            out.push(ResourceDetection::mongodb(
                url,
                config_id(&key),
                "mongodb-config",
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

pub(crate) fn config_pairs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut yaml_stack: Vec<(usize, String)> = Vec::new();
    for line in text.lines() {
        let Some((indent, key, value, separator)) = config_line(line) else {
            continue;
        };
        if separator == '=' {
            yaml_stack.clear();
            out.push((key, value));
            continue;
        }
        while yaml_stack
            .last()
            .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
        {
            yaml_stack.pop();
        }
        if value.is_empty() {
            yaml_stack.push((indent, key));
            continue;
        }
        let full_key = yaml_stack
            .iter()
            .map(|(_, key)| key.as_str())
            .chain(std::iter::once(key.as_str()))
            .collect::<Vec<_>>()
            .join(".");
        out.push((full_key, value));
    }
    out
}

fn config_line(line: &str) -> Option<(usize, String, String, char)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
        return None;
    }
    let (key, value, separator) = split_config_key_value(trimmed)?;
    let indent = line.len() - line.trim_start().len();
    let key = normalize_key(key)?;
    let value = clean_config_value(value).unwrap_or_default();
    Some((indent, key, value, separator))
}

fn split_config_key_value(trimmed: &str) -> Option<(&str, &str, char)> {
    let equals = trimmed.find('=');
    let colon = trimmed.find(':');
    let (split_at, separator) = match (equals, colon) {
        (Some(equals), Some(colon)) if equals < colon => (equals, '='),
        (Some(_), Some(colon)) => (colon, ':'),
        (Some(equals), None) => (equals, '='),
        (None, Some(colon)) => (colon, ':'),
        (None, None) => return None,
    };
    let (key, rest) = trimmed.split_at(split_at);
    Some((key, &rest[1..], separator))
}

fn normalize_key(raw: &str) -> Option<String> {
    let key = raw
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\''))
        .trim_start_matches("export ")
        .to_ascii_lowercase()
        .replace(['_', '-'], ".");
    (!key.is_empty()).then_some(key)
}

fn clean_config_value(raw: &str) -> Option<String> {
    let value = raw
        .split_once(" #")
        .map(|(value, _)| value)
        .unwrap_or(raw)
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | ',' | ';'))
        .trim_end_matches(']')
        .trim_start_matches('[')
        .trim()
        .to_string();
    (!value.is_empty() && !value.starts_with("${")).then_some(value)
}

fn database_resource_url(key: &str, value: &str) -> Option<String> {
    if is_database_key(key) || key.ends_with(".datasource.url") {
        if let Some(url) = normalize_database_url(value) {
            return Some(url);
        }
    }
    normalize_database_url(value).filter(|_| key.contains("database") || key.contains("datasource"))
}

fn redis_resource_url(key: &str, value: &str) -> Option<String> {
    if is_redis_key(key) {
        if let Some(url) = normalize_scheme_url(value, &["redis://", "rediss://"], "redis") {
            return Some(url);
        }
        if is_host_like(value) {
            return Some(format!("redis://{}", trim_endpoint(value)));
        }
    }
    normalize_scheme_url(value, &["redis://", "rediss://"], "redis")
        .filter(|_| key.contains("redis"))
}

fn kafka_resource_url(key: &str, value: &str) -> Option<String> {
    if is_kafka_key(key) {
        return endpoint_list_url("kafka", value);
    }
    None
}

fn rabbitmq_resource_url(key: &str, value: &str) -> Option<String> {
    if is_rabbitmq_key(key) {
        if let Some(url) = normalize_scheme_url(value, &["amqp://", "amqps://"], "rabbitmq") {
            return Some(url);
        }
        if is_host_like(value) {
            return Some(format!("rabbitmq://{}", trim_endpoint(value)));
        }
    }
    normalize_scheme_url(value, &["amqp://", "amqps://"], "rabbitmq")
        .filter(|_| key.contains("rabbit") || key.contains("amqp"))
}

fn mongodb_resource_url(key: &str, value: &str) -> Option<String> {
    if is_mongodb_key(key) {
        if let Some(url) = normalize_scheme_url(value, &["mongodb://", "mongodb+srv://"], "mongodb")
        {
            return Some(url);
        }
        if is_host_like(value) {
            return Some(format!("mongodb://{}", trim_endpoint(value)));
        }
    }
    normalize_scheme_url(value, &["mongodb://", "mongodb+srv://"], "mongodb")
        .filter(|_| key.contains("mongo"))
}

fn is_database_key(key: &str) -> bool {
    matches!(
        key,
        "spring.datasource.url"
            | "datasource.url"
            | "database.url"
            | "sqlalchemy.database.uri"
            | "sqlalchemy.database.url"
            | "database.uri"
            | "database.dsn"
            | "database"
    ) || key.ends_with(".datasource.url")
        || key.ends_with(".database.url")
        || key.ends_with(".database.uri")
}

fn is_redis_key(key: &str) -> bool {
    matches!(
        key,
        "redis.url"
            | "redis.uri"
            | "redis.host"
            | "spring.redis.host"
            | "spring.redis.url"
            | "spring.data.redis.host"
            | "spring.data.redis.url"
            | "celery.broker.url"
            | "celery.result.backend"
    ) || key.ends_with(".redis.url")
        || key.ends_with(".redis.uri")
        || key.ends_with(".redis.host")
}

fn is_kafka_key(key: &str) -> bool {
    matches!(
        key,
        "spring.kafka.bootstrap.servers"
            | "kafka.bootstrap.servers"
            | "bootstrap.servers"
            | "kafka.brokers"
            | "kafka.hosts"
    ) || key.ends_with(".kafka.bootstrap.servers")
        || key.ends_with(".bootstrap.servers")
}

fn is_rabbitmq_key(key: &str) -> bool {
    matches!(
        key,
        "spring.rabbitmq.host"
            | "spring.rabbitmq.addresses"
            | "spring.rabbitmq.uri"
            | "rabbitmq.host"
            | "rabbitmq.url"
            | "rabbitmq.uri"
            | "amqp.url"
    ) || key.ends_with(".rabbitmq.host")
        || key.ends_with(".rabbitmq.url")
        || key.ends_with(".rabbitmq.uri")
}

fn is_mongodb_key(key: &str) -> bool {
    matches!(
        key,
        "spring.data.mongodb.uri"
            | "spring.data.mongodb.host"
            | "mongodb.uri"
            | "mongodb.url"
            | "mongodb.host"
            | "mongo.uri"
            | "mongo.url"
            | "mongo.host"
    ) || key.ends_with(".mongodb.uri")
        || key.ends_with(".mongodb.url")
        || key.ends_with(".mongodb.host")
}

fn normalize_database_url(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("jdbc:postgresql://") {
        return Some(format!(
            "database:postgresql://{}",
            trim_endpoint(&value["jdbc:postgresql://".len()..])
        ));
    }
    if lower.starts_with("jdbc:mysql://") {
        return Some(format!(
            "database:mysql://{}",
            trim_endpoint(&value["jdbc:mysql://".len()..])
        ));
    }
    if lower.starts_with("jdbc:mariadb://") {
        return Some(format!(
            "database:mariadb://{}",
            trim_endpoint(&value["jdbc:mariadb://".len()..])
        ));
    }
    if lower.starts_with("jdbc:sqlserver://") {
        return Some(format!(
            "database:sqlserver://{}",
            trim_endpoint(&value["jdbc:sqlserver://".len()..])
        ));
    }
    for (prefix, engine) in [
        ("postgresql://", "postgresql"),
        ("postgres://", "postgresql"),
        ("mysql://", "mysql"),
        ("mariadb://", "mariadb"),
        ("sqlserver://", "sqlserver"),
    ] {
        if lower.starts_with(prefix) {
            return Some(format!(
                "database:{engine}://{}",
                trim_endpoint(&value[prefix.len()..])
            ));
        }
    }
    None
}

fn normalize_scheme_url(value: &str, schemes: &[&str], resource_scheme: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    for scheme in schemes {
        if lower.starts_with(scheme) {
            return Some(format!(
                "{resource_scheme}://{}",
                trim_endpoint(&value[scheme.len()..])
            ));
        }
    }
    None
}

fn endpoint_list_url(resource_scheme: &str, value: &str) -> Option<String> {
    let endpoints: Vec<String> = value
        .split(',')
        .map(trim_endpoint)
        .filter(|part| is_host_like(part))
        .map(ToOwned::to_owned)
        .collect();
    if endpoints.is_empty() {
        None
    } else {
        Some(format!("{resource_scheme}://{}", endpoints.join(",")))
    }
}

fn trim_endpoint(value: &str) -> &str {
    value
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | ',' | ';'))
        .trim_end_matches('/')
        .split_once('?')
        .map(|(value, _)| value)
        .unwrap_or(value)
}

fn is_host_like(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.starts_with("${")
        && !value.contains(char::is_whitespace)
        && (value.contains('.') || value.contains(':') || value == "localhost")
}

pub(crate) fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
