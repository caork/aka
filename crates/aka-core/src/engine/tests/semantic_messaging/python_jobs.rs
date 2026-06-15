use super::*;
use serde_json::json;

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
fn synthesizes_python_fastapi_background_task_jobs() {
    let repo = temp_repo("python-fastapi-background-tasks");
    std::fs::write(
        repo.join("api.py"),
        r#"from fastapi import BackgroundTasks, FastAPI

app = FastAPI()

def send_receipt(order_id: str):
    deliver_receipt(order_id)

def deliver_receipt(order_id: str):
    return order_id

@app.post("/orders/{order_id}/receipt")
def submit_receipt(order_id: str, background_tasks: BackgroundTasks):
    background_tasks.add_task(send_receipt, order_id)
    return {"queued": True}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "send_receipt",
        "api.send_receipt",
        "api.py",
        (5, 6),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "deliver_receipt",
        "api.deliver_receipt",
        "api.py",
        (8, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "submit_receipt",
        "api.submit_receipt",
        "api.py",
        (12, 14),
        json!({
            "decorators": ["@app.post(\"/orders/{order_id}/receipt\")"],
            "language": "python",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let job = synth
        .jobs
        .iter()
        .find(|job| job.handler_id == "cbm:1:api.send_receipt")
        .expect("fastapi background task job");
    assert_eq!(job.job_type, "fastapi-background-task");
    assert_eq!(job.strategy, "python-fastapi-background-task");
    assert!(job.edge_recs().iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB" && edge.source_id == "cbm:3:api.submit_receipt"
    }));
}

#[test]
fn synthesizes_python_rq_jobs() {
    let repo = temp_repo("python-rq-jobs");
    std::fs::write(
        repo.join("jobs.py"),
        r#"from redis import Redis
from rq import Queue
from rq.decorators import job

redis = Redis()
queue = Queue("orders", connection=redis)

@job("orders")
def rebuild_orders(order_id):
    return order_id

def enqueue_orders(order_id):
    queue.enqueue(rebuild_orders, order_id)
    queue.enqueue_call(func=rebuild_orders, args=(order_id,))
    queue.enqueue_in(60, rebuild_orders, order_id)
    queue.enqueue_at("2026-01-01T00:00:00Z", rebuild_orders, order_id)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "rebuild_orders",
        "jobs.rebuild_orders",
        "jobs.py",
        (9, 10),
        json!({
            "decorators": ["@job(\"orders\")"],
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "enqueue_orders",
        "jobs.enqueue_orders",
        "jobs.py",
        (12, 16),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let job = synth
        .jobs
        .iter()
        .find(|job| job.name == "orders")
        .expect("rq queue job");
    assert_eq!(job.job_type, "rq-job");
    assert_eq!(job.strategy, "python-rq-job");
    assert_eq!(job.handler_id, "cbm:1:jobs.rebuild_orders");
    let edges = job.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "HANDLES_JOB" && edge.source_id == "cbm:1:jobs.rebuild_orders"
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "ENQUEUES_JOB" && edge.source_id == "cbm:2:jobs.enqueue_orders"
    }));
    let enqueue_strategies: BTreeSet<_> = edges
        .iter()
        .filter(|edge| edge.edge_type == "ENQUEUES_JOB")
        .filter_map(|edge| {
            edge.evidence
                .as_ref()
                .and_then(|value| value.get("strategy"))
                .and_then(|value| value.as_str())
        })
        .collect();
    assert!(enqueue_strategies.contains("python-rq-enqueue"));
    assert!(enqueue_strategies.contains("python-rq-enqueue-call"));
    assert!(enqueue_strategies.contains("python-rq-enqueue-in"));
    assert!(enqueue_strategies.contains("python-rq-enqueue-at"));
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
