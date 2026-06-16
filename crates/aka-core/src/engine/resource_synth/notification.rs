use std::collections::HashSet;

use super::ResourceDetection;
use crate::engine::{find_call_args, node_at_offset, SynthNode};

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
    out
}

fn has_java_notification_context(text: &str) -> bool {
    text.contains("JavaMailSender")
        || text.contains("MimeMessage")
        || text.contains("SimpleMailMessage")
        || text.contains("software.amazon.awssdk.services.ses")
        || text.contains("SesClient")
        || text.contains("SendEmailRequest")
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
