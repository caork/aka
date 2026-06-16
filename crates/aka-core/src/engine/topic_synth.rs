use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, find_matching_paren, node_at_offset, pick_handler_node,
    project_code_nodes_by_file, read_repo_text, source_annotations_before_node,
    split_top_level_commas, stable_hash, string_literals, EdgeRec, NodeRec, ProjectSourceSet,
    SynthNode,
};

mod python;
mod stream;

use python::extract_python_topic_detections;
use stream::{
    extract_stream_bridge_topics, functional_stream_binding_detections,
    spring_cloud_stream_bindings, stream_binding_detections, StreamBinding,
};

#[derive(Debug, Clone)]
pub(super) struct SynthTopic {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) broker: String,
    pub(super) sources: BTreeSet<String>,
    pub(super) consumer_groups: Vec<String>,
    pub(super) producers: Vec<SynthTopicEndpoint>,
    pub(super) consumers: Vec<SynthTopicEndpoint>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct SynthTopicEndpoint {
    pub(super) node_id: String,
    pub(super) file_path: String,
    pub(super) strategy: String,
    pub(super) evidence_source: String,
    pub(super) native_edge_type: Option<String>,
}

impl SynthTopic {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("broker".into(), Value::String(self.broker.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert(
            "topicSource".into(),
            Value::String(self.sources.iter().cloned().collect::<Vec<_>>().join("+")),
        );
        properties.insert(
            "sources".into(),
            Value::Array(self.sources.iter().cloned().map(Value::String).collect()),
        );
        if !self.consumer_groups.is_empty() {
            properties.insert(
                "consumerGroups".into(),
                Value::Array(
                    self.consumer_groups
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }
        NodeRec {
            id: self.id.clone(),
            label: "Topic".into(),
            properties,
        }
    }

    pub(super) fn edge_recs(&self) -> Vec<EdgeRec> {
        let mut out = Vec::new();
        for endpoint in &self.consumers {
            out.push(EdgeRec {
                id: format!(
                    "{}:consumes:{:016x}",
                    self.id,
                    stable_hash(&format!("{}|{}", endpoint.node_id, endpoint.strategy))
                ),
                source_id: endpoint.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: "CONSUMES_TOPIC".into(),
                confidence: 0.72,
                reason: "aka topic consumer synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": endpoint.evidence_source,
                    "kind": "topic-consumer",
                    "broker": self.broker,
                    "topic": self.name,
                    "strategy": endpoint.strategy,
                    "filePath": endpoint.file_path,
                    "nativeLabel": endpoint.native_edge_type.as_ref().map(|_| "Channel"),
                    "nativeEdgeType": endpoint.native_edge_type,
                })),
            });
        }
        for endpoint in &self.producers {
            out.push(EdgeRec {
                id: format!(
                    "{}:publishes:{:016x}",
                    self.id,
                    stable_hash(&format!("{}|{}", endpoint.node_id, endpoint.strategy))
                ),
                source_id: endpoint.node_id.clone(),
                target_id: self.id.clone(),
                edge_type: "PUBLISHES_TOPIC".into(),
                confidence: 0.68,
                reason: "aka topic publisher synthesis".into(),
                step: None,
                evidence: Some(json!({
                    "source": endpoint.evidence_source,
                    "kind": "topic-producer",
                    "broker": self.broker,
                    "topic": self.name,
                    "strategy": endpoint.strategy,
                    "filePath": endpoint.file_path,
                    "nativeLabel": endpoint.native_edge_type.as_ref().map(|_| "Channel"),
                    "nativeEdgeType": endpoint.native_edge_type,
                })),
            });
        }
        out
    }
}

