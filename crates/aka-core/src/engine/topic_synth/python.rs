use super::{
    extract_call_topic_literals, extract_keyword_topic_literals, topic_detection, TopicDetection,
    TopicEndpointKind,
};
use crate::engine::SynthNode;

pub(super) fn extract_python_topic_detections(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    let ctx = PythonMessagingContext::from_text(text);
    for ctor in &ctx.kafka_consumer_ctors {
        out.extend(extract_call_topic_literals(
            text,
            nodes,
            ctor,
            "kafka",
            TopicEndpointKind::Consumer,
            "python-kafka-consumer",
            0,
        ));
    }
    for receiver in &ctx.kafka_consumer_receivers {
        out.extend(extract_call_topic_literals(
            text,
            nodes,
            &format!("{receiver}.subscribe"),
            "kafka",
            TopicEndpointKind::Consumer,
            "python-kafka-consumer-subscribe",
            0,
        ));
    }
    for receiver in &ctx.kafka_producer_receivers {
        out.extend(extract_call_topic_literals(
            text,
            nodes,
            &format!("{receiver}.send"),
            "kafka",
            TopicEndpointKind::Producer,
            "python-kafka-producer-send",
            0,
        ));
        out.extend(extract_call_topic_literals(
            text,
            nodes,
            &format!("{receiver}.produce"),
            "kafka",
            TopicEndpointKind::Producer,
            "python-kafka-producer-produce",
            0,
        ));
    }
    for receiver in &ctx.rabbit_channel_receivers {
        out.extend(extract_keyword_topic_literals(
            text,
            nodes,
            &format!("{receiver}.basic_consume"),
            "queue",
            "rabbitmq",
            TopicEndpointKind::Consumer,
            "python-rabbit-basic-consume",
        ));
        out.extend(extract_keyword_topic_literals(
            text,
            nodes,
            &format!("{receiver}.basic_publish"),
            "exchange",
            "rabbitmq",
            TopicEndpointKind::Producer,
            "python-rabbit-basic-publish",
        ));
    }
    for receiver in &ctx.nats_client_receivers {
        out.extend(extract_call_topic_literals(
            text,
            nodes,
            &format!("{receiver}.subscribe"),
            "nats",
            TopicEndpointKind::Consumer,
            "python-nats-subscribe",
            0,
        ));
        out.extend(extract_call_topic_literals(
            text,
            nodes,
            &format!("{receiver}.publish"),
            "nats",
            TopicEndpointKind::Producer,
            "python-nats-publish",
            0,
        ));
    }
    for receiver in &ctx.sqs_client_receivers {
        out.extend(extract_boto3_sqs_client_call_topics(
            text,
            nodes,
            &format!("{receiver}.send_message"),
            TopicEndpointKind::Producer,
            "python-boto3-sqs-send-message",
        ));
        out.extend(extract_boto3_sqs_client_call_topics(
            text,
            nodes,
            &format!("{receiver}.receive_message"),
            TopicEndpointKind::Consumer,
            "python-boto3-sqs-receive-message",
        ));
    }
    for queue in &ctx.sqs_queue_receivers {
        out.extend(extract_bound_sqs_queue_call_topics(
            text,
            nodes,
            &queue.receiver,
            &queue.queue,
            "send_message",
            TopicEndpointKind::Producer,
            "python-boto3-sqs-send-message",
        ));
        out.extend(extract_bound_sqs_queue_call_topics(
            text,
            nodes,
            &queue.receiver,
            &queue.queue,
            "receive_messages",
            TopicEndpointKind::Consumer,
            "python-boto3-sqs-receive-messages",
        ));
    }
    out
}

