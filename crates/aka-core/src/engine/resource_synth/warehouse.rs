use super::{infra_config, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn extract_warehouse_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_warehouse_context(text) {
        out.extend(extract_python_warehouses(text, nodes));
    }
    if has_java_warehouse_context(text) {
        out.extend(extract_java_warehouses(text, nodes));
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

pub(super) fn extract_warehouse_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some(provider) = warehouse_provider_for_config(&key, &value) else {
            continue;
        };
        if !warehouse_config_value_is_present(&value) {
            continue;
        }
        out.push(ResourceDetection::warehouse(
            provider.into(),
            infra_config::config_id(&key),
            warehouse_config_strategy(provider),
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

fn has_python_warehouse_context(text: &str) -> bool {
    text.contains("snowflake.connector")
        || text.contains("google.cloud.bigquery")
        || text.contains("BigQuery")
        || text.contains("redshift_connector")
        || text.contains("databricks.sql")
        || text.contains("sql.connect")
}

fn extract_python_warehouses(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "snowflake",
        &["snowflake.connector.connect("],
        &[(".execute", "python-snowflake-execute")],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "bigquery",
        &["bigquery.Client("],
        &[
            (".query", "python-bigquery-query"),
            (".load_table_from_uri", "python-bigquery-load-table"),
            (".insert_rows_json", "python-bigquery-insert-rows"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "redshift",
        &["redshift_connector.connect("],
        &[(".execute", "python-redshift-execute")],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "databricks",
        &["sql.connect(", "databricks.sql.connect("],
        &[(".execute", "python-databricks-sql-execute")],
    ));
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
            out.push(ResourceDetection::warehouse(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn has_java_warehouse_context(text: &str) -> bool {
    text.contains("BigQuery")
        || text.contains("com.google.cloud.bigquery")
        || text.contains("Snowflake")
        || text.contains("net.snowflake.client.jdbc")
        || text.contains("Redshift")
        || text.contains("com.amazon.redshift")
        || text.contains("Databricks")
        || text.contains("databricks")
}

fn extract_java_warehouses(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "bigquery",
        &["BigQuery"],
        &[
            (".query", "java-bigquery-query"),
            (".insertAll", "java-bigquery-insert-all"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "snowflake",
        &["SnowflakeConnection", "SnowflakeStatement"],
        &[
            (".execute", "java-snowflake-execute"),
            (".executeQuery", "java-snowflake-execute-query"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "redshift",
        &["RedshiftConnection", "RedshiftStatement"],
        &[
            (".execute", "java-redshift-execute"),
            (".executeQuery", "java-redshift-execute-query"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "databricks",
        &["DatabricksConnection", "DatabricksStatement"],
        &[
            (".execute", "java-databricks-execute"),
            (".executeQuery", "java-databricks-execute-query"),
        ],
    ));
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
            out.push(ResourceDetection::warehouse(
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
    receiver.to_ascii_lowercase().contains(provider)
        || python_receiver_assigned_to(text, receiver, constructors)
        || python_receiver_from_connection_cursor(text, receiver, provider)
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

fn python_receiver_from_connection_cursor(text: &str, receiver: &str, provider: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            return false;
        };
        let lhs = lhs
            .trim()
            .trim_start_matches("self.")
            .trim_start_matches("cls.");
        lhs == receiver
            && rhs.contains(".cursor(")
            && text.lines().any(|other| {
                let Some((conn_lhs, conn_rhs)) = other.trim().split_once('=') else {
                    return false;
                };
                rhs.contains(conn_lhs.trim())
                    && match provider {
                        "snowflake" => conn_rhs.contains("snowflake.connector.connect("),
                        "redshift" => conn_rhs.contains("redshift_connector.connect("),
                        "databricks" => {
                            conn_rhs.contains("sql.connect(")
                                || conn_rhs.contains("databricks.sql.connect(")
                        }
                        _ => false,
                    }
            })
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

fn warehouse_provider_for_config(key: &str, value: &str) -> Option<&'static str> {
    let value = value.to_ascii_lowercase();
    if key_contains_any(key, &["snowflake"]) || value.contains("snowflakecomputing.com") {
        Some("snowflake")
    } else if key_contains_any(key, &["bigquery", "big.query"]) {
        Some("bigquery")
    } else if key_contains_any(key, &["redshift"]) || value.contains("redshift.amazonaws.com") {
        Some("redshift")
    } else if key_contains_any(key, &["databricks"]) || value.contains("databricks.com") {
        Some("databricks")
    } else {
        None
    }
}

fn warehouse_config_value_is_present(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '`']);
    !value.is_empty()
        && !value.starts_with("${")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "none" | "null" | "0"
        )
}

fn warehouse_config_strategy(provider: &str) -> &'static str {
    match provider {
        "snowflake" => "snowflake-config",
        "bigquery" => "bigquery-config",
        "redshift" => "redshift-config",
        "databricks" => "databricks-config",
        _ => "warehouse-config",
    }
}

fn key_contains_any(key: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        if needle.contains('.') && key.contains(needle) {
            return true;
        }
        key.split('.')
            .any(|part| part == *needle || part.contains(needle))
    })
}
