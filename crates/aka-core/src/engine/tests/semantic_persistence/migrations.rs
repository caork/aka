use super::super::*;

#[test]
fn synthesizes_java_migration_tables() {
    let repo = temp_repo("java-migrations");
    std::fs::create_dir_all(repo.join("src/main/resources/db/migration")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/resources/db/changelog")).unwrap();
    std::fs::write(
        repo.join("src/main/resources/db/migration/V1__create_orders.sql"),
        r#"CREATE TABLE orders (
    id bigint primary key,
    status varchar(32)
);

ALTER TABLE order_items ADD COLUMN sku varchar(64);
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/resources/db/changelog/changelog.yaml"),
        r#"- changeSet:
    id: 2
    author: aka
    changes:
      - createTable:
          tableName: invoices
"#,
    )
    .unwrap();

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_names: BTreeSet<_> = synth
        .persistence
        .node_recs()
        .into_iter()
        .filter(|node| node.label == "Table")
        .filter_map(|node| {
            node.properties
                .get("tableName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(table_names.contains("orders"));
    assert!(table_names.contains("order_items"));
    assert!(table_names.contains("invoices"));

    let migrations: Vec<_> = synth
        .persistence
        .node_recs()
        .into_iter()
        .filter(|node| node.label == "Migration")
        .collect();
    assert_eq!(migrations.len(), 2);
    assert!(migrations
        .iter()
        .any(|node| node.properties["version"] == "1"));
    let edge_types: Vec<_> = synth
        .persistence
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"MIGRATES_TABLE".to_string()));
}

#[test]
fn synthesizes_python_migration_tables() {
    let repo = temp_repo("python-migrations");
    std::fs::create_dir_all(repo.join("alembic/versions")).unwrap();
    std::fs::create_dir_all(repo.join("orders/migrations")).unwrap();
    std::fs::write(
        repo.join("alembic/versions/20260615_create_shipments.py"),
        r#"from alembic import op
import sqlalchemy as sa

def upgrade():
    op.create_table("shipments", sa.Column("id", sa.Integer()))
    op.add_column("orders", sa.Column("shipped_at", sa.DateTime()))
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/migrations/0002_invoice.py"),
        r#"from django.db import migrations

class Migration(migrations.Migration):
    operations = [
        migrations.CreateModel(name="Invoice", fields=[]),
        migrations.RunSQL("ALTER TABLE orders ADD COLUMN invoice_id integer"),
    ]
"#,
    )
    .unwrap();

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_names: BTreeSet<_> = synth
        .persistence
        .node_recs()
        .into_iter()
        .filter(|node| node.label == "Table")
        .filter_map(|node| {
            node.properties
                .get("tableName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(table_names.contains("shipments"));
    assert!(table_names.contains("orders"));
    assert!(table_names.contains("invoice"));
    let migration_hits = synth
        .persistence
        .node_recs()
        .into_iter()
        .filter(|node| node.label == "Migration")
        .count();
    assert_eq!(migration_hits, 2);
}