#[derive(Debug, Clone, Default)]
struct PythonMessagingContext {
    kafka_consumer_ctors: Vec<String>,
    kafka_consumer_receivers: Vec<String>,
    kafka_producer_receivers: Vec<String>,
    rabbit_channel_receivers: Vec<String>,
    nats_client_receivers: Vec<String>,
    sqs_client_receivers: Vec<String>,
    sqs_resource_receivers: Vec<String>,
    sqs_queue_receivers: Vec<PythonSqsQueueReceiver>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct PythonSqsQueueReceiver {
    receiver: String,
    queue: String,
}

impl PythonMessagingContext {
    fn from_text(text: &str) -> Self {
        let imports = PythonMessagingImports::from_text(text);
        let mut ctx = Self {
            kafka_consumer_ctors: imports.kafka_consumer_ctors.clone(),
            ..Default::default()
        };

        for line in text.lines() {
            let Some((lhs, rhs)) = python_assignment(line) else {
                continue;
            };
            if imports
                .kafka_producer_ctors
                .iter()
                .any(|ctor| rhs_starts_with_call(rhs, ctor))
            {
                ctx.kafka_producer_receivers.push(lhs.to_string());
            }
            if imports
                .kafka_consumer_ctors
                .iter()
                .any(|ctor| rhs_starts_with_call(rhs, ctor))
            {
                ctx.kafka_consumer_receivers.push(lhs.to_string());
            }
            if imports.has_pika && rhs.contains(".channel(") {
                ctx.rabbit_channel_receivers.push(lhs.to_string());
            }
            if imports.has_nats
                && (rhs.contains("nats.connect(")
                    || imports
                        .nats_client_ctors
                        .iter()
                        .any(|ctor| rhs_starts_with_call(rhs, ctor)))
            {
                ctx.nats_client_receivers.push(lhs.to_string());
            }
            if !imports.boto3_aliases.is_empty() {
                if boto3_sqs_constructor(rhs, &imports.boto3_aliases, "client") {
                    ctx.sqs_client_receivers.push(lhs.to_string());
                }
                if boto3_sqs_constructor(rhs, &imports.boto3_aliases, "resource") {
                    ctx.sqs_resource_receivers.push(lhs.to_string());
                }
                for receiver in &ctx.sqs_resource_receivers {
                    if let Some(queue) = sqs_resource_queue_name(rhs, receiver) {
                        ctx.sqs_queue_receivers.push(PythonSqsQueueReceiver {
                            receiver: lhs.to_string(),
                            queue,
                        });
                    }
                }
            }
        }

        ctx.kafka_consumer_ctors.sort();
        ctx.kafka_consumer_ctors.dedup();
        ctx.kafka_consumer_receivers.sort();
        ctx.kafka_consumer_receivers.dedup();
        ctx.kafka_producer_receivers.sort();
        ctx.kafka_producer_receivers.dedup();
        ctx.rabbit_channel_receivers.sort();
        ctx.rabbit_channel_receivers.dedup();
        ctx.nats_client_receivers.sort();
        ctx.nats_client_receivers.dedup();
        ctx.sqs_client_receivers.sort();
        ctx.sqs_client_receivers.dedup();
        ctx.sqs_resource_receivers.sort();
        ctx.sqs_resource_receivers.dedup();
        ctx.sqs_queue_receivers.sort();
        ctx.sqs_queue_receivers.dedup();
        ctx
    }
}

#[derive(Debug, Clone, Default)]
struct PythonMessagingImports {
    kafka_consumer_ctors: Vec<String>,
    kafka_producer_ctors: Vec<String>,
    nats_client_ctors: Vec<String>,
    has_pika: bool,
    has_nats: bool,
    boto3_aliases: Vec<String>,
}

impl PythonMessagingImports {
    fn from_text(text: &str) -> Self {
        let mut imports = Self::default();
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if let Some(rest) = line.strip_prefix("from kafka import ") {
                for (name, alias) in python_import_items(rest) {
                    match name.as_str() {
                        "KafkaConsumer" => imports.kafka_consumer_ctors.push(alias),
                        "KafkaProducer" => imports.kafka_producer_ctors.push(alias),
                        _ => {}
                    }
                }
            } else if let Some(rest) = line.strip_prefix("from confluent_kafka import ") {
                for (name, alias) in python_import_items(rest) {
                    match name.as_str() {
                        "Consumer" => imports.kafka_consumer_ctors.push(alias),
                        "Producer" => imports.kafka_producer_ctors.push(alias),
                        _ => {}
                    }
                }
            } else if let Some(rest) = line.strip_prefix("from nats.aio.client import ") {
                imports.has_nats = true;
                for (name, alias) in python_import_items(rest) {
                    if name == "Client" {
                        imports.nats_client_ctors.push(alias);
                    }
                }
            } else if let Some(rest) = line.strip_prefix("import ") {
                for (name, alias) in python_import_items(rest) {
                    match name.as_str() {
                        "kafka" => {
                            imports
                                .kafka_consumer_ctors
                                .push(format!("{alias}.KafkaConsumer"));
                            imports
                                .kafka_producer_ctors
                                .push(format!("{alias}.KafkaProducer"));
                        }
                        "confluent_kafka" => {
                            imports
                                .kafka_consumer_ctors
                                .push(format!("{alias}.Consumer"));
                            imports
                                .kafka_producer_ctors
                                .push(format!("{alias}.Producer"));
                        }
                        "pika" => imports.has_pika = true,
                        "nats" => {
                            imports.has_nats = true;
                            imports.nats_client_ctors.push(format!("{alias}.NATS"));
                        }
                        "boto3" => imports.boto3_aliases.push(alias),
                        _ => {}
                    }
                }
            }
        }
        imports.kafka_consumer_ctors.sort();
        imports.kafka_consumer_ctors.dedup();
        imports.kafka_producer_ctors.sort();
        imports.kafka_producer_ctors.dedup();
        imports.nats_client_ctors.sort();
        imports.nats_client_ctors.dedup();
        imports.boto3_aliases.sort();
        imports.boto3_aliases.dedup();
        imports
    }
}

