use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_vector_store_resources() {
    let repo = temp_repo("configured-vector-store-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"spring:
  ai:
    vectorstore:
      qdrant:
        host: localhost
      pinecone:
        api-key: pc-redacted
weaviate:
  url: http://weaviate:8080
chroma:
  path: ./chroma
milvus:
  uri: http://milvus:19530
disabled:
  qdrant:
    api-key: ${QDRANT_API_KEY}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"PINECONE_INDEX = "orders"
ZILLIZ_CLOUD_URI = "https://zilliz.example"
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 16),
        json!({"language": "yaml"}),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Config", "settings.py", "settings.py", "settings.py"),
        (1, 2),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_vector_store_edge(
        &synth,
        "vector-store:qdrant",
        &config_id("spring.ai.vectorstore.qdrant.host"),
        "qdrant-config",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:pinecone",
        &config_id("spring.ai.vectorstore.pinecone.api.key"),
        "pinecone-config",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:weaviate",
        &config_id("weaviate.url"),
        "weaviate-config",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:chroma",
        &config_id("chroma.path"),
        "chroma-config",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:milvus",
        &config_id("milvus.uri"),
        "milvus-config",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:pinecone",
        &config_id("pinecone.index"),
        "pinecone-config",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:milvus",
        &config_id("zilliz.cloud.uri"),
        "milvus-config",
    );
    assert!(!synth.resources.iter().any(|resource| {
        resource.url == "vector-store:qdrant"
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("disabled.qdrant.api.key"))
    }));
}

#[test]
fn synthesizes_python_vector_store_resources() {
    let repo = temp_repo("python-vector-store-resources");
    std::fs::write(
        repo.join("rag.py"),
        r#"from pinecone import Pinecone
from qdrant_client import QdrantClient
import chromadb
from pymilvus import MilvusClient

pc = Pinecone(api_key="redacted")
pinecone_index = pc.Index("orders")
qdrant = QdrantClient(url="http://localhost:6333")
chroma = chromadb.PersistentClient(path="./chroma")
collection = chroma.get_collection("orders")
milvus = MilvusClient(uri="http://localhost:19530")

def write_pinecone(vectors):
    pinecone_index.upsert(vectors=vectors)

def query_qdrant(vector):
    return qdrant.query_points(collection_name="orders", query=vector)

def write_chroma(ids, embeddings):
    return collection.add(ids=ids, embeddings=embeddings)

def search_milvus(vector):
    return milvus.search(collection_name="orders", data=[vector])

def ordinary(index, store):
    index.upsert(vectors=[])
    store.add([])
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "write_pinecone",
        "rag.write_pinecone",
        "rag.py",
        (13, 14),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "query_qdrant",
        "rag.query_qdrant",
        "rag.py",
        (16, 17),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "write_chroma",
        "rag.write_chroma",
        "rag.py",
        (19, 20),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "search_milvus",
        "rag.search_milvus",
        "rag.py",
        (22, 23),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "ordinary",
        "rag.ordinary",
        "rag.py",
        (25, 27),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_vector_store_edge(
        &synth,
        "vector-store:pinecone",
        "cbm:1:rag.write_pinecone",
        "python-pinecone-upsert",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:qdrant",
        "cbm:2:rag.query_qdrant",
        "python-qdrant-query-points",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:chroma",
        "cbm:3:rag.write_chroma",
        "python-chroma-add",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:milvus",
        "cbm:4:rag.search_milvus",
        "python-milvus-search",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:5:rag.ordinary"));
}

#[test]
fn synthesizes_java_vector_store_resources() {
    let repo = temp_repo("java-vector-store-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/rag")).unwrap();
    let file = "src/main/java/com/example/rag/RagStore.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.rag;

import dev.langchain4j.store.embedding.EmbeddingStore;
import io.milvus.v2.client.MilvusClient;
import org.springframework.ai.vectorstore.VectorStore;
import tech.qdrant.client.QdrantClient;

class RagStore {
    void save(VectorStore vectorStore, java.util.List<?> docs) {
        vectorStore.add(docs);
    }

    Object find(VectorStore vectorStore, Object request) {
        return vectorStore.similaritySearch(request);
    }

    Object qdrant(QdrantClient client, Object points) {
        return client.upsert(points);
    }

    Object milvus(MilvusClient milvusClient, Object request) {
        return milvusClient.search(request);
    }

    void ordinary(Store store, Object docs) {
        store.add(docs);
        store.search(docs);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "save",
        "com.example.rag.RagStore.save",
        file,
        (9, 11),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "find",
        "com.example.rag.RagStore.find",
        file,
        (13, 15),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "qdrant",
        "com.example.rag.RagStore.qdrant",
        file,
        (17, 19),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "milvus",
        "com.example.rag.RagStore.milvus",
        file,
        (21, 23),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "ordinary",
        "com.example.rag.RagStore.ordinary",
        file,
        (25, 28),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_vector_store_edge(
        &synth,
        "vector-store:generic",
        "cbm:1:com.example.rag.RagStore.save",
        "java-vectorstore-add",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:generic",
        "cbm:2:com.example.rag.RagStore.find",
        "java-vectorstore-similarity-search",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:qdrant",
        "cbm:3:com.example.rag.RagStore.qdrant",
        "java-qdrant-upsert",
    );
    assert_vector_store_edge(
        &synth,
        "vector-store:milvus",
        "cbm:4:com.example.rag.RagStore.milvus",
        "java-milvus-search",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:5:com.example.rag.RagStore.ordinary"));
}

fn assert_vector_store_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected vector store resource {url}"));
    assert_eq!(resource.resource_type, "vector-store");
    let edges = resource.edge_recs();
    assert!(
        edges.iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }),
        "expected edge source={source_id} strategy={strategy}; edges={edges:#?}"
    );
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
