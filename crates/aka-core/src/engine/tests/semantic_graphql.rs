use super::*;
use serde_json::json;

#[test]
fn synthesizes_java_graphql_operations() {
    let repo = temp_repo("java-graphql");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/graphql")).unwrap();
    let file = "src/main/java/com/example/graphql/OrderGraphql.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.graphql;

import org.springframework.graphql.data.method.annotation.Argument;
import org.springframework.graphql.data.method.annotation.MutationMapping;
import org.springframework.graphql.data.method.annotation.QueryMapping;

class OrderGraphql {
    @QueryMapping(name = "order")
    Order order(@Argument String id) {
        return service.find(id);
    }

    @MutationMapping("createOrder")
    Order create(OrderInput input) {
        return service.create(input);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "order",
        "com.example.graphql.OrderGraphql.order",
        file,
        (8, 10),
        json!({
            "language": "java",
            "decorators": ["@QueryMapping(name = \"order\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "create",
        "com.example.graphql.OrderGraphql.create",
        file,
        (13, 15),
        json!({
            "language": "java",
            "decorators": ["@MutationMapping(\"createOrder\")"],
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let query = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "order")
        .expect("query operation");
    assert_eq!(query.operation_type, "query");
    assert_eq!(
        query.handler_id,
        "cbm:1:com.example.graphql.OrderGraphql.order"
    );
    let mutation = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "createOrder")
        .expect("mutation operation");
    assert_eq!(mutation.operation_type, "mutation");
    let edge_types: Vec<_> = synth
        .graphql
        .iter()
        .flat_map(SynthGraphqlOperation::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"HANDLES_GRAPHQL".to_string()));
}

#[test]
fn synthesizes_java_graphql_operations_from_source_annotations_without_metadata() {
    let repo = temp_repo("java-graphql-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/graphql")).unwrap();
    let file = "src/main/java/com/example/graphql/OrderGraphql.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.graphql;

import org.springframework.graphql.data.method.annotation.MutationMapping;
import org.springframework.graphql.data.method.annotation.QueryMapping;

class OrderGraphql {
    @QueryMapping(
        name = "order")
    Order order(String id) {
        return service.find(id);
    }

    @MutationMapping("createOrder")
    Order create(OrderInput input) {
        return service.create(input);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "order",
        "com.example.graphql.OrderGraphql.order",
        file,
        (9, 11),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "create",
        "com.example.graphql.OrderGraphql.create",
        file,
        (14, 16),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let query = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "order")
        .expect("source annotation query operation");
    assert_eq!(query.operation_type, "query");
    assert_eq!(
        query.handler_id,
        "cbm:1:com.example.graphql.OrderGraphql.order"
    );
    let mutation = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "createOrder")
        .expect("source annotation mutation operation");
    assert_eq!(mutation.operation_type, "mutation");
    assert!(mutation.edge_recs().iter().any(|edge| edge.source_id
        == "cbm:2:com.example.graphql.OrderGraphql.create"
        && edge.edge_type == "HANDLES_GRAPHQL"));
}

#[test]
fn synthesizes_python_graphql_operations() {
    let repo = temp_repo("python-graphql");
    std::fs::write(
        repo.join("schema.py"),
        r#"import strawberry

@strawberry.type
class Query:
    @strawberry.field(name="order")
    def resolve_order(self, id: str):
        return get_order(id)

@strawberry.type
class Mutation:
    @strawberry.mutation
    def create_order(self, name: str):
        return create_order(name)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "resolve_order",
        "schema.Query.resolve_order",
        "schema.py",
        (5, 7),
        json!({
            "language": "python",
            "decorators": ["@strawberry.field(name=\"order\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "create_order",
        "schema.Mutation.create_order",
        "schema.py",
        (11, 13),
        json!({
            "language": "python",
            "decorators": ["@strawberry.mutation"],
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let query = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "order")
        .expect("strawberry query operation");
    assert_eq!(query.operation_type, "query");
    let mutation = synth
        .graphql
        .iter()
        .find(|operation| operation.name == "create_order")
        .expect("strawberry mutation operation");
    assert_eq!(mutation.operation_type, "mutation");
}