pub(super) fn synthesize_topics_from_sources(
    repo: &Path,
    nodes: &BTreeMap<String, SynthNode>,
) -> Vec<SynthTopic> {
    let project_sources = ProjectSourceSet::discover(repo);
    let stream_bindings = spring_cloud_stream_bindings(repo, &project_sources);
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut topics: BTreeMap<(String, String), SynthTopic> = BTreeMap::new();
    let mut seen_edges: HashSet<(String, String, String, String)> = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for detection in extract_topic_detections(&text, &file_path, &file_nodes, &stream_bindings)
        {
            let key = (detection.broker.clone(), detection.topic.clone());
            let topic_id = format!(
                "topic:heuristic:{:016x}",
                stable_hash(&format!("{}|{}", detection.broker, detection.topic))
            );
            let endpoint = SynthTopicEndpoint {
                node_id: detection.node_id.clone(),
                file_path: file_path.clone(),
                strategy: detection.strategy.clone(),
                evidence_source: "aka-cbm-synth".into(),
                native_edge_type: None,
            };
            let topic = topics.entry(key).or_insert_with(|| SynthTopic {
                id: topic_id,
                name: detection.topic.clone(),
                broker: detection.broker.clone(),
                sources: BTreeSet::from(["source-scan".into()]),
                consumer_groups: Vec::new(),
                producers: Vec::new(),
                consumers: Vec::new(),
            });
            topic.sources.insert("source-scan".into());
            topic.consumer_groups.extend(detection.consumer_groups);
            let edge_key = (
                detection.kind.as_str().to_string(),
                detection.broker,
                detection.topic,
                detection.node_id,
            );
            if !seen_edges.insert(edge_key) {
                continue;
            }
            match detection.kind {
                TopicEndpointKind::Consumer => topic.consumers.push(endpoint),
                TopicEndpointKind::Producer => topic.producers.push(endpoint),
            }
        }
    }
    let mut out: Vec<SynthTopic> = topics.into_values().collect();
    for topic in &mut out {
        topic.consumers.sort();
        topic.consumers.dedup();
        topic.producers.sort();
        topic.producers.dedup();
        topic.consumer_groups.sort();
        topic.consumer_groups.dedup();
    }
    out.sort_by(|a, b| a.broker.cmp(&b.broker).then_with(|| a.name.cmp(&b.name)));
    out
}

#[derive(Debug, Clone)]
pub(super) struct NativeTopicDetection {
    pub(super) topic: String,
    pub(super) broker: String,
    pub(super) kind: TopicEndpointKind,
    pub(super) node_id: String,
    pub(super) file_path: String,
    pub(super) native_edge_type: String,
}

