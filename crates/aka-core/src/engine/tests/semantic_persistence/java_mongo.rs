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
