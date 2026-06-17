use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_warehouse_resources() {
    let repo = temp_repo("configured-warehouse-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"warehouse:
  snowflake:
    account: aka.us-east-1
  bigquery:
    project-id: aka-prod
  redshift:
    jdbc-url: jdbc:redshift://aka.redshift.amazonaws.com:5439/analytics
  databricks:
    host: https://dbc.example.databricks.com
disabled:
  snowflake:
    account: ${SNOWFLAKE_ACCOUNT}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 13),
        json!({"language": "yaml"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_warehouse_edge(
        &synth,
        "warehouse:snowflake",
        &config_id("warehouse.snowflake.account"),
        "snowflake-config",
    );
    assert_warehouse_edge(
        &synth,
        "warehouse:bigquery",
        &config_id("warehouse.bigquery.project.id"),
        "bigquery-config",
    );
    assert_warehouse_edge(
        &synth,
        "warehouse:redshift",
        &config_id("warehouse.redshift.jdbc.url"),
        "redshift-config",
    );
    assert_warehouse_edge(
        &synth,
        "warehouse:databricks",
        &config_id("warehouse.databricks.host"),
        "databricks-config",
    );
    assert!(!synth.resources.iter().any(|resource| {
        resource.url == "warehouse:snowflake"
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("disabled.snowflake.account"))
    }));
}

#[test]
fn synthesizes_python_warehouse_resources() {
    let repo = temp_repo("python-warehouse-resources");
    std::fs::write(
        repo.join("warehouse_ops.py"),
        r#"import snowflake.connector
from google.cloud import bigquery
from databricks import sql

sf = snowflake.connector.connect(account="aka")
sf_cursor = sf.cursor()
bq = bigquery.Client(project="aka-prod")
dbx = sql.connect(server_hostname="dbc.example.databricks.com")
dbx_cursor = dbx.cursor()

def snowflake_report():
    return sf_cursor.execute("select count(*) from orders")

def bigquery_report():
    return bq.query("select count(*) from `aka.orders`")

def databricks_report():
    return dbx_cursor.execute("select count(*) from orders")

def ordinary(cursor, client):
    cursor.execute("select 1")
    client.query("select 1")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "snowflake_report",
        "warehouse_ops.snowflake_report",
        "warehouse_ops.py",
        (11, 12),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "bigquery_report",
        "warehouse_ops.bigquery_report",
        "warehouse_ops.py",
        (14, 15),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "databricks_report",
        "warehouse_ops.databricks_report",
        "warehouse_ops.py",
        (17, 18),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "ordinary",
        "warehouse_ops.ordinary",
        "warehouse_ops.py",
        (20, 22),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_warehouse_edge(
        &synth,
        "warehouse:snowflake",
        "cbm:1:warehouse_ops.snowflake_report",
        "python-snowflake-execute",
    );
    assert_warehouse_edge(
        &synth,
        "warehouse:bigquery",
        "cbm:2:warehouse_ops.bigquery_report",
        "python-bigquery-query",
    );
    assert_warehouse_edge(
        &synth,
        "warehouse:databricks",
        "cbm:3:warehouse_ops.databricks_report",
        "python-databricks-sql-execute",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:4:warehouse_ops.ordinary"));
}

#[test]
fn synthesizes_java_warehouse_resources() {
    let repo = temp_repo("java-warehouse-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/warehouse")).unwrap();
    let file = "src/main/java/com/example/warehouse/WarehouseGateway.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.warehouse;

import com.google.cloud.bigquery.BigQuery;
import net.snowflake.client.jdbc.SnowflakeStatement;

class WarehouseGateway {
    Object bigquery(BigQuery bigQuery, Object config) throws Exception {
        return bigQuery.query(config);
    }

    Object snowflake(SnowflakeStatement statement, String sql) throws Exception {
        return statement.executeQuery(sql);
    }

    Object ordinary(Client client, Statement statement) throws Exception {
        client.query("select 1");
        return statement.executeQuery("select 1");
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "bigquery",
        "com.example.warehouse.WarehouseGateway.bigquery",
        file,
        (7, 9),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "snowflake",
        "com.example.warehouse.WarehouseGateway.snowflake",
        file,
        (11, 13),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "ordinary",
        "com.example.warehouse.WarehouseGateway.ordinary",
        file,
        (15, 18),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_warehouse_edge(
        &synth,
        "warehouse:bigquery",
        "cbm:1:com.example.warehouse.WarehouseGateway.bigquery",
        "java-bigquery-query",
    );
    assert_warehouse_edge(
        &synth,
        "warehouse:snowflake",
        "cbm:2:com.example.warehouse.WarehouseGateway.snowflake",
        "java-snowflake-execute-query",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:3:com.example.warehouse.WarehouseGateway.ordinary"));
}

fn assert_warehouse_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected warehouse resource {url}"));
    assert_eq!(resource.resource_type, "warehouse");
    let edges = resource.edge_recs();
    assert!(
        edges.iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }),
        "expected edge source={source_id} strategy={strategy}; edges={edges:#?}"
    );
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
