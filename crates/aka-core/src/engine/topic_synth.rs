use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde_json::{json, Map, Value};

use super::{
    find_call_args, find_matching_paren, node_at_offset, pick_handler_node,
    project_code_nodes_by_file, read_repo_text, split_top_level_commas, stable_hash,
    string_literals, EdgeRec, NodeRec, ProjectSourceSet, SynthNode,
};

#[derive(Debug, Clone)]
pub(super) struct SynthTopic {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) broker: String,
    pub(super) producers: Vec<SynthTopicEndpoint>,
    pub(super) consumers: Vec<SynthTopicEndpoint>,
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(super) struct SynthTopicEndpoint {
    pub(super) node_id: String,
    pub(super) file_path: String,
    pub(super) strategy: String,
}

impl SynthTopic {
    pub(super) fn node_rec(&self) -> NodeRec {
        let mut properties = Map::new();
        properties.insert("name".into(), Value::String(self.name.clone()));
        properties.insert("broker".into(), Value::String(self.broker.clone()));
        properties.insert("source".into(), Value::String("aka-cbm-synth".into()));
        properties.insert("topicSource".into(), Value::String("source-scan".into()));
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
                    "source": "aka-cbm-synth",
                    "kind": "topic-consumer",
                    "broker": self.broker,
                    "topic": self.name,
                    "strategy": endpoint.strategy,
                    "filePath": endpoint.file_path,
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
                    "source": "aka-cbm-synth",
                    "kind": "topic-producer",
                    "broker": self.broker,
                    "topic": self.name,
                    "strategy": endpoint.strategy,
                    "filePath": endpoint.file_path,
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
    let by_file = project_code_nodes_by_file(repo, nodes, &project_sources);
    let mut topics: BTreeMap<(String, String), SynthTopic> = BTreeMap::new();
    let mut seen_edges: HashSet<(String, String, String, String)> = HashSet::new();
    for (file_path, file_nodes) in by_file {
        let Some(text) = read_repo_text(repo, &file_path) else {
            continue;
        };
        for detection in extract_topic_detections(&text, &file_path, &file_nodes) {
            let key = (detection.broker.clone(), detection.topic.clone());
            let topic_id = format!(
                "topic:heuristic:{:016x}",
                stable_hash(&format!("{}|{}", detection.broker, detection.topic))
            );
            let endpoint = SynthTopicEndpoint {
                node_id: detection.node_id.clone(),
                file_path: file_path.clone(),
                strategy: detection.strategy.clone(),
            };
            let topic = topics.entry(key).or_insert_with(|| SynthTopic {
                id: topic_id,
                name: detection.topic.clone(),
                broker: detection.broker.clone(),
                producers: Vec::new(),
                consumers: Vec::new(),
            });
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
    }
    out.sort_by(|a, b| a.broker.cmp(&b.broker).then_with(|| a.name.cmp(&b.name)));
    out
}

#[derive(Debug, Clone, Copy)]
enum TopicEndpointKind {
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
struct TopicDetection {
    topic: String,
    broker: String,
    kind: TopicEndpointKind,
    node_id: String,
    strategy: String,
}

fn extract_topic_detections(
    text: &str,
    file_path: &str,
    nodes: &[&SynthNode],
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
        out.extend(extract_jvm_topic_detections(text, nodes));
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

fn extract_jvm_topic_detections(text: &str, nodes: &[&SynthNode]) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.label.as_str(), "Function" | "Method"))
    {
        for decorator in &node.decorators {
            if decorator.contains("KafkaListener") {
                for topic in annotation_string_values(decorator, &["topics", "topic", "value"]) {
                    out.push(TopicDetection {
                        topic,
                        broker: "kafka".into(),
                        kind: TopicEndpointKind::Consumer,
                        node_id: node.aka_id.clone(),
                        strategy: "java-kafka-listener".into(),
                    });
                }
            }
            if decorator.contains("RabbitListener") {
                for topic in annotation_string_values(decorator, &["queues", "queue", "value"]) {
                    out.push(TopicDetection {
                        topic,
                        broker: "rabbitmq".into(),
                        kind: TopicEndpointKind::Consumer,
                        node_id: node.aka_id.clone(),
                        strategy: "java-rabbit-listener".into(),
                    });
                }
            }
        }
    }
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "kafkaTemplate.send",
        "kafka",
        TopicEndpointKind::Producer,
        "java-kafka-template-send",
        0,
    ));
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "rabbitTemplate.convertAndSend",
        "rabbitmq",
        TopicEndpointKind::Producer,
        "java-rabbit-template-send",
        0,
    ));
    out
}

fn extract_python_topic_detections(text: &str, nodes: &[&SynthNode]) -> Vec<TopicDetection> {
    let mut out = Vec::new();
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "KafkaConsumer",
        "kafka",
        TopicEndpointKind::Consumer,
        "python-kafka-consumer",
        0,
    ));
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "producer.send",
        "kafka",
        TopicEndpointKind::Producer,
        "python-producer-send",
        0,
    ));
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "producer.produce",
        "kafka",
        TopicEndpointKind::Producer,
        "python-producer-produce",
        0,
    ));
    out.extend(extract_keyword_topic_literals(
        text,
        nodes,
        "channel.basic_consume",
        "queue",
        "rabbitmq",
        TopicEndpointKind::Consumer,
        "python-rabbit-basic-consume",
    ));
    out.extend(extract_keyword_topic_literals(
        text,
        nodes,
        "channel.basic_publish",
        "exchange",
        "rabbitmq",
        TopicEndpointKind::Producer,
        "python-rabbit-basic-publish",
    ));
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "nc.subscribe",
        "nats",
        TopicEndpointKind::Consumer,
        "python-nats-subscribe",
        0,
    ));
    out.extend(extract_call_topic_literals(
        text,
        nodes,
        "nc.publish",
        "nats",
        TopicEndpointKind::Producer,
        "python-nats-publish",
        0,
    ));
    out
}

fn annotation_string_values(annotation: &str, keys: &[&str]) -> Vec<String> {
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
        values.extend(string_literals(value));
    }
    values.sort();
    values.dedup();
    values
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
            out.push(TopicDetection {
                topic,
                broker: broker.into(),
                kind,
                node_id: node.aka_id.clone(),
                strategy: strategy.into(),
            });
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
                out.push(TopicDetection {
                    topic,
                    broker: broker.into(),
                    kind,
                    node_id: node.aka_id.clone(),
                    strategy: strategy.into(),
                });
            }
        }
    }
    out
}
