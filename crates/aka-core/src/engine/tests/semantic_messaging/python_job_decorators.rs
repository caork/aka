use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_python_jobs_from_source_decorators_without_metadata() {
    let repo = temp_repo("python-job-source-decorators");
    std::fs::write(
        repo.join("tasks.py"),
        r#"from celery import shared_task
import dramatiq
from huey import RedisHuey
from rq.decorators import job
from apscheduler.schedulers.background import BackgroundScheduler

huey = RedisHuey("orders")
scheduler = BackgroundScheduler()

@shared_task(
    name="orders.sync")
def sync_orders():
    return []

@job("orders")
def rebuild_orders(order_id):
    return order_id

@dramatiq.actor(actor_name="orders.rebuild", queue_name="orders")
def rebuild_actor(order_id):
    return order_id

@huey.periodic_task(crontab(minute="*/5"), name="orders.refresh")
def refresh_orders():
    return None

@scheduler.scheduled_job("cron", id="orders.cleanup", hour="3")
def cleanup_orders():
    return None
"#,
    )
    .unwrap();

    let conn = test_conn();
    for (id, name, qn, lines) in [
        (1, "sync_orders", "tasks.sync_orders", (12, 13)),
        (2, "rebuild_orders", "tasks.rebuild_orders", (16, 17)),
        (3, "rebuild_actor", "tasks.rebuild_actor", (20, 21)),
        (4, "refresh_orders", "tasks.refresh_orders", (24, 25)),
        (5, "cleanup_orders", "tasks.cleanup_orders", (28, 29)),
    ] {
        insert_function_node_props_at(
            &conn,
            id,
            name,
            qn,
            "tasks.py",
            lines,
            json!({
                "language": "python",
            }),
        );
    }

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (name, job_type, handler) in [
        ("orders.sync", "celery-task", "cbm:1:tasks.sync_orders"),
        ("orders", "rq-job", "cbm:2:tasks.rebuild_orders"),
        (
            "orders.rebuild",
            "dramatiq-actor",
            "cbm:3:tasks.rebuild_actor",
        ),
        (
            "orders.refresh",
            "huey-periodic-task",
            "cbm:4:tasks.refresh_orders",
        ),
        (
            "orders.cleanup",
            "apscheduler-job",
            "cbm:5:tasks.cleanup_orders",
        ),
    ] {
        let job = synth
            .jobs
            .iter()
            .find(|job| job.name == name)
            .unwrap_or_else(|| panic!("expected source decorator job {name}"));
        assert_eq!(job.job_type, job_type);
        assert_eq!(job.handler_id.as_deref(), Some(handler));
        assert!(job
            .edge_recs()
            .iter()
            .any(|edge| edge.edge_type == "HANDLES_JOB" && edge.source_id == handler));
    }
}
