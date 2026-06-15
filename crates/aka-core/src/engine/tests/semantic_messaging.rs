use super::*;
use serde_json::json;

mod python_jobs;

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
        r#"from kafka import KafkaConsumer

def consume():
    consumer = KafkaConsumer("orders.created")
    return consumer

def publish(producer):
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
        (7, 8),
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
fn synthesizes_spring_scheduled_jobs() {
    let repo = temp_repo("spring-scheduled-jobs");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/jobs")).unwrap();
    let file = "src/main/java/com/example/jobs/BillingJobs.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.jobs;

import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.scheduling.annotation.Async;

class BillingJobs {
    @Scheduled(cron = "0 0 * * * *")
    public void settleInvoices() {
        settleOpenInvoices();
    }

    void settleOpenInvoices() {
        writeLedger();
    }

    void writeLedger() {}
}

class BillingController {
    private BillingJobs jobs;

    void submitInvoice() {
        jobs.rebuildInvoiceCache();
    }
}

class AsyncBillingJobs {
    @Async
    void rebuildInvoiceCache() {}
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "settleInvoices",
        "com.example.jobs.BillingJobs.settleInvoices",
        file,
        (7, 9),
        json!({
            "decorators": ["@Scheduled(cron = \"0 0 * * * *\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "settleOpenInvoices",
        "com.example.jobs.BillingJobs.settleOpenInvoices",
        file,
        (11, 13),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "writeLedger",
        "com.example.jobs.BillingJobs.writeLedger",
        file,
        (15, 15),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "submitInvoice",
        "com.example.jobs.BillingController.submitInvoice",
        file,
        (21, 23),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "rebuildInvoiceCache",
        "com.example.jobs.AsyncBillingJobs.rebuildInvoiceCache",
        file,
        (28, 29),
        json!({
            "decorators": ["@Async"],
            "language": "java",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let job = synth
        .jobs
        .iter()
        .find(|job| job.handler_id == "cbm:1:com.example.jobs.BillingJobs.settleInvoices")
        .expect("spring scheduled job");
    assert_eq!(job.job_type, "spring-scheduled");
    assert_eq!(job.schedule.as_deref(), Some("cron=0 0 * * * *"));
    assert_eq!(job.process_ids.len(), 1);

    let edge_types: Vec<_> = job
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"HANDLES_JOB".to_string()));
    assert!(edge_types.contains(&"ENTRY_POINT_OF".to_string()));

    let async_job = synth
        .jobs
        .iter()
        .find(|job| job.handler_id == "cbm:5:com.example.jobs.AsyncBillingJobs.rebuildInvoiceCache")
        .expect("spring async job");
    assert_eq!(async_job.job_type, "spring-async");
    assert!(async_job.edge_recs().iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB"
            && edge.source_id == "cbm:4:com.example.jobs.BillingController.submitInvoice"
    }));
}

#[test]
fn synthesizes_spring_jobs_from_source_annotations_without_metadata() {
    let repo = temp_repo("spring-scheduled-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/jobs")).unwrap();
    let file = "src/main/java/com/example/jobs/BillingJobs.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.jobs;

import org.springframework.scheduling.annotation.Async;
import org.springframework.scheduling.annotation.Scheduled;

class BillingJobs {
    @Scheduled(fixedDelayString = "${billing.delay.ms}")
    void refreshInvoices() {
        writeLedger();
    }

    void writeLedger() {
        persistLedger();
    }

    void persistLedger() {}

    @Async
    void rebuildInvoiceCache() {}
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "refreshInvoices",
        "com.example.jobs.BillingJobs.refreshInvoices",
        file,
        (8, 10),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "writeLedger",
        "com.example.jobs.BillingJobs.writeLedger",
        file,
        (12, 14),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "persistLedger",
        "com.example.jobs.BillingJobs.persistLedger",
        file,
        (16, 16),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "rebuildInvoiceCache",
        "com.example.jobs.BillingJobs.rebuildInvoiceCache",
        file,
        (19, 19),
        json!({
            "language": "java",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let scheduled = synth
        .jobs
        .iter()
        .find(|job| job.handler_id == "cbm:1:com.example.jobs.BillingJobs.refreshInvoices")
        .expect("scheduled job from source annotation");
    assert_eq!(scheduled.job_type, "spring-scheduled");
    assert_eq!(
        scheduled.schedule.as_deref(),
        Some("fixedDelayString=${billing.delay.ms}")
    );
    assert_eq!(
        scheduled.strategy,
        "java-spring-scheduled-source-annotation"
    );
    assert_eq!(scheduled.process_ids.len(), 1);

    let async_job = synth
        .jobs
        .iter()
        .find(|job| job.handler_id == "cbm:4:com.example.jobs.BillingJobs.rebuildInvoiceCache")
        .expect("async job from source annotation");
    assert_eq!(async_job.job_type, "spring-async");
    assert_eq!(async_job.strategy, "java-spring-async-source-annotation");
}
