use super::super::ResourceDetection;
use crate::engine::{find_call_args, find_matching_paren, node_at_offset, skip_ws, SynthNode};

pub(super) fn has_java_notification_context(text: &str) -> bool {
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

pub(super) fn extract_java_notifications(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
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
