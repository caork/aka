use std::collections::HashSet;

use super::super::ResourceDetection;
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn has_python_notification_context(text: &str) -> bool {
    text.contains("sendgrid")
        || text.contains("SendGridAPIClient")
        || text.contains("mailgun")
        || text.contains("boto3")
        || text.contains("send_email")
        || text.contains("send_raw_email")
        || text.contains("slack_sdk")
        || text.contains("slack.com")
        || text.contains("twilio.rest")
        || text.contains("firebase_admin")
        || text.contains("messaging.send")
        || text.contains("apns2")
        || text.contains("APNsClient")
}

pub(super) fn extract_python_notifications(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if text.contains("sendgrid") || text.contains("SendGridAPIClient") {
        let clients = python_sendgrid_client_aliases(text);
        for call in find_call_args(text, ".send") {
            if !python_receiver_matches(text, call.start, &clients, &["sendgrid", "sg"]) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "email:sendgrid".into(),
                node.aka_id.clone(),
                "python-sendgrid-send",
            ));
        }
    }
    if text.contains("mailgun") {
        for call in find_call_args(text, ".post") {
            if !call.args.contains("mailgun") {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "email:mailgun".into(),
                node.aka_id.clone(),
                "python-mailgun-send",
            ));
        }
    }
    if text.contains("boto3") || text.contains("ses") {
        let clients = python_boto3_ses_client_aliases(text);
        for (callee, strategy) in [
            (".send_email", "python-aws-ses-send-email"),
            (".send_raw_email", "python-aws-ses-send-raw-email"),
            (
                ".send_templated_email",
                "python-aws-ses-send-templated-email",
            ),
        ] {
            for call in find_call_args(text, callee) {
                if !python_receiver_matches(text, call.start, &clients, &["ses"]) {
                    continue;
                }
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::notification(
                    "email:aws-ses".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    if text.contains("slack_sdk") || text.contains("slack.com") {
        let clients = python_slack_client_aliases(text);
        for call in find_call_args(text, ".chat_postMessage") {
            if !python_receiver_matches(text, call.start, &clients, &["slack", "client"]) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "chat:slack".into(),
                node.aka_id.clone(),
                "python-slack-chat-post-message",
            ));
        }
        for call in find_call_args(text, ".post") {
            if !call.args.contains("hooks.slack.com") && !call.args.contains("slack.com/api") {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "chat:slack".into(),
                node.aka_id.clone(),
                "python-slack-webhook-post",
            ));
        }
    }
    if text.contains("twilio.rest") {
        let clients = python_twilio_client_aliases(text);
        for call in find_call_args(text, ".messages.create") {
            if !python_receiver_matches(text, call.start, &clients, &["client", "twilio"]) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "sms:twilio".into(),
                node.aka_id.clone(),
                "python-twilio-message-create",
            ));
        }
    }
    if text.contains("firebase_admin") || text.contains("messaging.send") {
        for call in find_call_args(text, "messaging.send") {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "push:firebase".into(),
                node.aka_id.clone(),
                "python-firebase-messaging-send",
            ));
        }
        for call in find_call_args(text, "messaging.send_multicast") {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "push:firebase".into(),
                node.aka_id.clone(),
                "python-firebase-messaging-send-multicast",
            ));
        }
    }
    if text.contains("apns2") || text.contains("APNsClient") {
        let clients = python_apns_client_aliases(text);
        for call in find_call_args(text, ".send_notification") {
            if !python_receiver_matches(text, call.start, &clients, &["apns", "client"]) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "push:apns".into(),
                node.aka_id.clone(),
                "python-apns-send-notification",
            ));
        }
    }
    out
}

fn python_sendgrid_client_aliases(text: &str) -> HashSet<String> {
    let mut aliases = HashSet::new();
    for call in find_call_args(text, "SendGridAPIClient") {
        if let Some(lhs) = assignment_lhs(text, call.start) {
            aliases.insert(lhs);
        }
    }
    aliases
}

fn python_boto3_ses_client_aliases(text: &str) -> HashSet<String> {
    let mut aliases = HashSet::new();
    for call in find_call_args(text, ".client") {
        if !call.args.contains("ses") {
            continue;
        }
        if let Some(lhs) = assignment_lhs(text, call.start) {
            aliases.insert(lhs);
        }
    }
    for call in find_call_args(text, "boto3.client") {
        if !call.args.contains("ses") {
            continue;
        }
        if let Some(lhs) = assignment_lhs(text, call.start) {
            aliases.insert(lhs);
        }
    }
    aliases
}

fn python_slack_client_aliases(text: &str) -> HashSet<String> {
    let mut aliases = HashSet::new();
    for call in find_call_args(text, "WebClient") {
        if let Some(lhs) = assignment_lhs(text, call.start) {
            aliases.insert(lhs);
        }
    }
    aliases
}

fn python_twilio_client_aliases(text: &str) -> HashSet<String> {
    let mut aliases = HashSet::new();
    for call in find_call_args(text, "Client") {
        if let Some(lhs) = assignment_lhs(text, call.start) {
            aliases.insert(lhs);
        }
    }
    aliases
}

fn python_apns_client_aliases(text: &str) -> HashSet<String> {
    let mut aliases = HashSet::new();
    for call in find_call_args(text, "APNsClient") {
        if let Some(lhs) = assignment_lhs(text, call.start) {
            aliases.insert(lhs);
        }
    }
    aliases
}

fn assignment_lhs(text: &str, offset: usize) -> Option<String> {
    let line_start = text[..offset].rfind('\n').map_or(0, |idx| idx + 1);
    let prefix = &text[line_start..offset];
    let eq = prefix.rfind('=')?;
    if prefix[eq + 1..].trim().is_empty() {
        let lhs = prefix[..eq].trim();
        if is_identifier(lhs) {
            return Some(lhs.to_string());
        }
    }
    None
}

fn python_receiver_matches(
    text: &str,
    dot_start: usize,
    aliases: &HashSet<String>,
    fallback_names: &[&str],
) -> bool {
    let Some(receiver) = receiver_before_dot(text, dot_start) else {
        return false;
    };
    if aliases.contains(receiver) {
        return true;
    }
    let tail = receiver
        .rsplit('.')
        .next()
        .unwrap_or(receiver)
        .to_ascii_lowercase();
    fallback_names.iter().any(|name| tail == *name)
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

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
