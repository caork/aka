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
