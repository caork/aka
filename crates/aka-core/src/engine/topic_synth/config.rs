use std::collections::BTreeMap;

use super::ConfigTopicDetection;
use crate::engine::resource_synth::infra_config;

pub(super) fn extract_config_topics(text: &str) -> Vec<ConfigTopicDetection> {
    let pairs = infra_config::config_pairs(text);
    let groups = consumer_groups_by_prefix(&pairs);
    let mut out = Vec::new();
    for (key, value) in pairs {
        let Some(topic) = clean_topic_name(&value) else {
            continue;
        };
        let Some(broker) = broker_for_topic_key(&key) else {
            continue;
        };
        let prefix = topic_group_prefix(&key);
        out.push(ConfigTopicDetection {
            topic,
            broker,
            consumer_groups: prefix
                .and_then(|prefix| groups.get(prefix).cloned())
                .into_iter()
                .collect(),
        });
    }
    out.sort_by(|a, b| a.broker.cmp(&b.broker).then_with(|| a.topic.cmp(&b.topic)));
    out.dedup_by(|a, b| a.broker == b.broker && a.topic == b.topic);
    out
}

fn consumer_groups_by_prefix(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (key, value) in pairs {
        if !(key.ends_with(".group")
            || key.ends_with(".group.id")
            || key.ends_with(".consumer.group")
            || key.ends_with(".consumer.group.id"))
        {
            continue;
        }
        let Some(group) = clean_topic_name(value) else {
            continue;
        };
        if let Some(prefix) = topic_group_prefix(key) {
            out.insert(prefix.to_string(), group);
        }
    }
    out
}

fn broker_for_topic_key(key: &str) -> Option<String> {
    if key_contains_any(key, &["kafka"]) && topic_key_suffix(key) {
        Some("kafka".into())
    } else if key_contains_any(key, &["rabbit", "amqp"])
        && (topic_key_suffix(key) || queue_key_suffix(key))
    {
        Some("rabbitmq".into())
    } else if key_contains_any(key, &["sqs"]) && queue_key_suffix(key) {
        Some("sqs".into())
    } else if key_contains_any(key, &["nats"]) && topic_key_suffix(key) {
        Some("nats".into())
    } else if key_contains_any(key, &["celery"]) && queue_key_suffix(key) {
        Some("celery".into())
    } else {
        None
    }
}

fn topic_key_suffix(key: &str) -> bool {
    key.ends_with(".topic")
        || key.ends_with(".topics")
        || key.ends_with(".topic.name")
        || key.ends_with(".destination")
        || key.ends_with(".routing.key")
        || key.ends_with(".exchange")
        || key == "topic"
        || key == "topics"
}

fn queue_key_suffix(key: &str) -> bool {
    key.ends_with(".queue")
        || key.ends_with(".queues")
        || key.ends_with(".queue.name")
        || key.ends_with(".destination")
        || key == "queue"
        || key == "queues"
}

fn key_contains_any(key: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        key == *needle
            || key.starts_with(&format!("{needle}."))
            || key.ends_with(&format!(".{needle}"))
            || key.contains(&format!(".{needle}."))
    })
}

fn topic_group_prefix(key: &str) -> Option<&str> {
    for suffix in [
        ".topic.name",
        ".routing.key",
        ".queue.name",
        ".consumer.group.id",
        ".consumer.group",
        ".group.id",
        ".destination",
        ".topics",
        ".topic",
        ".queues",
        ".queue",
        ".exchange",
        ".group",
    ] {
        if let Some(prefix) = key.strip_suffix(suffix) {
            return Some(prefix.trim_end_matches('.'));
        }
    }
    None
}

fn clean_topic_name(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('/').to_string();
    if value.is_empty()
        || value.starts_with("${")
        || value.starts_with("http://")
        || value.starts_with("https://")
        || value.contains(char::is_whitespace)
    {
        return None;
    }
    Some(value)
}
