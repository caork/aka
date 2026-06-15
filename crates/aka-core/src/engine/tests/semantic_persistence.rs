use super::*;
use serde_json::json;

#[test]
fn synthesizes_java_persistence_tables_and_repositories() {
    let repo = temp_repo("java-persistence");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/Order.java"),
        r#"package com.example.orders;

import jakarta.persistence.Column;
import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {
    @Column(name = "status")
    String status;
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderRepository.java"),
        r#"package com.example.orders;

import org.springframework.data.jpa.repository.JpaRepository;

interface OrderRepository extends JpaRepository<Order, Long> {
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "Order",
            "com.example.orders.Order",
            "src/main/java/com/example/orders/Order.java",
        ),
        (8, 12),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Interface",
            "OrderRepository",
            "com.example.orders.OrderRepository",
            "src/main/java/com/example/orders/OrderRepository.java",
        ),
        (5, 6),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let nodes = synth.persistence.node_recs();
    assert!(nodes.iter().any(|node| {
        node.label == "Table"
            && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
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
    assert!(edge_types.contains(&"REPOSITORY_FOR".to_string()));
}

#[test]
fn synthesizes_java_table_access_edges() {
    let repo = temp_repo("java-table-access");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let entity_file = "src/main/java/com/example/orders/Order.java";
    let repo_file = "src/main/java/com/example/orders/OrderRepository.java";
    let service_file = "src/main/java/com/example/orders/OrderService.java";
    std::fs::write(
        repo.join(entity_file),
        r#"package com.example.orders;

import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(repo_file),
        r#"package com.example.orders;

import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;

interface OrderRepository extends JpaRepository<Order, Long> {
    @Query(value = "select * from orders where status = ?1", nativeQuery = true)
    List<Order> findNative(String status);
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(service_file),
        r#"package com.example.orders;

class OrderService {
    void cancelOrders(EntityManager em) {
        em.createNativeQuery("update orders set status = 'CANCELLED'");
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "com.example.orders.Order", entity_file),
        (6, 9),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "findNative",
        "com.example.orders.OrderRepository.findNative",
        repo_file,
        (7, 8),
        json!({
            "decorators": ["@Query(value = \"select * from orders where status = ?1\", nativeQuery = true)"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "cancelOrders",
        "com.example.orders.OrderService.cancelOrders",
        service_file,
        (4, 6),
        json!({
            "language": "java",
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
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:2:com.example.orders.OrderRepository.findNative"
            && edge.target_id == table_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:3:com.example.orders.OrderService.cancelOrders"
            && edge.target_id == table_id
    }));
}

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
fn synthesizes_python_django_orm_write_table_access_edges() {
    let repo = temp_repo("python-django-orm-table-access");
    std::fs::write(
        repo.join("models.py"),
        r#"from django.db import models

class Order(models.Model):
    status = models.CharField(max_length=32)

    class Meta:
        db_table = "orders"
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("services.py"),
        r#"from models import Order

def load_order(order_id):
    return Order.objects.get(id=order_id)

def create_order(payload):
    return Order.objects.create(**payload)

def cancel_orders(customer_id):
    return Order.objects.filter(customer_id=customer_id).update(status="cancelled")

def purge_cancelled():
    return Order.objects.filter(status="cancelled").delete()
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "models.Order", "models.py"),
        (3, 7),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_order",
        "services.load_order",
        "services.py",
        (3, 4),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "create_order",
        "services.create_order",
        "services.py",
        (6, 7),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "cancel_orders",
        "services.cancel_orders",
        "services.py",
        (9, 10),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "purge_cancelled",
        "services.purge_cancelled",
        "services.py",
        (12, 13),
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
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:2:services.load_order"
            && edge.target_id == table_id
    }));
    for writer in [
        "cbm:3:services.create_order",
        "cbm:4:services.cancel_orders",
        "cbm:5:services.purge_cancelled",
    ] {
        assert!(
            edges.iter().any(|edge| {
                edge.edge_type == "WRITES_TABLE"
                    && edge.source_id == writer
                    && edge.target_id == table_id
            }),
            "expected {writer} to write orders"
        );
    }
}

#[test]
fn synthesizes_python_django_instance_write_table_access_edges() {
    let repo = temp_repo("python-django-instance-table-access");
    std::fs::write(
        repo.join("models.py"),
        r#"from django.db import models

class Order(models.Model):
    status = models.CharField(max_length=32)

    class Meta:
        db_table = "orders"
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("services.py"),
        r#"from models import Order

def create_order(payload):
    order = Order(**payload)
    order.save()
    return order

def cancel_order(order_id):
    order = Order.objects.get(id=order_id)
    order.status = "cancelled"
    order.save(update_fields=["status"])
    return order

def purge_order(order_id):
    order = Order.objects.get(id=order_id)
    order.delete()

async def async_create(payload):
    order = Order(**payload)
    await order.asave()
    return order
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "models.Order", "models.py"),
        (3, 7),
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
        (3, 6),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "cancel_order",
        "services.cancel_order",
        "services.py",
        (8, 12),
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
        (14, 16),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "async_create",
        "services.async_create",
        "services.py",
        (18, 21),
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
        "cbm:3:services.cancel_order",
        "cbm:4:services.purge_order",
        "cbm:5:services.async_create",
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
                        == Some("python-orm-instance-write")
            }),
            "expected {writer} to write orders through a Django model instance"
        );
    }
}

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
