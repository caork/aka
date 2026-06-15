use super::*;
use serde_json::json;

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

#[test]
fn synthesizes_spring_batch_job_and_step_beans() {
    let repo = temp_repo("spring-batch-job-step-beans");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/jobs")).unwrap();
    let file = "src/main/java/com/example/jobs/OrderBatchConfig.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.jobs;

import org.springframework.batch.core.Job;
import org.springframework.batch.core.Step;
import org.springframework.batch.core.job.builder.JobBuilder;
import org.springframework.batch.core.step.builder.StepBuilder;
import org.springframework.context.annotation.Bean;

class OrderBatchConfig {
    @Bean
    Job importOrdersJob(Step loadOrdersStep) {
        return new JobBuilder("orders.import")
            .start(loadOrdersStep)
            .build();
    }

    @Bean
    Step loadOrdersStep() {
        return new StepBuilder("orders.load")
            .tasklet((contribution, context) -> null)
            .build();
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
            "importOrdersJob",
            "com.example.jobs.OrderBatchConfig.importOrdersJob",
            file,
        ),
        (11, 15),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "loadOrdersStep",
            "com.example.jobs.OrderBatchConfig.loadOrdersStep",
            file,
        ),
        (18, 22),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let job = synth
        .jobs
        .iter()
        .find(|job| job.handler_id == "cbm:1:com.example.jobs.OrderBatchConfig.importOrdersJob")
        .expect("spring batch job bean");
    assert_eq!(job.name, "orders.import");
    assert_eq!(job.job_type, "spring-batch-job");
    assert_eq!(job.strategy, "java-spring-batch-job-bean");
    assert!(job.edge_recs().iter().any(|edge| {
        edge.edge_type == "USES_STEP"
            && edge.source_id == job.id
            && edge.target_id == "cbm:2:com.example.jobs.OrderBatchConfig.loadOrdersStep"
    }));

    let step = synth
        .jobs
        .iter()
        .find(|job| job.handler_id == "cbm:2:com.example.jobs.OrderBatchConfig.loadOrdersStep")
        .expect("spring batch step bean");
    assert_eq!(step.name, "orders.load");
    assert_eq!(step.job_type, "spring-batch-step");
    assert_eq!(step.strategy, "java-spring-batch-step-bean");
}
