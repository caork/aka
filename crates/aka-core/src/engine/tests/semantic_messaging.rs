use super::*;
use serde_json::json;

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
fn synthesizes_python_task_jobs() {
    let repo = temp_repo("python-task-jobs");
    std::fs::write(
        repo.join("tasks.py"),
        r#"from celery import shared_task
from apscheduler.schedulers.background import BackgroundScheduler

@shared_task(name="orders.sync")
def sync_orders():
    load_orders()

def load_orders():
    write_orders()

def write_orders():
    return []

scheduler = BackgroundScheduler()

@scheduler.scheduled_job("cron", id="orders.cleanup", hour="3")
def cleanup_orders():
    return None

def enqueue_orders():
    sync_orders.delay()
    app.send_task("orders.cleanup")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "sync_orders",
        "tasks.sync_orders",
        "tasks.py",
        (4, 6),
        json!({
            "decorators": ["@shared_task(name=\"orders.sync\")"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_orders",
        "tasks.load_orders",
        "tasks.py",
        (8, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "write_orders",
        "tasks.write_orders",
        "tasks.py",
        (11, 12),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "cleanup_orders",
        "tasks.cleanup_orders",
        "tasks.py",
        (17, 18),
        json!({
            "decorators": ["@scheduler.scheduled_job(\"cron\", id=\"orders.cleanup\", hour=\"3\")"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "enqueue_orders",
        "tasks.enqueue_orders",
        "tasks.py",
        (20, 22),
        json!({
            "language": "python",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let celery = synth
        .jobs
        .iter()
        .find(|job| job.name == "orders.sync")
        .expect("celery task job");
    assert_eq!(celery.job_type, "celery-task");
    assert_eq!(celery.handler_id, "cbm:1:tasks.sync_orders");
    assert_eq!(celery.process_ids.len(), 1);

    let aps = synth
        .jobs
        .iter()
        .find(|job| job.name == "orders.cleanup")
        .expect("apscheduler job");
    assert_eq!(aps.job_type, "apscheduler-job");
    assert!(aps
        .schedule
        .as_deref()
        .is_some_and(|schedule| schedule.contains("trigger=cron") && schedule.contains("hour=3")));
    assert!(celery.edge_recs().iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB" && edge.source_id == "cbm:5:tasks.enqueue_orders"
    }));
    assert!(aps.edge_recs().iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB" && edge.source_id == "cbm:5:tasks.enqueue_orders"
    }));
}

#[test]
fn synthesizes_python_dramatiq_jobs() {
    let repo = temp_repo("python-dramatiq-jobs");
    std::fs::write(
        repo.join("actors.py"),
        r#"import dramatiq

@dramatiq.actor(actor_name="orders.rebuild", queue_name="orders")
def rebuild_orders(order_id):
    return order_id

def enqueue_orders(order_id):
    rebuild_orders.send(order_id)
    rebuild_orders.send_with_options(args=(order_id,), delay=1000)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "rebuild_orders",
        "actors.rebuild_orders",
        "actors.py",
        (4, 5),
        json!({
            "decorators": ["@dramatiq.actor(actor_name=\"orders.rebuild\", queue_name=\"orders\")"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "enqueue_orders",
        "actors.enqueue_orders",
        "actors.py",
        (7, 9),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let job = synth
        .jobs
        .iter()
        .find(|job| job.name == "orders.rebuild")
        .expect("dramatiq actor job");
    assert_eq!(job.job_type, "dramatiq-actor");
    assert_eq!(job.handler_id, "cbm:1:actors.rebuild_orders");
    let edges = job.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "HANDLES_JOB" && edge.source_id == "cbm:1:actors.rebuild_orders"
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB" && edge.source_id == "cbm:2:actors.enqueue_orders"
    }));
}

#[test]
fn synthesizes_python_huey_jobs() {
    let repo = temp_repo("python-huey-jobs");
    std::fs::write(
        repo.join("huey_tasks.py"),
        r#"from huey import RedisHuey

huey = RedisHuey("orders")

@huey.task(name="orders.rebuild")
def rebuild_orders(order_id):
    return order_id

def enqueue_orders(order_id):
    rebuild_orders.schedule(args=(order_id,), delay=60)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "rebuild_orders",
        "huey_tasks.rebuild_orders",
        "huey_tasks.py",
        (6, 7),
        json!({
            "decorators": ["@huey.task(name=\"orders.rebuild\")"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "enqueue_orders",
        "huey_tasks.enqueue_orders",
        "huey_tasks.py",
        (9, 10),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let job = synth
        .jobs
        .iter()
        .find(|job| job.name == "orders.rebuild")
        .expect("huey task job");
    assert_eq!(job.job_type, "huey-task");
    assert_eq!(job.handler_id, "cbm:1:huey_tasks.rebuild_orders");
    assert!(job.edge_recs().iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB" && edge.source_id == "cbm:2:huey_tasks.enqueue_orders"
    }));
}
