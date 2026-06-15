use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_python_persistence_tables_repositories_and_relationships() {
    let repo = temp_repo("python-persistence");
    std::fs::write(
        repo.join("models.py"),
        r#"from sqlalchemy import Column, ForeignKey, Integer, String
from sqlalchemy.orm import relationship

class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True)
    customer_id = Column(ForeignKey("customers.id"))
    customer = relationship("Customer")

class Customer(Base):
    __tablename__ = "customers"
    id = Column(Integer, primary_key=True)

class OrderRepository:
    model = Order
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "models.Order", "models.py"),
        (4, 8),
        json!({
            "language": "python",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Class", "Customer", "models.Customer", "models.py"),
        (10, 12),
        json!({
            "language": "python",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Class",
            "OrderRepository",
            "models.OrderRepository",
            "models.py",
        ),
        (14, 15),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let nodes = synth.persistence.node_recs();
    assert!(nodes.iter().any(|node| {
        node.label == "Table"
            && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
    }));
    assert!(nodes.iter().any(|node| {
        node.label == "Table"
            && node.properties.get("tableName").and_then(Value::as_str) == Some("customers")
    }));
    assert!(nodes.iter().any(|node| {
        node.label == "Repository"
            && node.properties.get("entityName").and_then(Value::as_str) == Some("Order")
    }));
    let edge_types: Vec<_> = synth
        .persistence
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"MAPS_TO_TABLE".to_string()));
    assert!(edge_types.contains(&"MANAGES_ENTITY".to_string()));
    assert!(edge_types.contains(&"HAS_RELATION".to_string()));
}

#[test]
fn synthesizes_python_table_access_edges() {
    let repo = temp_repo("python-table-access");
    std::fs::write(
        repo.join("models.py"),
        r#"from sqlalchemy import Column, Integer

class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True)

class Customer(Base):
    __tablename__ = "customers"
    id = Column(Integer, primary_key=True)
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("services.py"),
        r#"from sqlalchemy import select, text
from models import Customer, Order

def load_orders(session):
    return session.query(Order).all()

def load_customers(session):
    return session.execute(select(Customer)).all()

def archive_orders(session):
    session.execute(text("update orders set archived = true"))
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
        ("Class", "Customer", "models.Customer", "models.py"),
        (7, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "load_orders",
        "services.load_orders",
        "services.py",
        (4, 5),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "load_customers",
        "services.load_customers",
        "services.py",
        (7, 8),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "archive_orders",
        "services.archive_orders",
        "services.py",
        (10, 11),
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
    let customers_id = nodes
        .iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("customers")
        })
        .expect("customers table")
        .id
        .clone();
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:3:services.load_orders"
            && edge.target_id == orders_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:4:services.load_customers"
            && edge.target_id == customers_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:5:services.archive_orders"
            && edge.target_id == orders_id
    }));
}

#[test]
fn synthesizes_python_sqlalchemy_session_write_table_access_edges() {
    let repo = temp_repo("python-sqlalchemy-session-writes");
    std::fs::write(
        repo.join("models.py"),
        r#"from sqlalchemy import Column, Integer

class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True)
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("services.py"),
        r#"from models import Order

def create_order(session, payload):
    session.add(Order(**payload))

async def upsert_order(session, payload):
    order = Order(**payload)
    await session.merge(order)

def purge_order(session, order_id):
    order = session.get(Order, order_id)
    session.delete(order)
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
    insert_function_node_props_at(
        &conn,
        2,
        "create_order",
        "services.create_order",
        "services.py",
        (3, 4),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "upsert_order",
        "services.upsert_order",
        "services.py",
        (6, 8),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "purge_order",
        "services.purge_order",
        "services.py",
        (10, 12),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_id = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id;
    let edges = synth.persistence.edge_recs();
    for writer in [
        "cbm:2:services.create_order",
        "cbm:3:services.upsert_order",
        "cbm:4:services.purge_order",
    ] {
        assert!(
            edges.iter().any(|edge| {
                edge.edge_type == "WRITES_TABLE"
                    && edge.source_id == writer
                    && edge.target_id == table_id
                    && edge
                        .evidence
                        .as_ref()
                        .and_then(|v| v.get("strategy"))
                        .and_then(Value::as_str)
                        == Some("python-sqlalchemy-session-write")
            }),
            "expected {writer} to write orders through SQLAlchemy session"
        );
    }
}

#[test]
fn synthesizes_python_sqlalchemy_core_write_table_access_edges() {
    let repo = temp_repo("python-sqlalchemy-core-writes");
    std::fs::write(
        repo.join("models.py"),
        r#"from sqlalchemy import Column, Integer

class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True)
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("services.py"),
        r#"from sqlalchemy import delete, insert, update
from models import Order

def bulk_create(session, rows):
    session.execute(insert(Order), rows)

async def mark_cancelled(session):
    await session.execute(update(Order).where(Order.status == "open").values(status="cancelled"))

def purge_cancelled(db):
    db.execute(delete(Order).where(Order.status == "cancelled"))
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
    insert_function_node_props_at(
        &conn,
        2,
        "bulk_create",
        "services.bulk_create",
        "services.py",
        (4, 5),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "mark_cancelled",
        "services.mark_cancelled",
        "services.py",
        (7, 8),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "purge_cancelled",
        "services.purge_cancelled",
        "services.py",
        (10, 11),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_id = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id;
    let edges = synth.persistence.edge_recs();
    for writer in [
        "cbm:2:services.bulk_create",
        "cbm:3:services.mark_cancelled",
        "cbm:4:services.purge_cancelled",
    ] {
        assert!(
            edges.iter().any(|edge| {
                edge.edge_type == "WRITES_TABLE"
                    && edge.source_id == writer
                    && edge.target_id == table_id
                    && edge
                        .evidence
                        .as_ref()
                        .and_then(|v| v.get("strategy"))
                        .and_then(Value::as_str)
                        == Some("python-sqlalchemy-core-write")
            }),
            "expected {writer} to write orders through SQLAlchemy Core"
        );
    }
}
