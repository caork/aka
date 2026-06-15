use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_python_async_db_table_access_from_multiline_sql() {
    let repo = temp_repo("python-async-db-table-access");
    std::fs::write(
        repo.join("models.py"),
        r#"from sqlalchemy import Column, Integer

class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True)

class Payment(Base):
    __tablename__ = "payments"
    id = Column(Integer, primary_key=True)
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("store.py"),
        r#"async def load_recent_orders(conn):
    return await conn.fetch("""
        SELECT o.id, p.id
        FROM orders o
        JOIN payments p ON p.order_id = o.id
        WHERE o.status = 'paid'
    """)

async def cancel_order(conn, order_id):
    await conn.execute("""
        UPDATE orders
        SET status = 'cancelled'
        WHERE id = $1
    """, order_id)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "models.Order", "models.py"),
        (3, 5),
        json!({
            "language": "python",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Class", "Payment", "models.Payment", "models.py"),
        (7, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "load_recent_orders",
        "store.load_recent_orders",
        "store.py",
        (1, 7),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "cancel_order",
        "store.cancel_order",
        "store.py",
        (9, 14),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let nodes = synth.persistence.node_recs();
    let orders_id = nodes
        .iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id
        .clone();
    let payments_id = nodes
        .iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("payments")
        })
        .expect("payments table")
        .id
        .clone();
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:3:store.load_recent_orders"
            && edge.target_id == orders_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:3:store.load_recent_orders"
            && edge.target_id == payments_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:4:store.cancel_order"
            && edge.target_id == orders_id
    }));
}
