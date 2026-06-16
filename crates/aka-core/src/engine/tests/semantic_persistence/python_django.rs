use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_python_django_default_table_name() {
    let repo = temp_repo("python-django-default-table-name");
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(repo.join("orders/__init__.py"), "").unwrap();
    std::fs::write(
        repo.join("orders/models.py"),
        r#"from django.db import models

class Order(models.Model):
    status = models.CharField(max_length=32)
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/services.py"),
        r#"from .models import Order

def load_order(order_id):
    return Order.objects.get(id=order_id)

def create_order(payload):
    return Order.objects.create(**payload)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "orders.models.Order", "orders/models.py"),
        (3, 4),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_order",
        "orders.services.load_order",
        "orders/services.py",
        (3, 4),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "create_order",
        "orders.services.create_order",
        "orders/services.py",
        (6, 7),
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
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders_order")
        })
        .expect("orders_order table")
        .id;
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:2:orders.services.load_order"
            && edge.target_id == table_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:3:orders.services.create_order"
            && edge.target_id == table_id
    }));
}

#[test]
fn synthesizes_python_django_default_table_name_from_models_package() {
    let repo = temp_repo("python-django-default-table-name-models-package");
    std::fs::create_dir_all(repo.join("orders/models")).unwrap();
    std::fs::write(repo.join("orders/__init__.py"), "").unwrap();
    std::fs::write(repo.join("orders/models/__init__.py"), "").unwrap();
    std::fs::write(
        repo.join("orders/models/order_item.py"),
        r#"from django.db.models import Model

class OrderItem(Model):
    sku = "demo"
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderItem",
            "orders.models.order_item.OrderItem",
            "orders/models/order_item.py",
        ),
        (3, 4),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.persistence.node_recs().into_iter().any(|node| {
        node.label == "Table"
            && node.properties.get("tableName").and_then(Value::as_str) == Some("orders_order_item")
    }));
}

#[test]
fn synthesizes_python_django_columns_and_relationships() {
    let repo = temp_repo("python-django-columns-relationships");
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(repo.join("orders/__init__.py"), "").unwrap();
    std::fs::write(
        repo.join("orders/models.py"),
        r#"from django.db import models

class Customer(models.Model):
    email = models.EmailField(unique=True)

class Tag(models.Model):
    name = models.CharField(max_length=64)

class Order(models.Model):
    status = models.CharField(max_length=32)
    total_cents = models.IntegerField()
    customer = models.ForeignKey(Customer, on_delete=models.CASCADE)
    tags = models.ManyToManyField("Tag")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "Customer",
            "orders.models.Customer",
            "orders/models.py",
        ),
        (3, 4),
        json!({
            "language": "python",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Class", "Tag", "orders.models.Tag", "orders/models.py"),
        (6, 7),
        json!({
            "language": "python",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        ("Class", "Order", "orders.models.Order", "orders/models.py"),
        (9, 13),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let order_table = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders_order")
        })
        .expect("orders_order table");
    let columns: Vec<_> = order_table
        .properties
        .get("columns")
        .and_then(Value::as_array)
        .expect("columns")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(columns.contains(&"status"));
    assert!(columns.contains(&"total_cents"));
    assert!(columns.contains(&"customer_id"));
    assert!(columns.contains(&"tags"));

    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "HAS_RELATION"
            && edge.source_id == "cbm:3:orders.models.Order"
            && edge.target_id == "cbm:1:orders.models.Customer"
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "HAS_RELATION"
            && edge.source_id == "cbm:3:orders.models.Order"
            && edge.target_id == "cbm:2:orders.models.Tag"
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
