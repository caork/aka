use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_python_notification_resources() {
    let repo = temp_repo("python-notification-resources");
    std::fs::write(
        repo.join("notifications.py"),
        r#"import boto3
import requests
from sendgrid import SendGridAPIClient

sg = SendGridAPIClient("token")
ses = boto3.client("ses")

def send_receipt(message):
    sg.send(message)
    ses.send_email(Source="orders@example.com", Destination={"ToAddresses": ["buyer@example.com"]}, Message={})

def send_support_ticket(payload):
    requests.post("https://api.mailgun.net/v3/mg.example.com/messages", data=payload)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "send_receipt",
        "notifications.send_receipt",
        "notifications.py",
        (8, 10),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "send_support_ticket",
        "notifications.send_support_ticket",
        "notifications.py",
        (12, 13),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (url, source_id, strategy) in [
        (
            "notification:email:sendgrid",
            "cbm:1:notifications.send_receipt",
            "python-sendgrid-send",
        ),
        (
            "notification:email:aws-ses",
            "cbm:1:notifications.send_receipt",
            "python-aws-ses-send-email",
        ),
        (
            "notification:email:mailgun",
            "cbm:2:notifications.send_support_ticket",
            "python-mailgun-send",
        ),
    ] {
        let resource = synth
            .resources
            .iter()
            .find(|resource| resource.url == url)
            .unwrap_or_else(|| panic!("expected notification resource {url}"));
        assert_eq!(resource.resource_type, "notification");
        assert!(resource.edge_recs().iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }));
    }
}

#[test]
fn synthesizes_java_notification_resources() {
    let repo = temp_repo("java-notification-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/notify")).unwrap();
    let file = "src/main/java/com/example/notify/Notifier.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.notify;

import org.springframework.mail.SimpleMailMessage;
import org.springframework.mail.javamail.JavaMailSender;
import software.amazon.awssdk.services.ses.SesClient;
import software.amazon.awssdk.services.ses.model.SendEmailRequest;

class Notifier {
    private final JavaMailSender mailSender;
    private final SesClient ses;

    void sendReceipt(SimpleMailMessage message) {
        mailSender.send(message);
    }

    void sendCampaign(SendEmailRequest request) {
        ses.sendEmail(request);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "sendReceipt",
        "com.example.notify.Notifier.sendReceipt",
        file,
        (12, 14),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "sendCampaign",
        "com.example.notify.Notifier.sendCampaign",
        file,
        (16, 18),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (url, source_id, strategy) in [
        (
            "notification:email:spring-mail",
            "cbm:1:com.example.notify.Notifier.sendReceipt",
            "java-spring-mail-send",
        ),
        (
            "notification:email:aws-ses",
            "cbm:2:com.example.notify.Notifier.sendCampaign",
            "java-aws-ses-send-email",
        ),
    ] {
        let resource = synth
            .resources
            .iter()
            .find(|resource| resource.url == url)
            .unwrap_or_else(|| panic!("expected notification resource {url}"));
        assert_eq!(resource.resource_type, "notification");
        assert!(resource.edge_recs().iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }));
    }
}

#[test]
fn synthesizes_python_chat_sms_and_push_notification_resources() {
    let repo = temp_repo("python-rich-notification-resources");
    std::fs::write(
        repo.join("alerts.py"),
        r##"import requests
from slack_sdk import WebClient
from twilio.rest import Client
from firebase_admin import messaging

slack = WebClient(token="xoxb-token")
twilio = Client("sid", "token")

def post_slack_alert(text):
    slack.chat_postMessage(channel="#ops", text=text)
    requests.post("https://hooks.slack.com/services/T000/B000/XXX", json={"text": text})

def send_sms(to, body):
    twilio.messages.create(to=to, from_="+15551234567", body=body)

def push_mobile(token):
    msg = messaging.Message(token=token)
    messaging.send(msg)

def ordinary_send(sender, payload):
    sender.send(payload)
"##,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "post_slack_alert",
        "alerts.post_slack_alert",
        "alerts.py",
        (9, 11),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "send_sms",
        "alerts.send_sms",
        "alerts.py",
        (13, 14),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "push_mobile",
        "alerts.push_mobile",
        "alerts.py",
        (16, 18),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "ordinary_send",
        "alerts.ordinary_send",
        "alerts.py",
        (20, 21),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (url, source_id, strategy) in [
        (
            "notification:chat:slack",
            "cbm:1:alerts.post_slack_alert",
            "python-slack-chat-post-message",
        ),
        (
            "notification:sms:twilio",
            "cbm:2:alerts.send_sms",
            "python-twilio-message-create",
        ),
        (
            "notification:push:firebase",
            "cbm:3:alerts.push_mobile",
            "python-firebase-messaging-send",
        ),
    ] {
        assert_notification_edge(&synth, url, source_id, strategy);
    }
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:4:alerts.ordinary_send"));
}

#[test]
fn synthesizes_java_chat_sms_and_push_notification_resources() {
    let repo = temp_repo("java-rich-notification-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/notify")).unwrap();
    let file = "src/main/java/com/example/notify/AlertService.java";
    std::fs::write(
        repo.join(file),
        r##"package com.example.notify;

import com.google.firebase.messaging.FirebaseMessaging;
import com.google.firebase.messaging.Message;
import com.slack.api.Slack;
import com.twilio.rest.api.v2010.account.MessageCreator;
import com.twilio.rest.api.v2010.account.Message;
import org.springframework.web.client.RestTemplate;

class AlertService {
    void postSlack(String text) throws Exception {
        Slack.getInstance().methods("token").chatPostMessage(req -> req.channel("#ops").text(text));
    }

    void postSlackWebhook(RestTemplate restTemplate, Object payload) {
        restTemplate.postForObject("https://hooks.slack.com/services/T000/B000/XXX", payload, String.class);
    }

    void sendSms(String to) {
        Message.creator(new com.twilio.type.PhoneNumber(to), new com.twilio.type.PhoneNumber("+15551234567"), "hello").create();
    }

    void pushMobile(Message message) throws Exception {
        FirebaseMessaging.getInstance().send(message);
    }

    void ordinarySend(Sender sender, Object payload) {
        sender.send(payload);
    }
}"##,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "postSlack",
        "com.example.notify.AlertService.postSlack",
        file,
        (11, 13),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "postSlackWebhook",
        "com.example.notify.AlertService.postSlackWebhook",
        file,
        (15, 17),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "sendSms",
        "com.example.notify.AlertService.sendSms",
        file,
        (19, 21),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "pushMobile",
        "com.example.notify.AlertService.pushMobile",
        file,
        (23, 25),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "ordinarySend",
        "com.example.notify.AlertService.ordinarySend",
        file,
        (27, 29),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (url, source_id, strategy) in [
        (
            "notification:chat:slack",
            "cbm:1:com.example.notify.AlertService.postSlack",
            "java-slack-chat-post-message",
        ),
        (
            "notification:chat:slack",
            "cbm:2:com.example.notify.AlertService.postSlackWebhook",
            "java-slack-webhook-post",
        ),
        (
            "notification:sms:twilio",
            "cbm:3:com.example.notify.AlertService.sendSms",
            "java-twilio-message-create",
        ),
        (
            "notification:push:firebase",
            "cbm:4:com.example.notify.AlertService.pushMobile",
            "java-firebase-messaging-send",
        ),
    ] {
        assert_notification_edge(&synth, url, source_id, strategy);
    }
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:5:com.example.notify.AlertService.ordinarySend"));
}

fn assert_notification_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected notification resource {url}"));
    assert_eq!(resource.resource_type, "notification");
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == source_id
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == strategy
    }));
}
