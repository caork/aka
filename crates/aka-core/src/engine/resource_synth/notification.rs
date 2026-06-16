use std::collections::HashSet;

use super::ResourceDetection;
use crate::engine::{find_call_args, find_matching_paren, node_at_offset, skip_ws, SynthNode};

pub(super) fn extract_notification_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_notification_context(text) {
        out.extend(extract_python_notifications(text, nodes));
    }
    if has_java_notification_context(text) {
        out.extend(extract_java_notifications(text, nodes));
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then_with(|| a.node_id.cmp(&b.node_id)));
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn has_python_notification_context(text: &str) -> bool {
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

fn extract_python_notifications(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
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

fn has_java_notification_context(text: &str) -> bool {
    text.contains("JavaMailSender")
        || text.contains("MimeMessage")
        || text.contains("SimpleMailMessage")
        || text.contains("software.amazon.awssdk.services.ses")
        || text.contains("SesClient")
        || text.contains("SendEmailRequest")
        || text.contains("com.slack.api")
        || text.contains("Slack.getInstance")
        || text.contains("hooks.slack.com")
        || text.contains("com.twilio")
        || text.contains("Message.creator")
        || text.contains("FirebaseMessaging")
        || text.contains("ApnsClient")
}

fn extract_java_notifications(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if text.contains("JavaMailSender") || text.contains("SimpleMailMessage") {
        for call in find_call_args(text, ".send") {
            if !java_mail_sender_receiver(text, call.start) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "email:spring-mail".into(),
                node.aka_id.clone(),
                "java-spring-mail-send",
            ));
        }
    }
    if text.contains("SesClient") || text.contains("SendEmailRequest") {
        for call in find_call_args(text, ".sendEmail") {
            if !java_ses_receiver(text, call.start) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "email:aws-ses".into(),
                node.aka_id.clone(),
                "java-aws-ses-send-email",
            ));
        }
    }
    if text.contains("com.slack.api") || text.contains("Slack.getInstance") {
        for (start, args_len) in find_dotted_call_offsets(text, ".chatPostMessage") {
            if !java_slack_call_site(text, start, args_len) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "chat:slack".into(),
                node.aka_id.clone(),
                "java-slack-chat-post-message",
            ));
        }
    }
    if text.contains("hooks.slack.com") {
        for call in find_call_args(text, ".postForObject") {
            if !call.args.contains("hooks.slack.com") {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "chat:slack".into(),
                node.aka_id.clone(),
                "java-slack-webhook-post",
            ));
        }
    }
    if text.contains("com.twilio") || text.contains("Message.creator") {
        for call in find_call_args(text, "Message.creator") {
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "sms:twilio".into(),
                node.aka_id.clone(),
                "java-twilio-message-create",
            ));
        }
    }
    if text.contains("FirebaseMessaging") {
        for (start, args_len) in find_dotted_call_offsets(text, ".send") {
            if !java_firebase_call_site(text, start, args_len) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, start) else {
                continue;
            };
            out.push(ResourceDetection::notification(
                "push:firebase".into(),
                node.aka_id.clone(),
                "java-firebase-messaging-send",
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

fn java_mail_sender_receiver(text: &str, dot_start: usize) -> bool {
    let Some(receiver) = receiver_before_dot(text, dot_start) else {
        return false;
    };
    let tail = receiver
        .rsplit('.')
        .next()
        .unwrap_or(receiver)
        .to_ascii_lowercase();
    tail.contains("mail") || tail.contains("sender")
}

fn java_ses_receiver(text: &str, dot_start: usize) -> bool {
    let Some(receiver) = receiver_before_dot(text, dot_start) else {
        return false;
    };
    let tail = receiver
        .rsplit('.')
        .next()
        .unwrap_or(receiver)
        .to_ascii_lowercase();
    tail == "ses" || tail.contains("ses")
}

fn java_slack_call_site(text: &str, start: usize, args_len: usize) -> bool {
    if receiver_tail_matches(text, start, &["slack", "methods", "client"]) {
        return true;
    }
    same_statement_prefix(text, start, args_len).contains("Slack.getInstance")
}

fn java_firebase_call_site(text: &str, start: usize, args_len: usize) -> bool {
    if receiver_tail_matches(text, start, &["firebase", "messaging"]) {
        return true;
    }
    same_statement_prefix(text, start, args_len).contains("FirebaseMessaging.getInstance")
}

fn receiver_tail_matches(text: &str, dot_start: usize, candidates: &[&str]) -> bool {
    let Some(receiver) = receiver_before_dot(text, dot_start) else {
        return false;
    };
    let tail = receiver
        .rsplit('.')
        .next()
        .unwrap_or(receiver)
        .to_ascii_lowercase();
    candidates
        .iter()
        .any(|candidate| tail == *candidate || tail.contains(candidate))
}

fn find_dotted_call_offsets(text: &str, callee: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = text[offset..].find(callee) {
        let start = offset + rel;
        let open = skip_ws(text, start + callee.len());
        if text.as_bytes().get(open) == Some(&b'(') {
            if let Some(close) = find_matching_paren(text, open) {
                out.push((start, close.saturating_sub(open + 1)));
                offset = close + 1;
                continue;
            }
        }
        offset = start + callee.len();
    }
    out
}

fn same_statement_prefix(text: &str, start: usize, args_len: usize) -> &str {
    let line_start = text[..start]
        .rfind(['\n', ';', '{', '}'])
        .map_or(0, |idx| idx + 1);
    let line_end = (start + args_len).min(text.len());
    &text[line_start..line_end]
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