pub(super) fn merge_native_channel_topics(
    topics: &mut Vec<SynthTopic>,
    detections: Vec<NativeTopicDetection>,
) {
    let mut by_key: BTreeMap<(String, String), usize> = topics
        .iter()
        .enumerate()
        .map(|(idx, topic)| ((topic.broker.clone(), topic.name.clone()), idx))
        .collect();
    let mut seen_edges: HashSet<(String, String, String, String)> = topics
        .iter()
        .flat_map(|topic| {
            topic
                .consumers
                .iter()
                .map(|endpoint| {
                    (
                        "consumer".to_string(),
                        topic.broker.clone(),
                        topic.name.clone(),
                        endpoint.node_id.clone(),
                    )
                })
                .chain(topic.producers.iter().map(|endpoint| {
                    (
                        "producer".to_string(),
                        topic.broker.clone(),
                        topic.name.clone(),
                        endpoint.node_id.clone(),
                    )
                }))
        })
        .collect();
    for detection in detections {
        let key = (detection.broker.clone(), detection.topic.clone());
        let idx = if let Some(idx) = by_key.get(&key).copied() {
            idx
        } else {
            let topic_id = format!(
                "topic:heuristic:{:016x}",
                stable_hash(&format!("{}|{}", detection.broker, detection.topic))
            );
            topics.push(SynthTopic {
                id: topic_id,
                name: detection.topic.clone(),
                broker: detection.broker.clone(),
                sources: BTreeSet::from(["native-channel".into()]),
                consumer_groups: Vec::new(),
                producers: Vec::new(),
                consumers: Vec::new(),
            });
            let idx = topics.len() - 1;
            by_key.insert(key.clone(), idx);
            idx
        };
        let topic = &mut topics[idx];
        topic.sources.insert("native-channel".into());
        let edge_key = (
            detection.kind.as_str().to_string(),
            detection.broker,
            detection.topic,
            detection.node_id.clone(),
        );
        if !seen_edges.insert(edge_key) {
            continue;
        }
        let endpoint = SynthTopicEndpoint {
            node_id: detection.node_id,
            file_path: detection.file_path,
            strategy: "native-channel".into(),
            evidence_source: "aka-engine".into(),
            native_edge_type: Some(detection.native_edge_type),
        };
        match detection.kind {
            TopicEndpointKind::Consumer => topic.consumers.push(endpoint),
            TopicEndpointKind::Producer => topic.producers.push(endpoint),
        }
    }
    for topic in topics {
        topic.consumers.sort();
        topic.consumers.dedup();
        topic.producers.sort();
        topic.producers.dedup();
        topic.consumer_groups.sort();
        topic.consumer_groups.dedup();
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum TopicEndpointKind {
    Consumer,
    Producer,
}

impl TopicEndpointKind {
    fn as_str(self) -> &'static str {
        match self {
            TopicEndpointKind::Consumer => "consumer",
            TopicEndpointKind::Producer => "producer",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct TopicDetection {
    pub(super) topic: String,
    pub(super) broker: String,
    pub(super) kind: TopicEndpointKind,
    pub(super) node_id: String,
    pub(super) strategy: String,
    pub(super) consumer_groups: Vec<String>,
}

pub(super) fn topic_detection(
    topic: String,
    broker: &str,
    kind: TopicEndpointKind,
    node_id: String,
    strategy: &str,
) -> TopicDetection {
    TopicDetection {
        topic,
        broker: broker.into(),
        kind,
        node_id,
        strategy: strategy.into(),
        consumer_groups: Vec::new(),
    }
}

fn extract_topic_detections(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
    stream_bindings: &BTreeMap<String, StreamBinding>,
) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    let lower = file_path.to_ascii_lowercase();
    if lower.ends_with(".java")
        || lower.ends_with(".kt")
        || lower.ends_with(".kts")
        || lower.ends_with(".scala")
        || lower.ends_with(".groovy")
        || nodes.iter().any(|node| {
            matches!(
                node.language.to_ascii_lowercase().as_str(),
                "java" | "kotlin" | "scala" | "groovy"
            )
        })
    {
        out.extend(extract_jvm_topic_detections(text, nodes, stream_bindings));
    }
    if lower.ends_with(".py")
        || nodes
            .iter()
            .any(|node| node.language.eq_ignore_ascii_case("python"))
    {
        out.extend(extract_python_topic_detections(text, nodes));
    }
    out.sort_by(|a, b| {
        a.broker
            .cmp(&b.broker)
            .then_with(|| a.topic.cmp(&b.topic))
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out
}

fn extract_jvm_topic_detections(
    text: &str,
    nodes: &[&SynthNode],
    stream_bindings: &BTreeMap<String, StreamBinding>,
) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    let class_kafka_listeners = class_kafka_listeners(text, nodes);
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        for decorator in decorators_for_node(text, node) {
            if decorator.contains("KafkaListener") {
                let consumer_groups = annotation_string_values(&decorator, &["groupId", "group"]);
                for topic in annotation_string_values(&decorator, &["topics", "topic", "value"]) {
                    let mut detection = topic_detection(
                        topic,
                        "kafka",
                        TopicEndpointKind::Consumer,
                        node.aka_id.clone(),
                        "java-kafka-listener",
                    );
                    detection.consumer_groups = consumer_groups.clone();
                    out.push(detection);
                }
            }
            if decorator.contains("KafkaHandler") {
                for listener in class_kafka_listeners_for_node(node, &class_kafka_listeners) {
                    for topic in &listener.topics {
                        let mut detection = topic_detection(
                            topic.clone(),
                            "kafka",
                            TopicEndpointKind::Consumer,
                            node.aka_id.clone(),
                            "java-kafka-handler-class-listener",
                        );
                        detection.consumer_groups = listener.consumer_groups.clone();
                        out.push(detection);
                    }
                }
            }
            if decorator.contains("StreamListener") {
                for binding in annotation_string_values(&decorator, &["target", "value"]) {
                    out.extend(stream_binding_detections(
                        &binding,
                        stream_bindings,
                        TopicEndpointKind::Consumer,
                        node.aka_id.clone(),
                        "java-spring-cloud-stream-listener",
                    ));
                }
            }
            if decorator.contains("RabbitListener") {
                for topic in rabbit_listener_topics(&decorator) {
                    out.push(topic_detection(
                        topic,
                        "rabbitmq",
                        TopicEndpointKind::Consumer,
                        node.aka_id.clone(),
                        "java-rabbit-listener",
                    ));
                }
            }
            if decorator.contains("JmsListener") {
                for topic in annotation_string_values(
                    &decorator,
                    &["destination", "queue", "topic", "value"],
                ) {
                    out.push(topic_detection(
                        topic,
                        "jms",
                        TopicEndpointKind::Consumer,
                        node.aka_id.clone(),
                        "java-jms-listener",
                    ));
                }
            }
            if decorator.contains("SqsListener") {
                for topic in
                    annotation_string_values(&decorator, &["queueNames", "queueName", "value"])
                {
                    out.push(topic_detection(
                        topic,
                        "sqs",
                        TopicEndpointKind::Consumer,
                        node.aka_id.clone(),
                        "java-sqs-listener",
                    ));
                }
            }
            if decorator.contains("SendTo") {
                let strategy = if decorator.contains("SendToUser") {
                    "java-spring-stomp-send-to-user"
                } else {
                    "java-spring-stomp-send-to"
                };
                for topic in annotation_destination_values(&decorator, &["destinations", "value"]) {
                    out.push(topic_detection(
                        topic,
                        "stomp",
                        TopicEndpointKind::Producer,
                        node.aka_id.clone(),
                        strategy,
                    ));
                }
            }
        }
        out.extend(functional_stream_binding_detections(
            text,
            node,
            stream_bindings,
        ));
    }
    out.extend(extract_stream_bridge_topics(text, nodes, stream_bindings));
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "kafkaTemplate.send",
        "kafka",
        TopicEndpointKind::Producer,
        "java-kafka-template-send",
        0,
    ));
    out.extend(extract_rabbit_template_topics(text, nodes));
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "jmsTemplate.convertAndSend",
        "jms",
        TopicEndpointKind::Producer,
        "java-jms-template-send",
        0,
    ));
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "sqsTemplate.send",
        "sqs",
        TopicEndpointKind::Producer,
        "java-sqs-template-send",
        0,
    ));
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "sqsTemplate.convertAndSend",
        "sqs",
        TopicEndpointKind::Producer,
        "java-sqs-template-convert-and-send",
        0,
    ));
    out.extend(extract_call_destination_literals(
        text,
        nodes,
        "messagingTemplate.convertAndSend",
        "stomp",
        TopicEndpointKind::Producer,
        "java-spring-stomp-template-send",
        0,
    ));
    out.extend(extract_call_destination_literals(
        text,
        nodes,
        "simpMessagingTemplate.convertAndSend",
        "stomp",
        TopicEndpointKind::Producer,
        "java-spring-stomp-template-send",
        0,
    ));
    out.extend(extract_call_destination_literals(
        text,
        nodes,
        "messagingTemplate.convertAndSendToUser",
        "stomp",
        TopicEndpointKind::Producer,
        "java-spring-stomp-template-send-to-user",
        1,
    ));
    out.extend(extract_call_destination_literals(
        text,
        nodes,
        "simpMessagingTemplate.convertAndSendToUser",
        "stomp",
        TopicEndpointKind::Producer,
        "java-spring-stomp-template-send-to-user",
        1,
    ));
    out
}

