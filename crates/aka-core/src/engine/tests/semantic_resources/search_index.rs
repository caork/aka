use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_python_search_index_resources() {
    let repo = temp_repo("python-search-index-resources");
    std::fs::write(
        repo.join("search_ops.py"),
        r#"from elasticsearch import Elasticsearch, helpers

es = Elasticsearch("http://localhost:9200")

def search_orders(customer_id):
    return es.search(index="orders-v1", query={"term": {"customer_id": customer_id}})

def write_order(order):
    es.index(index="orders-v1", id=order["id"], document=order)
    es.delete(index="orders-deadletter", id=order["id"])
    helpers.bulk(es, [{"_index": "orders-bulk", "_id": order["id"], "_source": order}])
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "search_orders",
        "search_ops.search_orders",
        "search_ops.py",
        (5, 6),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "write_order",
        "search_ops.write_order",
        "search_ops.py",
        (8, 11),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (url, source_id, strategy) in [
        (
            "search-index:orders-v1",
            "cbm:1:search_ops.search_orders",
            "python-search-index-search",
        ),
        (
            "search-index:orders-v1",
            "cbm:2:search_ops.write_order",
            "python-search-index-index",
        ),
        (
            "search-index:orders-deadletter",
            "cbm:2:search_ops.write_order",
            "python-search-index-delete",
        ),
        (
            "search-index:orders-bulk",
            "cbm:2:search_ops.write_order",
            "python-search-index-bulk",
        ),
    ] {
        let resource = synth
            .resources
            .iter()
            .find(|resource| resource.url == url)
            .unwrap_or_else(|| panic!("expected search index resource {url}"));
        assert_eq!(resource.resource_type, "search-index");
        assert!(resource.edge_recs().iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }));
    }
}

#[test]
fn synthesizes_java_search_index_resources() {
    let repo = temp_repo("java-search-index-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/search")).unwrap();
    let file = "src/main/java/com/example/search/OrderSearch.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.search;

import co.elastic.clients.elasticsearch.ElasticsearchClient;

class OrderSearch {
    private final ElasticsearchClient client;

    OrderSearch(ElasticsearchClient client) {
        this.client = client;
    }

    public Object searchOrders(String id) throws Exception {
        return client.search(s -> s.index("orders-v1").query(q -> q), Object.class);
    }

    public void writeOrder(Object order) throws Exception {
        client.index(i -> i.index("orders-v1").id("42").document(order));
        client.delete(d -> d.index("orders-deadletter").id("42"));
        client.bulk(b -> b.operations(op -> op.index(i -> i.index("orders-bulk").document(order))));
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "searchOrders",
        "com.example.search.OrderSearch.searchOrders",
        file,
        (12, 14),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "writeOrder",
        "com.example.search.OrderSearch.writeOrder",
        file,
        (16, 20),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (url, source_id, strategy) in [
        (
            "search-index:orders-v1",
            "cbm:1:com.example.search.OrderSearch.searchOrders",
            "java-search-index-search",
        ),
        (
            "search-index:orders-v1",
            "cbm:2:com.example.search.OrderSearch.writeOrder",
            "java-search-index-index",
        ),
        (
            "search-index:orders-deadletter",
            "cbm:2:com.example.search.OrderSearch.writeOrder",
            "java-search-index-delete",
        ),
        (
            "search-index:orders-bulk",
            "cbm:2:com.example.search.OrderSearch.writeOrder",
            "java-search-index-bulk",
        ),
    ] {
        let resource = synth
            .resources
            .iter()
            .find(|resource| resource.url == url)
            .unwrap_or_else(|| panic!("expected search index resource {url}"));
        assert_eq!(resource.resource_type, "search-index");
        assert!(resource.edge_recs().iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }));
    }
}
