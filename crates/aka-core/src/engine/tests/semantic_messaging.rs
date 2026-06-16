use super::*;
use serde_json::json;

mod java_jobs;
mod python_job_decorators;
mod python_jobs;

#[test]
fn bridges_native_channel_edges_into_topics() {
    let repo = temp_repo("native-channel-topics");
    std::fs::create_dir_all(repo.join("src/orders")).unwrap();
    let file = "src/orders/events.py";
    std::fs::write(
        repo.join(file),
        r#"def publish_order(event):
    pass

def consume_order(event):
    pass
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "publish_order",
        "src.orders.events.publish_order",
        file,
        (1, 2),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "consume_order",
        "src.orders.events.consume_order",
        file,
        (4, 5),
        json!({"language": "python"}),
    );
    insert_node_props(
        &conn,
        3,
        "Channel",
        "orders.created",
        "__channel__kafka__orders.created",
        "",
        json!({"transport": "kafka", "name": "orders.created"}),
    );
    insert_edge_props(&conn, 1, 1, 3, "EMITS", json!({"transport": "kafka"}));
    insert_edge_props(&conn, 2, 2, 3, "LISTENS_ON", json!({"transport": "kafka"}));

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("native channel topic");
    assert_eq!(topic.broker, "kafka");
    assert_eq!(topic.producers.len(), 1);
    assert_eq!(topic.consumers.len(), 1);
    assert_eq!(topic.producers[0].strategy, "native-channel");
    assert_eq!(topic.consumers[0].strategy, "native-channel");
    let node = topic.node_rec();
    assert_eq!(node.properties["topicSource"], json!("native-channel"));

    let edges = topic.edge_recs();
    assert!(edges.iter().any(|edge| edge.edge_type == "PUBLISHES_TOPIC"
        && edge.evidence.as_ref().and_then(|v| v.get("nativeEdgeType")) == Some(&json!("EMITS"))));
    assert!(edges.iter().any(|edge| edge.edge_type == "CONSUMES_TOPIC"
        && edge.evidence.as_ref().and_then(|v| v.get("nativeEdgeType"))
            == Some(&json!("LISTENS_ON"))));
}

