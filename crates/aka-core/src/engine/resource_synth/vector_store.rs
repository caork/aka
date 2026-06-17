use super::{infra_config, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn extract_vector_store_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_vector_context(text) {
        out.extend(extract_python_vector_stores(text, nodes));
    }
    if has_java_vector_context(text) {
        out.extend(extract_java_vector_stores(text, nodes));
    }
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

pub(super) fn extract_vector_store_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some(provider) = vector_provider_for_config_key(&key) else {
            continue;
        };
        if !vector_config_value_is_present(&value) {
            continue;
        }
        out.push(ResourceDetection::vector_store(
            provider.into(),
            infra_config::config_id(&key),
            vector_config_strategy(provider),
        ));
    }
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn has_python_vector_context(text: &str) -> bool {
    text.contains("pinecone")
        || text.contains("Pinecone(")
        || text.contains("qdrant_client")
        || text.contains("QdrantClient")
        || text.contains("weaviate")
        || text.contains("chromadb")
        || text.contains("Chroma")
        || text.contains("MilvusClient")
        || text.contains("pymilvus")
}

fn extract_python_vector_stores(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "pinecone",
        &["Pinecone(", "pinecone.init("],
        &[
            (".upsert", "python-pinecone-upsert"),
            (".query", "python-pinecone-query"),
            (".delete", "python-pinecone-delete"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "qdrant",
        &["QdrantClient("],
        &[
            (".upsert", "python-qdrant-upsert"),
            (".search", "python-qdrant-search"),
            (".query_points", "python-qdrant-query-points"),
            (".delete", "python-qdrant-delete"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "weaviate",
        &["weaviate.connect", "weaviate.Client("],
        &[
            (".collections.get", "python-weaviate-collection"),
            (".query.near_vector", "python-weaviate-near-vector"),
            (".data.insert", "python-weaviate-insert"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "chroma",
        &["chromadb.Client(", "chromadb.PersistentClient(", "Chroma("],
        &[
            (".add", "python-chroma-add"),
            (".query", "python-chroma-query"),
            (".upsert", "python-chroma-upsert"),
            (".delete", "python-chroma-delete"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "milvus",
        &["MilvusClient(", "connections.connect("],
        &[
            (".insert", "python-milvus-insert"),
            (".search", "python-milvus-search"),
            (".upsert", "python-milvus-upsert"),
            (".delete", "python-milvus-delete"),
        ],
    ));
    out
}

fn extract_python_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    constructors: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for call in find_call_args(text, callee) {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !python_receiver_is_provider(text, receiver, provider, constructors) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::vector_store(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn has_java_vector_context(text: &str) -> bool {
    text.contains("PineconeClient")
        || text.contains("QdrantClient")
        || text.contains("WeaviateClient")
        || text.contains("ChromaApi")
        || text.contains("MilvusClient")
        || text.contains("dev.langchain4j.store.embedding")
        || text.contains("org.springframework.ai.vectorstore")
}

fn extract_java_vector_stores(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "pinecone",
        &["PineconeClient", "PineconeIndex"],
        &[
            (".upsert", "java-pinecone-upsert"),
            (".query", "java-pinecone-query"),
            (".delete", "java-pinecone-delete"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "qdrant",
        &["QdrantClient"],
        &[
            (".upsert", "java-qdrant-upsert"),
            (".search", "java-qdrant-search"),
            (".delete", "java-qdrant-delete"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "weaviate",
        &["WeaviateClient"],
        &[
            (".graphql", "java-weaviate-graphql"),
            (".batch", "java-weaviate-batch"),
            (".data", "java-weaviate-data"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "milvus",
        &["MilvusClient", "MilvusServiceClient"],
        &[
            (".insert", "java-milvus-insert"),
            (".search", "java-milvus-search"),
            (".upsert", "java-milvus-upsert"),
            (".delete", "java-milvus-delete"),
        ],
    ));
    out.extend(extract_java_vector_store_interface_calls(text, nodes));
    out
}

fn extract_java_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    types: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for call in find_call_args(text, callee) {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !java_receiver_has_type(text, receiver, types) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::vector_store(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn extract_java_vector_store_interface_calls(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if !(text.contains("VectorStore") || text.contains("EmbeddingStore")) {
        return out;
    }
    for (callee, strategy) in [
        (".add", "java-vectorstore-add"),
        (".similaritySearch", "java-vectorstore-similarity-search"),
        (".findRelevant", "java-vectorstore-find-relevant"),
        (".remove", "java-vectorstore-remove"),
    ] {
        for call in find_call_args(text, callee) {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !java_receiver_has_type(text, receiver, &["VectorStore", "EmbeddingStore"]) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::vector_store(
                "generic".into(),
                node.aka_id.clone(),
                strategy,
            ));
        }
    }
    out
}

fn python_receiver_is_provider(
    text: &str,
    receiver: &str,
    provider: &str,
    constructors: &[&str],
) -> bool {
    let receiver = receiver_tail(receiver);
    let receiver_lower = receiver.to_ascii_lowercase();
    receiver_lower.contains(provider)
        || python_receiver_assigned_to(text, receiver, constructors)
        || python_receiver_from_provider_collection(text, receiver, provider)
}

fn python_receiver_assigned_to(text: &str, receiver: &str, constructors: &[&str]) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            return false;
        };
        let lhs = lhs
            .trim()
            .trim_start_matches("self.")
            .trim_start_matches("cls.");
        lhs == receiver && constructors.iter().any(|ctor| rhs.contains(ctor))
    })
}

fn python_receiver_from_provider_collection(text: &str, receiver: &str, provider: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            return false;
        };
        let lhs = lhs
            .trim()
            .trim_start_matches("self.")
            .trim_start_matches("cls.");
        lhs == receiver
            && match provider {
                "pinecone" => rhs.contains(".Index(") || rhs.contains(".index("),
                "chroma" => rhs.contains(".get_collection(") || rhs.contains(".create_collection("),
                "weaviate" => rhs.contains(".collections.get("),
                _ => false,
            }
    })
}

fn java_receiver_has_type(text: &str, receiver: &str, types: &[&str]) -> bool {
    let receiver = receiver_tail(receiver);
    text.lines().any(|line| {
        let line = line.trim();
        types.iter().any(|ty| {
            line.contains(&format!("{ty} {receiver}"))
                || line.contains(&format!("{ty} {receiver},"))
                || line.contains(&format!("{ty} {receiver})"))
                || line.contains(&format!("{ty} {receiver} ="))
        })
    })
}

fn receiver_before_dot(text: &str, dot_start: usize) -> Option<&str> {
    if text.as_bytes().get(dot_start) != Some(&b'.') {
        return None;
    }
    let mut start = dot_start;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if ch == '.' || ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let receiver = text[start..dot_start].trim_matches('.');
    (!receiver.is_empty()).then_some(receiver)
}

fn receiver_tail(receiver: &str) -> &str {
    receiver.rsplit('.').next().unwrap_or(receiver)
}

fn vector_provider_for_config_key(key: &str) -> Option<&'static str> {
    if key_contains_any(key, &["pinecone"]) {
        Some("pinecone")
    } else if key_contains_any(key, &["qdrant"]) {
        Some("qdrant")
    } else if key_contains_any(key, &["weaviate"]) {
        Some("weaviate")
    } else if key_contains_any(key, &["chromadb", "chroma"]) {
        Some("chroma")
    } else if key_contains_any(key, &["milvus", "zilliz"]) {
        Some("milvus")
    } else if key_contains_any(key, &["vector.store", "vectorstore", "embedding.store"]) {
        Some("generic")
    } else {
        None
    }
}

fn vector_config_value_is_present(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '`']);
    !value.is_empty()
        && !value.starts_with("${")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "none" | "null" | "0"
        )
}

fn vector_config_strategy(provider: &str) -> &'static str {
    match provider {
        "pinecone" => "pinecone-config",
        "qdrant" => "qdrant-config",
        "weaviate" => "weaviate-config",
        "chroma" => "chroma-config",
        "milvus" => "milvus-config",
        "generic" => "vector-store-config",
        _ => "vector-store-config",
    }
}

fn key_contains_any(key: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        if needle.contains('.') && key.contains(needle) {
            return true;
        }
        key.split('.')
            .any(|part| part == *needle || part.contains(needle))
    })
}
