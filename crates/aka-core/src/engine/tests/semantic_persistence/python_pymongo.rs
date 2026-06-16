use super::super::*;

#[test]
fn synthesizes_python_pymongo_collections_and_table_access_edges() {
    let repo = temp_repo("python-pymongo-table-access");
    std::fs::write(
        repo.join("services.py"),
        r#"from motor.motor_asyncio import AsyncIOMotorClient
from pymongo import MongoClient

client = MongoClient()
db = client["orders_db"]
async_db = AsyncIOMotorClient().orders_db

def load_orders(status):
    return list(db["orders"].find({"status": status}))

def create_order(payload):
    return db.orders.insert_one(payload)

async def mark_shipped(order_id):
    return await async_db.get_collection("orders").update_one({"_id": order_id}, {"$set": {"status": "SHIPPED"}})

def purge_audit(before):
    return mongo_db.audit_events.delete_many({"created_at": {"$lt": before}})
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "load_orders",
        "services.load_orders",
        "services.py",
        (8, 9),
        serde_json::json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "create_order",
        "services.create_order",
        "services.py",
        (11, 12),
        serde_json::json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "mark_shipped",
        "services.mark_shipped",
        "services.py",
        (14, 15),
        serde_json::json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "purge_audit",
        "services.purge_audit",
        "services.py",
        (17, 18),
        serde_json::json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let nodes = synth.persistence.node_recs();
    let orders_table = nodes
        .iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
                && node.properties.get("tableSource").and_then(Value::as_str)
                    == Some("python-pymongo-collection")
        })
        .expect("orders PyMongo collection table");
    let audit_table = nodes
        .iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("audit_events")
                && node.properties.get("tableSource").and_then(Value::as_str)
                    == Some("python-pymongo-collection")
        })
        .expect("audit_events PyMongo collection table");

    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:1:services.load_orders"
            && edge.target_id == orders_table.id
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("python-pymongo-read")
    }));
    for writer in ["cbm:2:services.create_order", "cbm:3:services.mark_shipped"] {
        assert!(
            edges.iter().any(|edge| {
                edge.edge_type == "WRITES_TABLE"
                    && edge.source_id == writer
                    && edge.target_id == orders_table.id
                    && edge
                        .evidence
                        .as_ref()
                        .and_then(|v| v.get("strategy"))
                        .and_then(Value::as_str)
                        == Some("python-pymongo-write")
            }),
            "expected {writer} to write orders through PyMongo/Motor"
        );
    }
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:4:services.purge_audit"
            && edge.target_id == audit_table.id
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("python-pymongo-write")
    }));
}
