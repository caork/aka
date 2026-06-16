use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_search_index_resources() {
    let repo = temp_repo("configured-search-index-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"elasticsearch:
  orders:
    index: orders-v1
opensearch:
  indices: orders-read,orders-write
solr:
  collection: products
dynamic:
  search:
    index: ${SEARCH_INDEX}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"MEILISEARCH_INDEX = "catalog"
TYPESENSE_COLLECTION = "products-v2"
ALGOLIA_INDEX_NAME = "orders_algolia"
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 10),
        json!({"language": "yaml"}),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Config", "settings.py", "settings.py", "settings.py"),
        (1, 3),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_search_config_edge(
        &synth,
        "search-index:orders-v1",
        &config_id("elasticsearch.orders.index"),
        "elasticsearch-config-index",
    );
    assert_search_config_edge(
        &synth,
        "search-index:orders-read",
        &config_id("opensearch.indices"),
        "opensearch-config-index",
    );
    assert_search_config_edge(
        &synth,
        "search-index:orders-write",
        &config_id("opensearch.indices"),
        "opensearch-config-index",
    );
    assert_search_config_edge(
        &synth,
        "search-index:products",
        &config_id("solr.collection"),
        "solr-config-core",
    );
    assert_search_config_edge(
        &synth,
        "search-index:catalog",
        &config_id("meilisearch.index"),
        "meilisearch-config-index",
    );
    assert_search_config_edge(
        &synth,
        "search-index:products-v2",
        &config_id("typesense.collection"),
        "typesense-config-collection",
    );
    assert_search_config_edge(
        &synth,
        "search-index:orders_algolia",
        &config_id("algolia.index.name"),
        "algolia-config-index",
    );
    assert!(synth
        .resources
        .iter()
        .all(|resource| resource.url != "search-index:${SEARCH_INDEX}"));
}

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

fn assert_search_config_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected search config resource {url}"));
    assert_eq!(resource.resource_type, "search-index");
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == source_id
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == strategy
    }));
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}

#[test]
fn synthesizes_python_search_index_dsl_resources() {
    let repo = temp_repo("python-search-index-dsl-resources");
    std::fs::write(
        repo.join("documents.py"),
        r#"from elasticsearch_dsl import Document, Index, Search

orders_admin = Index("orders-admin")

class OrderDocument(Document):
    class Index:
        name = "orders-documents"

def find_orders(customer_id):
    return Search(index="orders-v1").query("term", customer_id=customer_id).execute()

def rebuild_admin():
    orders_admin.create()
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
            "documents.OrderDocument",
            "documents.py",
        ),
        (5, 7),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "find_orders",
        "documents.find_orders",
        "documents.py",
        (9, 10),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "rebuild_admin",
        "documents.rebuild_admin",
        "documents.py",
        (12, 13),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    for (url, source_id, strategy) in [
        (
            "search-index:orders-documents",
            "cbm:1:documents.OrderDocument",
            "python-search-index-dsl-document",
        ),
        (
            "search-index:orders-v1",
            "cbm:2:documents.find_orders",
            "python-search-index-dsl-search",
        ),
        (
            "search-index:orders-admin",
            "cbm:3:documents.rebuild_admin",
            "python-search-index-dsl-index",
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

#[test]
fn synthesizes_java_search_request_resources() {
    let repo = temp_repo("java-search-request-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/search")).unwrap();
    let file = "src/main/java/com/example/search/OrderSearchRequests.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.search;

import co.elastic.clients.elasticsearch.ElasticsearchClient;
import co.elastic.clients.elasticsearch.core.SearchRequest;

class OrderSearchRequests {
    private final ElasticsearchClient client;

    OrderSearchRequests(ElasticsearchClient client) {
        this.client = client;
    }

    public Object searchOrders() throws Exception {
        SearchRequest request = SearchRequest.of(s -> s.index("orders-request"));
        return client.search(request, Object.class);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "searchOrders",
        "com.example.search.OrderSearchRequests.searchOrders",
        file,
        (13, 16),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == "search-index:orders-request")
        .unwrap_or_else(|| panic!("expected search index resource orders-request"));
    assert_eq!(resource.resource_type, "search-index");
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == "cbm:1:com.example.search.OrderSearchRequests.searchOrders"
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == "java-search-index-request"
    }));
}
