use super::super::{infra_config, ResourceDetection};

pub(super) fn extract_notification_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some((channel, strategy)) = notification_provider_for_config_key(&key) else {
            continue;
        };
        if !notification_config_value_is_present(&value) {
            continue;
        }
        out.push(ResourceDetection::notification(
            channel.into(),
            infra_config::config_id(&key),
            strategy,
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

fn notification_provider_for_config_key(key: &str) -> Option<(&'static str, &'static str)> {
    if key_contains_any(key, &["sendgrid"]) {
        Some(("email:sendgrid", "sendgrid-config"))
    } else if key_contains_any(key, &["mailgun"]) {
        Some(("email:mailgun", "mailgun-config"))
    } else if key_contains_any(key, &["ses"]) || key_contains_any(key, &["aws.ses"]) {
        Some(("email:aws-ses", "aws-ses-config"))
    } else if key_contains_any(key, &["smtp", "mail.smtp", "spring.mail"]) {
        Some(("email:smtp", "smtp-config"))
    } else if key_contains_any(key, &["slack"]) {
        Some(("chat:slack", "slack-config"))
    } else if key_contains_any(key, &["twilio"]) {
        Some(("sms:twilio", "twilio-config"))
    } else if key_contains_any(key, &["firebase", "fcm"]) {
        Some(("push:firebase", "firebase-config"))
    } else if key_contains_any(key, &["apns"]) {
        Some(("push:apns", "apns-config"))
    } else {
        None
    }
}

fn notification_config_value_is_present(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '`']);
    !value.is_empty()
        && !value.starts_with("${")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "none" | "null"
        )
}

fn key_contains_any(key: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        key.split('.')
            .any(|part| part == *needle || part.contains(needle))
            || key.contains(needle)
    })
}