#[derive(Debug, Clone)]
struct ClassKafkaListener {
    topics: Vec<String>,
    consumer_groups: Vec<String>,
}

fn decorators_for_node(text: &str, node: &SynthNode) -> Vec<String> {
    let mut decorators = node.decorators.clone();
    decorators.extend(source_annotations_before_node(text, node));
    decorators.sort();
    decorators.dedup();
    decorators
}

fn class_kafka_listeners(
    text: &str,
    nodes: &[&SynthNode],
) -> BTreeMap<String, Vec<ClassKafkaListener>> {
    let mut out: BTreeMap<String, Vec<ClassKafkaListener>> = BTreeMap::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Class" | "Interface" | "Record"))
    {
        let mut listeners = Vec::new();
        for decorator in decorators_for_node(text, node) {
            if !decorator.contains("KafkaListener") {
                continue;
            }
            let topics = annotation_string_values(&decorator, &["topics", "topic", "value"]);
            if topics.is_empty() {
                continue;
            }
            listeners.push(ClassKafkaListener {
                topics,
                consumer_groups: annotation_string_values(&decorator, &["groupId", "group"]),
            });
        }
        if listeners.is_empty() {
            continue;
        }
        for key in class_node_keys(node) {
            out.entry(key).or_default().extend(listeners.clone());
        }
    }
    out
}

fn class_kafka_listeners_for_node<'a>(
    node: &SynthNode,
    listeners: &'a BTreeMap<String, Vec<ClassKafkaListener>>,
) -> &'a [ClassKafkaListener] {
    let Some(parent) = node.parent_class.as_deref() else {
        return &[];
    };
    listeners
        .get(parent)
        .or_else(|| simple_class_name(parent).and_then(|simple| listeners.get(simple)))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn class_node_keys(node: &SynthNode) -> Vec<String> {
    let mut keys = vec![node.aka_id.clone(), node.qn.clone(), node.name.clone()];
    if let Some(simple) = simple_class_name(&node.qn) {
        keys.push(simple.to_string());
    }
    keys.sort();
    keys.dedup();
    keys.retain(|key| !key.is_empty());
    keys
}

fn simple_class_name(name: &str) -> Option<&str> {
    name.rsplit(['.', '$'])
        .next()
        .filter(|value| !value.is_empty())
}

fn annotation_string_values(annotation: &str, keys: &[&str]) -> Vec<String> {
    annotation_values(annotation, keys, string_literals)
}

fn annotation_destination_values(annotation: &str, keys: &[&str]) -> Vec<String> {
    annotation_values(annotation, keys, destination_literals)
}