#[test]
fn synthesizes_message_topics_from_config_files() {
    let repo = temp_repo("config-message-topics");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    std::fs::write(
        repo.join("src/main/resources/application.yml"),
        r#"orders:
  kafka:
    topic: orders.created
    group-id: orders-service
  rabbit:
    queue: orders.created.queue
    routing-key: orders.created
  sqs:
    queue-name: orders-created
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(".env"),
        r#"CELERY_QUEUE=orders.tasks
NATS_TOPIC=orders.broadcast
KAFKA_TOPIC=${DYNAMIC_TOPIC}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Config",
            "application.yml",
            "src/main/resources/application.yml",
            "src/main/resources/application.yml",
        ),
        (1, 10),
        json!({"language": "yaml"}),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Config", ".env", ".env", ".env"),
        (1, 3),
        json!({"language": "dotenv"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_config_topic(&synth, "orders.created", "kafka", &["orders-service"]);
    assert_config_topic(&synth, "orders.created.queue", "rabbitmq", &[]);
    assert_config_topic(&synth, "orders.created", "rabbitmq", &[]);
    assert_config_topic(&synth, "orders-created", "sqs", &[]);
    assert_config_topic(&synth, "orders.tasks", "celery", &[]);
    assert_config_topic(&synth, "orders.broadcast", "nats", &[]);
    assert!(synth
        .topics
        .iter()
        .all(|topic| topic.name != "${DYNAMIC_TOPIC}"));
}

fn assert_config_topic(synth: &SynthGraph, name: &str, broker: &str, consumer_groups: &[&str]) {
    let topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == name && topic.broker == broker)
        .unwrap_or_else(|| panic!("expected config topic {broker}:{name}"));
    assert!(topic.sources.contains("config-scan"));
    assert!(topic.producers.is_empty());
    assert!(topic.consumers.is_empty());
    assert_eq!(
        topic.consumer_groups,
        consumer_groups
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
    );
    let node = topic.node_rec();
    assert!(node.properties["sources"]
        .as_array()
        .unwrap()
        .contains(&json!("config-scan")));
}

#[test]
fn dedups_native_channel_edges_against_source_scan_topics() {
    let repo = temp_repo("native-channel-source-dedup");
    std::fs::create_dir_all(repo.join("src/orders")).unwrap();
    let file = "src/orders/events.py";
    std::fs::write(
        repo.join(file),
        r#"from kafka import KafkaProducer

def publish_order(event):
    producer = KafkaProducer()
    producer.send("orders.created", event)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "publish_order",
        "src.orders.events.publish_order",
        file,
        (3, 5),
        json!({"language": "python"}),
    );
    insert_node_props(
        &conn,
        2,
        "Channel",
        "orders.created",
        "__channel__kafka__orders.created",
        "",
        json!({"transport": "kafka", "name": "orders.created"}),
    );
    insert_edge_props(&conn, 1, 1, 2, "EMITS", json!({"transport": "kafka"}));

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("orders.created topic");
    assert_eq!(topic.broker, "kafka");
    assert_eq!(topic.producers.len(), 1);
    let node = topic.node_rec();
    assert_eq!(
        node.properties["topicSource"],
        json!("native-channel+source-scan")
    );
}

#[test]
fn synthesizes_java_message_topics() {
    let repo = temp_repo("java-message-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderEvents.java"),
        r#"package com.example.orders;

import org.springframework.kafka.annotation.KafkaListener;

class OrderEvents {
    @KafkaListener(topics = "orders.created", groupId = "orders-service")
    public void onCreated(String payload) {}

    @KafkaListener("orders.updated")
    public void onUpdated(String payload) {}

    public void publish(Object payload) {
        kafkaTemplate.send("orders.created", payload);
    }
}"#,
    )
    .unwrap();

    let file = "src/main/java/com/example/orders/OrderEvents.java";
    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "onCreated",
        "com.example.orders.OrderEvents.onCreated",
        file,
        json!({
            "decorators": ["@KafkaListener(topics = \"orders.created\", groupId = \"orders-service\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "onUpdated",
        "com.example.orders.OrderEvents.onUpdated",
        file,
        json!({
            "decorators": ["@KafkaListener(\"orders.updated\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "publish",
        "com.example.orders.OrderEvents.publish",
        file,
        (11, 13),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let created = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("orders.created topic");
    assert_eq!(created.broker, "kafka");
    assert_eq!(created.consumers.len(), 1);
    assert_eq!(created.producers.len(), 1);
    assert_eq!(
        created.consumers[0].node_id,
        "cbm:1:com.example.orders.OrderEvents.onCreated"
    );
    assert_eq!(
        created.producers[0].node_id,
        "cbm:3:com.example.orders.OrderEvents.publish"
    );
    let created_node = created.node_rec();
    assert_eq!(
        created_node.properties["consumerGroups"],
        json!(["orders-service"])
    );

    let updated = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.updated")
        .expect("orders.updated positional listener topic");
    assert_eq!(updated.broker, "kafka");
    assert_eq!(updated.consumers.len(), 1);

    let edge_types: Vec<_> = created
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"CONSUMES_TOPIC".to_string()));
    assert!(edge_types.contains(&"PUBLISHES_TOPIC".to_string()));
}

#[test]
fn synthesizes_java_kafka_handler_class_listener_topics() {
    let repo = temp_repo("java-kafka-handler-class-listener");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderLifecycleEvents.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.kafka.annotation.KafkaHandler;
import org.springframework.kafka.annotation.KafkaListener;

@KafkaListener(topics = "orders.lifecycle", groupId = "orders-service")
class OrderLifecycleEvents {
    @KafkaHandler
    public void onCreated(OrderCreated event) {}

    @KafkaHandler
    public void onCancelled(OrderCancelled event) {}
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderLifecycleEvents",
            "com.example.orders.OrderLifecycleEvents",
            file,
        ),
        (6, 13),
        json!({
            "decorators": ["@KafkaListener(topics = \"orders.lifecycle\", groupId = \"orders-service\")"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "onCreated",
            "com.example.orders.OrderLifecycleEvents.onCreated",
            file,
        ),
        (8, 9),
        json!({
            "decorators": ["@KafkaHandler"],
            "language": "java",
            "parent_class": "com.example.orders.OrderLifecycleEvents",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "onCancelled",
            "com.example.orders.OrderLifecycleEvents.onCancelled",
            file,
        ),
        (11, 12),
        json!({
            "decorators": ["@KafkaHandler"],
            "language": "java",
            "parent_class": "com.example.orders.OrderLifecycleEvents",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.lifecycle")
        .expect("orders.lifecycle topic");
    assert_eq!(topic.broker, "kafka");
    assert_eq!(topic.consumers.len(), 2);
    assert_eq!(topic.consumer_groups, vec!["orders-service"]);
    let consumer_ids: BTreeSet<_> = topic
        .consumers
        .iter()
        .map(|consumer| consumer.node_id.as_str())
        .collect();
    assert!(consumer_ids.contains("cbm:2:com.example.orders.OrderLifecycleEvents.onCreated"));
    assert!(consumer_ids.contains("cbm:3:com.example.orders.OrderLifecycleEvents.onCancelled"));
    assert!(topic
        .consumers
        .iter()
        .all(|consumer| consumer.strategy == "java-kafka-handler-class-listener"));
}

#[test]
fn synthesizes_java_kafka_topics_from_source_annotations_without_metadata() {
    let repo = temp_repo("java-kafka-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/OrderLifecycleEvents.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.kafka.annotation.KafkaHandler;
import org.springframework.kafka.annotation.KafkaListener;

@KafkaListener(
    topics = "orders.lifecycle",
    groupId = "orders-service")
class OrderLifecycleEvents {
    @KafkaHandler
    public void onCreated(OrderCreated event) {}

    @KafkaListener(
        topics = "orders.priority",
        groupId = "priority-service")
    public void onPriority(String payload) {}
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderLifecycleEvents",
            "com.example.orders.OrderLifecycleEvents",
            file,
        ),
        (9, 17),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "onCreated",
            "com.example.orders.OrderLifecycleEvents.onCreated",
            file,
        ),
        (11, 11),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderLifecycleEvents",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "onPriority",
            "com.example.orders.OrderLifecycleEvents.onPriority",
            file,
        ),
        (16, 16),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderLifecycleEvents",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let lifecycle = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.lifecycle")
        .expect("class-level kafka listener topic from source annotation");
    assert_eq!(lifecycle.broker, "kafka");
    assert_eq!(lifecycle.consumer_groups, vec!["orders-service"]);
    assert_eq!(lifecycle.consumers.len(), 1);
    assert_eq!(
        lifecycle.consumers[0].node_id,
        "cbm:2:com.example.orders.OrderLifecycleEvents.onCreated"
    );
    assert_eq!(
        lifecycle.consumers[0].strategy,
        "java-kafka-handler-class-listener"
    );

    let priority = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.priority")
        .expect("method-level kafka listener topic from source annotation");
    assert_eq!(priority.broker, "kafka");
    assert_eq!(priority.consumer_groups, vec!["priority-service"]);
    assert_eq!(priority.consumers.len(), 1);
    assert_eq!(
        priority.consumers[0].node_id,
        "cbm:3:com.example.orders.OrderLifecycleEvents.onPriority"
    );
    assert_eq!(priority.consumers[0].strategy, "java-kafka-listener");
}

#[test]
fn synthesizes_spring_rabbit_topics_from_routing_keys() {
    let repo = temp_repo("java-rabbit-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/RabbitEvents.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.amqp.rabbit.annotation.RabbitListener;
import org.springframework.amqp.rabbit.annotation.QueueBinding;
import org.springframework.amqp.rabbit.annotation.Queue;
import org.springframework.amqp.rabbit.annotation.Exchange;

class RabbitEvents {
    @RabbitListener(queues = "orders.created")
    public void consumeQueue(String payload) {}

    @RabbitListener(bindings = @QueueBinding(
        value = @Queue("orders.created"),
        exchange = @Exchange("orders.exchange"),
        key = "orders.shipped"))
    public void consumeBinding(String payload) {}

    public void publish(Object event) {
        rabbitTemplate.convertAndSend("orders.exchange", "orders.created", event);
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "consumeQueue",
        "com.example.orders.RabbitEvents.consumeQueue",
        file,
        json!({
            "decorators": ["@RabbitListener(queues = \"orders.created\")"],
            "language": "java",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Method",
        "consumeBinding",
        "com.example.orders.RabbitEvents.consumeBinding",
        file,
        json!({
            "decorators": ["@RabbitListener(bindings = @QueueBinding(value = @Queue(\"orders.created\"), exchange = @Exchange(\"orders.exchange\"), key = \"orders.shipped\"))"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "publish",
        "com.example.orders.RabbitEvents.publish",
        file,
        (17, 19),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let created = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("orders.created rabbit topic");
    assert_eq!(created.broker, "rabbitmq");
    assert_eq!(created.consumers.len(), 2);
    assert_eq!(created.producers.len(), 1);
    assert_eq!(
        created.producers[0].node_id,
        "cbm:3:com.example.orders.RabbitEvents.publish"
    );
    assert!(!synth
        .topics
        .iter()
        .any(|topic| topic.name == "orders.exchange"));

    let shipped = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.shipped")
        .expect("orders.shipped binding key topic");
    assert_eq!(shipped.broker, "rabbitmq");
    assert_eq!(shipped.consumers.len(), 1);
}

#[test]
fn synthesizes_spring_jms_topics() {
    let repo = temp_repo("java-jms-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/JmsEvents.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import org.springframework.jms.annotation.JmsListener;

class JmsEvents {
    @JmsListener(destination = "orders.created")
    public void consume(String payload) {}

    public void publish(Object event) {
        jmsTemplate.convertAndSend("orders.created", event);
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "consume",
        "com.example.orders.JmsEvents.consume",
        file,
        json!({
            "decorators": ["@JmsListener(destination = \"orders.created\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "publish",
        "com.example.orders.JmsEvents.publish",
        file,
        (9, 11),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("orders.created jms topic");
    assert_eq!(topic.broker, "jms");
    assert_eq!(topic.consumers.len(), 1);
    assert_eq!(topic.producers.len(), 1);
}

#[test]
fn synthesizes_spring_sqs_topics() {
    let repo = temp_repo("java-sqs-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let file = "src/main/java/com/example/orders/SqsEvents.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.orders;

import io.awspring.cloud.sqs.annotation.SqsListener;

class SqsEvents {
    @SqsListener("orders-created")
    public void consume(String payload) {}

    public void publish(Object event) {
        sqsTemplate.send("orders-created", event);
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Method",
        "consume",
        "com.example.orders.SqsEvents.consume",
        file,
        json!({
            "decorators": ["@SqsListener(\"orders-created\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "publish",
        "com.example.orders.SqsEvents.publish",
        file,
        (9, 11),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders-created")
        .expect("orders-created sqs topic");
    assert_eq!(topic.broker, "sqs");
    assert_eq!(topic.consumers.len(), 1);
    assert_eq!(topic.producers.len(), 1);
}

#[test]
fn synthesizes_spring_stomp_send_to_topics() {
    let repo = temp_repo("spring-stomp-topics");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/realtime")).unwrap();
    let file = "src/main/java/com/example/realtime/OrderSocket.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.realtime;

import org.springframework.messaging.handler.annotation.MessageMapping;
import org.springframework.messaging.handler.annotation.SendTo;
import org.springframework.messaging.simp.annotation.SendToUser;

class OrderSocket {
    @MessageMapping("/orders")
    @SendTo("/topic/orders")
    public OrderAck handleOrder(OrderMessage message) {
        return new OrderAck();
    }

    @MessageMapping("/orders/private")
    @SendToUser("/queue/orders")
    public OrderAck handlePrivate(OrderMessage message) {
        return new OrderAck();
    }

    public void broadcast(OrderAck ack) {
        messagingTemplate.convertAndSend("/topic/orders", ack);
    }

    public void sendUser(String user, OrderAck ack) {
        messagingTemplate.convertAndSendToUser(user, "/queue/orders", ack);
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Method",
            "handleOrder",
            "com.example.realtime.OrderSocket.handleOrder",
            file,
        ),
        (8, 11),
        json!({
            "decorators": ["@MessageMapping(\"/orders\")", "@SendTo(\"/topic/orders\")"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "handlePrivate",
            "com.example.realtime.OrderSocket.handlePrivate",
            file,
        ),
        (14, 17),
        json!({
            "decorators": ["@MessageMapping(\"/orders/private\")", "@SendToUser(\"/queue/orders\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "broadcast",
        "com.example.realtime.OrderSocket.broadcast",
        file,
        (19, 21),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "sendUser",
        "com.example.realtime.OrderSocket.sendUser",
        file,
        (23, 25),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let public_topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "/topic/orders")
        .expect("stomp public topic");
    assert_eq!(public_topic.broker, "stomp");
    assert_eq!(public_topic.producers.len(), 2);
    let public_producers: Vec<_> = public_topic
        .producers
        .iter()
        .map(|endpoint| endpoint.node_id.as_str())
        .collect();
    assert!(public_producers.contains(&"cbm:1:com.example.realtime.OrderSocket.handleOrder"));
    assert!(public_producers.contains(&"cbm:3:com.example.realtime.OrderSocket.broadcast"));

    let user_topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "/queue/orders")
        .expect("stomp user queue topic");
    assert_eq!(user_topic.broker, "stomp");
    assert_eq!(user_topic.producers.len(), 2);
    let user_strategies: Vec<_> = user_topic
        .producers
        .iter()
        .map(|endpoint| endpoint.strategy.as_str())
        .collect();
    assert!(user_strategies.contains(&"java-spring-stomp-send-to-user"));
    assert!(user_strategies.contains(&"java-spring-stomp-template-send-to-user"));
}

#[test]
fn synthesizes_python_message_topics() {
    let repo = temp_repo("python-message-topics");
    std::fs::write(
        repo.join("events.py"),
        r#"from kafka import KafkaConsumer, KafkaProducer

def consume():
    consumer = KafkaConsumer("orders.created")
    return consumer

def publish():
    producer = KafkaProducer()
    producer.send("orders.created", b"{}")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "consume",
        "events.consume",
        "events.py",
        (3, 5),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "publish",
        "events.publish",
        "events.py",
        (7, 9),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let topic = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created")
        .expect("orders.created topic");
    assert_eq!(topic.broker, "kafka");
    assert_eq!(topic.consumers.len(), 1);
    assert_eq!(topic.producers.len(), 1);
    assert_eq!(topic.consumers[0].node_id, "cbm:1:events.consume");
    assert_eq!(topic.producers[0].node_id, "cbm:2:events.publish");

    let edge_types: Vec<_> = topic
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"CONSUMES_TOPIC".to_string()));
    assert!(edge_types.contains(&"PUBLISHES_TOPIC".to_string()));
}

#[test]
fn ignores_python_message_topic_name_collisions_without_import_bindings() {
    let repo = temp_repo("python-message-topic-name-collisions");
    std::fs::write(
        repo.join("events.py"),
        r#"class Consumer:
    pass

def business_names(producer, channel, nc):
    consumer = Consumer("orders.created")
    producer.send("orders.created", b"{}")
    channel.basic_consume(queue="orders.created")
    channel.basic_publish(exchange="orders.created", routing_key="orders.created")
    nc.subscribe("orders.created")
    nc.publish("orders.created", b"{}")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "business_names",
        "events.business_names",
        "events.py",
        (4, 10),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(
        synth
            .topics
            .iter()
            .all(|topic| topic.name != "orders.created"),
        "plain business variables named producer/channel/nc must not become message topics"
    );
}

#[test]
fn synthesizes_python_message_topics_from_import_bound_receivers() {
    let repo = temp_repo("python-message-import-bound-topics");
    std::fs::write(
        repo.join("events.py"),
        r#"from confluent_kafka import Consumer as CConsumer, Producer
import pika
import nats

def consume_kafka():
    consumer = CConsumer({"group.id": "orders-service"})
    consumer.subscribe(["orders.created"])

def publish_kafka():
    p = Producer({})
    p.produce("orders.created", b"{}")

def rabbit_publish(conn):
    ch = conn.channel()
    ch.basic_publish(exchange="orders.exchange", routing_key="orders.created")

async def nats_publish():
    client = await nats.connect("nats://localhost:4222")
    await client.publish("orders.created", b"{}")
"#,
    )
    .unwrap();

    let conn = test_conn();
    for (id, name, qn, lines) in [
        (1, "consume_kafka", "events.consume_kafka", (5, 7)),
        (2, "publish_kafka", "events.publish_kafka", (9, 11)),
        (3, "rabbit_publish", "events.rabbit_publish", (13, 15)),
        (4, "nats_publish", "events.nats_publish", (17, 19)),
    ] {
        insert_function_node_props_at(
            &conn,
            id,
            name,
            qn,
            "events.py",
            lines,
            json!({
                "language": "python",
            }),
        );
    }

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let kafka = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created" && topic.broker == "kafka")
        .expect("kafka orders.created topic");
    assert_eq!(kafka.consumers.len(), 1);
    assert_eq!(kafka.producers.len(), 1);
    assert_eq!(kafka.consumers[0].node_id, "cbm:1:events.consume_kafka");
    assert_eq!(kafka.producers[0].node_id, "cbm:2:events.publish_kafka");

    let rabbit = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.exchange" && topic.broker == "rabbitmq")
        .expect("rabbit exchange topic");
    assert_eq!(rabbit.producers[0].node_id, "cbm:3:events.rabbit_publish");

    let nats = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders.created" && topic.broker == "nats")
        .expect("nats orders.created topic");
    assert_eq!(nats.producers[0].node_id, "cbm:4:events.nats_publish");
}

#[test]
fn synthesizes_python_boto3_sqs_topics() {
    let repo = temp_repo("python-boto3-sqs-topics");
    std::fs::write(
        repo.join("sqs_events.py"),
        r#"import boto3 as aws

sqs = aws.client("sqs")
sqs_resource = aws.resource("sqs")

def publish_order(event):
    sqs.send_message(QueueUrl="https://sqs.us-east-1.amazonaws.com/123/orders-created", MessageBody=event)

def consume_order():
    return sqs.receive_message(QueueUrl="https://sqs.us-east-1.amazonaws.com/123/orders-created")

def publish_dead_letter(event):
    queue = sqs_resource.get_queue_by_name(QueueName="orders-dlq")
    queue.send_message(MessageBody=event)
"#,
    )
    .unwrap();

    let conn = test_conn();
    for (id, name, qn, lines) in [
        (1, "publish_order", "sqs_events.publish_order", (6, 7)),
        (2, "consume_order", "sqs_events.consume_order", (9, 10)),
        (
            3,
            "publish_dead_letter",
            "sqs_events.publish_dead_letter",
            (12, 14),
        ),
    ] {
        insert_function_node_props_at(
            &conn,
            id,
            name,
            qn,
            "sqs_events.py",
            lines,
            json!({
                "language": "python",
            }),
        );
    }

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let orders = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders-created" && topic.broker == "sqs")
        .expect("orders-created sqs topic");
    assert_eq!(orders.producers.len(), 1);
    assert_eq!(orders.consumers.len(), 1);
    assert_eq!(
        orders.producers[0].node_id,
        "cbm:1:sqs_events.publish_order"
    );
    assert_eq!(
        orders.consumers[0].node_id,
        "cbm:2:sqs_events.consume_order"
    );

    let dlq = synth
        .topics
        .iter()
        .find(|topic| topic.name == "orders-dlq" && topic.broker == "sqs")
        .expect("orders-dlq sqs topic");
    assert_eq!(dlq.producers.len(), 1);
    assert_eq!(
        dlq.producers[0].node_id,
        "cbm:3:sqs_events.publish_dead_letter"
    );
    assert_eq!(dlq.producers[0].strategy, "python-boto3-sqs-send-message");
}
