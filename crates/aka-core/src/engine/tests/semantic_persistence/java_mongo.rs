use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_spring_data_mongo_documents_and_repositories() {
    let repo = temp_repo("java-mongo-persistence");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let document_file = "src/main/java/com/example/orders/OrderDocument.java";
    let repo_file = "src/main/java/com/example/orders/OrderDocumentRepository.java";
    std::fs::write(
        repo.join(document_file),
        r#"package com.example.orders;

import org.springframework.data.mongodb.core.mapping.Document;
import org.springframework.data.mongodb.core.mapping.Field;

@Document(collection = "orders")
class OrderDocument {
    @Field("status")
    private String status;

    @Field(name = "customer_id")
    private String customerId;
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(repo_file),
        r#"package com.example.orders;

import org.springframework.data.mongodb.repository.MongoRepository;

interface OrderDocumentRepository extends MongoRepository<OrderDocument, String> {
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderDocument",
            "com.example.orders.OrderDocument",
            document_file,
        ),
        (7, 14),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Interface",
            "OrderDocumentRepository",
            "com.example.orders.OrderDocumentRepository",
            repo_file,
        ),
        (5, 6),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders collection table node");
    assert_eq!(
        table.properties.get("tableSource").and_then(Value::as_str),
        Some("java-spring-data-mongo-document")
    );
    let columns: Vec<_> = table
        .properties
        .get("columns")
        .and_then(Value::as_array)
        .expect("mongo fields")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(columns, ["customer_id", "status"]);

    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "MAPS_TO_TABLE"
            && edge.source_id == "cbm:1:com.example.orders.OrderDocument"
            && edge.target_id == table.id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "MANAGES_ENTITY"
            && edge.source_id.starts_with("repository:heuristic:")
            && edge.target_id == "cbm:1:com.example.orders.OrderDocument"
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "REPOSITORY_FOR"
            && edge.source_id.starts_with("repository:heuristic:")
            && edge.target_id == table.id
    }));
}

#[test]
fn synthesizes_java_mongo_template_table_access_edges() {
    let repo = temp_repo("java-mongo-template-table-access");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let document_file = "src/main/java/com/example/orders/OrderDocument.java";
    let service_file = "src/main/java/com/example/orders/OrderService.java";
    std::fs::write(
        repo.join(document_file),
        r#"package com.example.orders;

import org.springframework.data.mongodb.core.mapping.Document;

@Document(collection = "orders")
class OrderDocument {}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(service_file),
        r#"package com.example.orders;

import org.springframework.data.mongodb.core.MongoTemplate;
import org.springframework.data.mongodb.core.query.Query;
import org.springframework.data.mongodb.core.query.Update;

class OrderService {
    private final MongoTemplate mongoTemplate;

    OrderService(MongoTemplate mongoTemplate) {
        this.mongoTemplate = mongoTemplate;
    }

    java.util.List<OrderDocument> findOpen(Query query) {
        return mongoTemplate.find(query, OrderDocument.class);
    }

    void save(OrderDocument order) {
        mongoTemplate.save(order, "orders");
    }

    void markShipped(Query query) {
        mongoTemplate.updateFirst(query, new Update().set("status", "SHIPPED"), OrderDocument.class);
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
            "Class",
            "OrderDocument",
            "com.example.orders.OrderDocument",
            document_file,
        ),
        (6, 6),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "findOpen",
        "com.example.orders.OrderService.findOpen",
        service_file,
        (14, 16),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "save",
        "com.example.orders.OrderService.save",
        service_file,
        (18, 20),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "markShipped",
        "com.example.orders.OrderService.markShipped",
        service_file,
        (22, 24),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
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
        .expect("orders collection table")
        .id;
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:2:com.example.orders.OrderService.findOpen"
            && edge.target_id == table_id
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("java-mongo-template-read")
    }));
    for writer in [
        "cbm:3:com.example.orders.OrderService.save",
        "cbm:4:com.example.orders.OrderService.markShipped",
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
                        == Some("java-mongo-template-write")
            }),
            "expected {writer} to write orders through MongoTemplate"
        );
    }
}
