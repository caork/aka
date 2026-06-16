use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_payment_resources() {
    let repo = temp_repo("configured-payment-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"payments:
  stripe:
    api-key: sk_live_redacted
  paypal:
    client-id: paypal-client
  square:
    access-token: sq-token
disabled:
  adyen:
    api-key: ${ADYEN_API_KEY}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"ADYEN_MERCHANT_ACCOUNT = "merchant"
BRAINTREE_MERCHANT_ID = "bt-merchant"
RAZORPAY_KEY_ID = "rzp"
"#,
    )
    .unwrap();
    std::fs::write(repo.join(".env"), "PAYPAL_SECRET=secret\n").unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 10),
        json!({"language": "yaml"}),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Config", "settings.py", "settings.py", "settings.py"),
        (1, 3),
        json!({"language": "python"}),
    );
    insert_node_props_at(
        &conn,
        3,
        ("Config", ".env", ".env", ".env"),
        (1, 1),
        json!({"language": "dotenv"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_payment_config_edge(
        &synth,
        "payment:stripe",
        &config_id("payments.stripe.api.key"),
        "stripe-config",
    );
    assert_payment_config_edge(
        &synth,
        "payment:paypal",
        &config_id("payments.paypal.client.id"),
        "paypal-config",
    );
    assert_payment_config_edge(
        &synth,
        "payment:square",
        &config_id("payments.square.access.token"),
        "square-config",
    );
    assert_payment_config_edge(
        &synth,
        "payment:adyen",
        &config_id("adyen.merchant.account"),
        "adyen-config",
    );
    assert_payment_config_edge(
        &synth,
        "payment:braintree",
        &config_id("braintree.merchant.id"),
        "braintree-config",
    );
    assert_payment_config_edge(
        &synth,
        "payment:razorpay",
        &config_id("razorpay.key.id"),
        "razorpay-config",
    );
    assert_payment_config_edge(
        &synth,
        "payment:paypal",
        &config_id("paypal.secret"),
        "paypal-config",
    );
    assert!(!synth.resources.iter().any(|resource| {
        resource.url == "payment:adyen"
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("disabled.adyen.api.key"))
    }));
}

#[test]
fn synthesizes_python_payment_resources() {
    let repo = temp_repo("python-payment-resources");
    std::fs::write(
        repo.join("billing.py"),
        r#"import stripe
from paypalcheckoutsdk.orders import OrdersCreateRequest
from paypalhttp import HttpError

def start_checkout(cart):
    session = stripe.checkout.Session.create(mode="payment", line_items=cart.items)
    return session

def create_intent(order):
    return stripe.PaymentIntent.create(amount=order.total_cents, currency="usd")

def create_paypal_order(client, payload):
    request = OrdersCreateRequest()
    request.request_body(payload)
    return client.execute(request)

def ordinary_create(service):
    return service.create()
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "start_checkout",
        "billing.start_checkout",
        "billing.py",
        (6, 8),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "create_intent",
        "billing.create_intent",
        "billing.py",
        (10, 11),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "create_paypal_order",
        "billing.create_paypal_order",
        "billing.py",
        (13, 16),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "ordinary_create",
        "billing.ordinary_create",
        "billing.py",
        (18, 19),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let stripe = synth
        .resources
        .iter()
        .find(|resource| resource.url == "payment:stripe")
        .expect("expected Stripe payment resource");
    assert_eq!(stripe.resource_type, "payment");
    assert!(stripe.edge_recs().iter().any(|edge| {
        edge.source_id == "cbm:1:billing.start_checkout"
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == "python-stripe-checkout-session"
    }));
    assert!(stripe.edge_recs().iter().any(|edge| {
        edge.source_id == "cbm:2:billing.create_intent"
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == "python-stripe-payment-intent"
    }));
    let paypal = synth
        .resources
        .iter()
        .find(|resource| resource.url == "payment:paypal")
        .expect("expected PayPal payment resource");
    assert_eq!(paypal.resource_type, "payment");
    assert!(paypal.edge_recs().iter().any(|edge| {
        edge.source_id == "cbm:3:billing.create_paypal_order"
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == "python-paypal-orders-execute"
    }));
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:4:billing.ordinary_create"));
}

fn assert_payment_config_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected payment config resource {url}"));
    assert_eq!(resource.resource_type, "payment");
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == source_id
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == strategy
    }));
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}

#[test]
fn synthesizes_java_payment_resources() {
    let repo = temp_repo("java-payment-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/billing")).unwrap();
    let file = "src/main/java/com/example/billing/BillingService.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.billing;

import com.paypal.http.HttpResponse;
import com.paypal.orders.Order;
import com.paypal.orders.OrdersCreateRequest;
import com.paypal.core.PayPalHttpClient;
import com.stripe.model.PaymentIntent;
import com.stripe.param.PaymentIntentCreateParams;

class BillingService {
    PaymentIntent createIntent(long amount) throws Exception {
        PaymentIntentCreateParams params = PaymentIntentCreateParams.builder()
            .setAmount(amount)
            .setCurrency("usd")
            .build();
        return PaymentIntent.create(params);
    }

    HttpResponse<Order> createPayPal(PayPalHttpClient client, OrdersCreateRequest request) throws Exception {
        return client.execute(request);
    }

    Object ordinaryCreate(Widget widget) {
        return widget.create();
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "createIntent",
        "com.example.billing.BillingService.createIntent",
        file,
        (12, 18),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "createPayPal",
        "com.example.billing.BillingService.createPayPal",
        file,
        (20, 22),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "ordinaryCreate",
        "com.example.billing.BillingService.ordinaryCreate",
        file,
        (24, 26),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let stripe = synth
        .resources
        .iter()
        .find(|resource| resource.url == "payment:stripe")
        .expect("expected Stripe payment resource");
    assert_eq!(stripe.resource_type, "payment");
    assert!(stripe.edge_recs().iter().any(|edge| {
        edge.source_id == "cbm:1:com.example.billing.BillingService.createIntent"
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == "java-stripe-create"
    }));
    let paypal = synth
        .resources
        .iter()
        .find(|resource| resource.url == "payment:paypal")
        .expect("expected PayPal payment resource");
    assert_eq!(paypal.resource_type, "payment");
    assert!(paypal.edge_recs().iter().any(|edge| {
        edge.source_id == "cbm:2:com.example.billing.BillingService.createPayPal"
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == "java-paypal-orders-execute"
    }));
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:3:com.example.billing.BillingService.ordinaryCreate"));
}