fn annotation_values(
    annotation: &str,
    keys: &[&str],
    literal_reader: fn(&str) -> Vec<String>,
) -> Vec<String> {
    let Some(open) = annotation.find('(') else {
        return Vec::new();
    };
    let close = find_matching_paren(annotation, open).unwrap_or(annotation.len());
    let args = &annotation[open + 1..close];
    let mut values = Vec::new();
    for part in split_top_level_commas(args) {
        let part = part.trim();
        let value = if let Some((key, value)) = part.split_once('=') {
            if !keys.iter().any(|expected| key.trim().ends_with(expected)) {
                continue;
            }
            value.trim()
        } else if keys.contains(&"value") {
            part
        } else {
            continue;
        };
        values.extend(literal_reader(value));
    }
    values.sort();
    values.dedup();
    values
}

fn destination_literals(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        let Some(byte) = text.as_bytes().get(idx).copied() else {
            break;
        };
        if matches!(byte, b'\'' | b'"' | b'`') {
            if let Some((literal, end)) = super::read_string_literal(text, idx) {
                if is_destination_literal(&literal) {
                    values.push(literal);
                }
                idx = end;
                continue;
            }
        }
        idx += 1;
    }
    values.sort();
    values.dedup();
    values
}

fn is_destination_literal(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && !value.contains("://")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/'))
}

fn rabbit_listener_topics(annotation: &str) -> Vec<String> {
    let mut values = annotation_string_values(annotation, &["queues", "queue", "value"]);
    values.extend(queue_binding_values(annotation));
    values.sort();
    values.dedup();
    values
}

fn queue_binding_values(annotation: &str) -> Vec<String> {
    let mut values = Vec::new();
    for call in find_call_args(annotation, "@Queue") {
        let args = split_top_level_commas(call.args);
        for arg in args {
            let value = arg
                .split_once('=')
                .map(|(key, value)| {
                    if key.trim().ends_with("value") {
                        Some(value.trim())
                    } else {
                        None
                    }
                })
                .unwrap_or(Some(arg.trim()));
            if let Some(value) = value {
                values.extend(string_literals(value));
            }
        }
    }
    for call in find_call_args(annotation, "@QueueBinding") {
        for arg in split_top_level_commas(call.args) {
            let Some((key, value)) = arg.split_once('=') else {
                continue;
            };
            if key.trim().ends_with("key") {
                values.extend(string_literals(value));
            }
        }
    }
    values
}

fn extract_rabbit_template_topics(text: &str, nodes: &[&SynthNode]) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, "rabbitTemplate.convertAndSend") {
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        let args = split_top_level_commas(call.args);
        let topic_arg = if args.len() >= 3 {
            args.get(1)
        } else {
            args.first()
        };
        let Some(topic_arg) = topic_arg else {
            continue;
        };
        for topic in string_literals(topic_arg) {
            out.push(topic_detection(
                topic,
                "rabbitmq",
                TopicEndpointKind::Producer,
                node.aka_id.clone(),
                "java-rabbit-template-routing-key",
            ));
        }
    }
    out
}

fn extract_call_topic_literals(
    text: &str,
    nodes: &[&SynthNode],
    callee: &str,
    broker: &str,
    kind: TopicEndpointKind,
    strategy: &str,
    arg_index: usize,
) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, callee) {
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        let args = split_top_level_commas(call.args);
        let Some(arg) = args.get(arg_index) else {
            continue;
        };
        for topic in string_literals(arg) {
            out.push(topic_detection(
                topic,
                broker,
                kind,
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out
}

fn extract_call_destination_literals(
    text: &str,
    nodes: &[&SynthNode],
    callee: &str,
    broker: &str,
    kind: TopicEndpointKind,
    strategy: &str,
    arg_index: usize,
) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, callee) {
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        let args = split_top_level_commas(call.args);
        let Some(arg) = args.get(arg_index) else {
            continue;
        };
        for topic in destination_literals(arg) {
            out.push(topic_detection(
                topic,
                broker,
                kind,
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out
}

fn extract_keyword_topic_literals(
    text: &str,
    nodes: &[&SynthNode],
    callee: &str,
    keyword: &str,
    broker: &str,
    kind: TopicEndpointKind,
    strategy: &str,
) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, callee) {
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        for arg in split_top_level_commas(call.args) {
            let Some((key, value)) = arg.split_once('=') else {
                continue;
            };
            if key.trim() != keyword {
                continue;
            }
            for topic in string_literals(value) {
                out.push(topic_detection(
                    topic,
                    broker,
                    kind,
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    out
}