fn python_import_items(rest: &str) -> Vec<(String, String)> {
    rest.split('#')
        .next()
        .unwrap_or(rest)
        .split(',')
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            let (name, alias) = item
                .split_once(" as ")
                .map(|(name, alias)| (name.trim(), alias.trim()))
                .unwrap_or((item, item));
            is_python_ref(name)
                .then(|| (name.to_string(), alias.to_string()))
                .filter(|(_, alias)| is_python_ref(alias))
        })
        .collect()
}

fn python_assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.starts_with('#') || line.contains("==") || line.contains("!=") {
        return None;
    }
    let (lhs, rhs) = line.split_once('=')?;
    let lhs = lhs.trim();
    let rhs = rhs.trim();
    (is_python_ref(lhs) && !rhs.is_empty()).then_some((lhs, rhs))
}

fn rhs_starts_with_call(rhs: &str, callee: &str) -> bool {
    rhs.strip_prefix(callee)
        .is_some_and(|rest| rest.trim_start().starts_with('('))
}

fn boto3_sqs_constructor(rhs: &str, aliases: &[String], constructor: &str) -> bool {
    aliases.iter().any(|alias| {
        let callee = format!("{alias}.{constructor}");
        call_args_if_starts_with(rhs, &callee)
            .and_then(first_python_string_literal)
            .is_some_and(|service| service == "sqs")
    })
}

fn sqs_resource_queue_name(rhs: &str, receiver: &str) -> Option<String> {
    for (method, key) in [("Queue", None), ("get_queue_by_name", Some("QueueName"))] {
        let callee = format!("{receiver}.{method}");
        let Some(args) = call_args_if_starts_with(rhs, &callee) else {
            continue;
        };
        let queue = if let Some(key) = key {
            python_keyword_string_literals(args, &[key])
                .into_iter()
                .next()
        } else {
            first_python_string_literal(args)
        };
        if let Some(queue) = queue {
            return Some(normalize_sqs_queue_name(queue));
        }
    }
    None
}

fn call_args_if_starts_with<'a>(rhs: &'a str, callee: &str) -> Option<&'a str> {
    let rest = rhs.strip_prefix(callee)?.trim_start();
    if !rest.starts_with('(') {
        return None;
    }
    let open = rhs.find('(')?;
    let close = crate::engine::find_matching_paren(rhs, open)?;
    rhs.get(open + 1..close)
}

fn extract_boto3_sqs_client_call_topics(
    text: &str,
    nodes: &[&SynthNode],
    callee: &str,
    kind: TopicEndpointKind,
    strategy: &str,
) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    for call in crate::engine::find_call_args(text, callee) {
        let Some(node) = crate::engine::node_at_offset(text, nodes, call.start)
            .or_else(|| crate::engine::pick_handler_node(nodes))
        else {
            continue;
        };
        for topic in python_keyword_string_literals(call.args, &["QueueUrl", "QueueName"]) {
            out.push(topic_detection(
                normalize_sqs_queue_name(topic),
                "sqs",
                kind,
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out
}

fn extract_bound_sqs_queue_call_topics(
    text: &str,
    nodes: &[&SynthNode],
    receiver: &str,
    queue: &str,
    method: &str,
    kind: TopicEndpointKind,
    strategy: &str,
) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    let callee = format!("{receiver}.{method}");
    for call in crate::engine::find_call_args(text, &callee) {
        let Some(node) = crate::engine::node_at_offset(text, nodes, call.start)
            .or_else(|| crate::engine::pick_handler_node(nodes))
        else {
            continue;
        };
        out.push(topic_detection(
            queue.to_string(),
            "sqs",
            kind,
            node.aka_id.clone(),
            strategy,
        ));
    }
    out
}

fn python_keyword_string_literals(args: &str, keys: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for arg in crate::engine::split_top_level_commas(args) {
        let Some((key, value)) = arg.split_once('=') else {
            continue;
        };
        if !keys.iter().any(|expected| key.trim() == *expected) {
            continue;
        }
        if let Some(value) = first_python_string_literal(value.trim()) {
            out.push(value);
        }
    }
    out.sort();
    out.dedup();
    out
}

fn first_python_string_literal(text: &str) -> Option<String> {
    let mut idx = 0usize;
    while idx < text.len() {
        let byte = *text.as_bytes().get(idx)?;
        if matches!(byte, b'\'' | b'"') {
            return crate::engine::read_string_literal(text, idx).map(|(literal, _)| literal);
        }
        idx += 1;
    }
    None
}

fn normalize_sqs_queue_name(value: String) -> String {
    value
        .rsplit('/')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(&value)
        .to_string()
}

fn is_python_ref(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|part| {
            let mut chars = part.chars();
            chars.next().is_some_and(crate::engine::is_ident_start)
                && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        })
}
