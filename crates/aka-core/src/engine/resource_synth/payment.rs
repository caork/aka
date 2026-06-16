use super::ResourceDetection;
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn extract_payment_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_payment_context(text) {
        out.extend(extract_python_payments(text, nodes));
    }
    if has_java_payment_context(text) {
        out.extend(extract_java_payments(text, nodes));
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then_with(|| a.node_id.cmp(&b.node_id)));
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn has_python_payment_context(text: &str) -> bool {
    text.contains("stripe")
        || text.contains("paypalcheckoutsdk")
        || text.contains("paypalhttp")
        || text.contains("PayPalHttpClient")
}

fn extract_python_payments(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if text.contains("stripe") {
        for (callee, strategy) in [
            (
                "stripe.checkout.Session.create",
                "python-stripe-checkout-session",
            ),
            (
                "stripe.PaymentIntent.create",
                "python-stripe-payment-intent",
            ),
            ("stripe.Charge.create", "python-stripe-charge"),
            ("stripe.Refund.create", "python-stripe-refund"),
            ("stripe.Subscription.create", "python-stripe-subscription"),
        ] {
            for call in find_call_args(text, callee) {
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::payment(
                    "stripe".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    if text.contains("paypalcheckoutsdk") || text.contains("PayPalHttpClient") {
        out.extend(extract_python_paypal_execute_calls(text, nodes));
    }
    out
}

fn extract_python_paypal_execute_calls(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, ".execute") {
        let window_start = call.start.saturating_sub(500);
        let window_end = (call.start + call.args.len() + 500).min(text.len());
        let window = &text[window_start..window_end];
        if !window.contains("OrdersCreateRequest")
            && !window.contains("OrdersCaptureRequest")
            && !window.contains("OrdersAuthorizeRequest")
        {
            continue;
        }
        let Some(node) = node_at_offset(text, nodes, call.start) else {
            continue;
        };
        out.push(ResourceDetection::payment(
            "paypal".into(),
            node.aka_id.clone(),
            "python-paypal-orders-execute",
        ));
    }
    out
}

fn has_java_payment_context(text: &str) -> bool {
    text.contains("com.stripe")
        || text.contains("ChargeCreateParams")
        || text.contains("PaymentIntentCreateParams")
        || text.contains("RefundCreateParams")
        || text.contains("SessionCreateParams")
        || text.contains("com.paypal")
        || text.contains("PayPalHttpClient")
        || text.contains("OrdersCreateRequest")
}

fn extract_java_payments(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if text.contains("com.stripe")
        || text.contains("PaymentIntentCreateParams")
        || text.contains("SessionCreateParams")
    {
        for (callee, strategy) in [
            (".create", "java-stripe-create"),
            (".confirm", "java-stripe-confirm"),
            (".capture", "java-stripe-capture"),
            (".refund", "java-stripe-refund"),
        ] {
            for call in find_call_args(text, callee) {
                if !java_stripe_call_site(text, call.start, call.args) {
                    continue;
                }
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::payment(
                    "stripe".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    if text.contains("com.paypal") || text.contains("PayPalHttpClient") {
        for call in find_call_args(text, ".execute") {
            if !java_paypal_call_window(text, call.start, call.args.len()) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::payment(
                "paypal".into(),
                node.aka_id.clone(),
                "java-paypal-orders-execute",
            ));
        }
    }
    out
}

fn java_stripe_call_site(text: &str, start: usize, args: &str) -> bool {
    let Some(receiver) = receiver_before_dot(text, start) else {
        return false;
    };
    let tail = receiver.rsplit('.').next().unwrap_or(receiver);
    matches!(
        tail,
        "PaymentIntent" | "Session" | "Charge" | "Refund" | "Subscription"
    ) || receiver.ends_with("Checkout.Session")
        || args.contains("PaymentIntentCreateParams")
        || args.contains("SessionCreateParams")
        || args.contains("ChargeCreateParams")
        || args.contains("RefundCreateParams")
        || args.contains("SubscriptionCreateParams")
}

fn java_paypal_call_window(text: &str, start: usize, args_len: usize) -> bool {
    let window_start = start.saturating_sub(500);
    let window_end = (start + args_len + 500).min(text.len());
    let window = &text[window_start..window_end];
    window.contains("OrdersCreateRequest")
        || window.contains("OrdersCaptureRequest")
        || window.contains("OrdersAuthorizeRequest")
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
