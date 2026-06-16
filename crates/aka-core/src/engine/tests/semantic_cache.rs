use super::*;
use serde_json::json;

#[test]
fn synthesizes_java_cache_nodes() {
    let repo = temp_repo("java-cache");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/cache")).unwrap();
    let file = "src/main/java/com/example/cache/OrderCache.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.cache;

import org.springframework.cache.annotation.CacheEvict;
import org.springframework.cache.annotation.Cacheable;

class OrderCache {
    @Cacheable(cacheNames = "orders")
    public OrderDto loadOrder(String id) {
        return redisTemplate.opsForValue().get("orders:" + id);
    }

    @CacheEvict(value = "orders")
    public void evictOrder(String id) {
        redisTemplate.delete("orders:" + id);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "loadOrder",
        "com.example.cache.OrderCache.loadOrder",
        file,
        (7, 9),
        json!({
            "decorators": ["@Cacheable(cacheNames = \"orders\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "evictOrder",
        "com.example.cache.OrderCache.evictOrder",
        file,
        (12, 14),
        json!({
            "decorators": ["@CacheEvict(value = \"orders\")"],
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let cache = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders" && cache.backend == "spring-cache")
        .expect("spring orders cache");
    assert_eq!(cache.readers.len(), 1);
    assert_eq!(cache.evictors.len(), 1);
    let redis = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders:" && cache.backend == "redis")
        .expect("redis key prefix");
    assert_eq!(redis.readers.len(), 1);
    assert_eq!(redis.evictors.len(), 1);
    let edge_types: Vec<_> = synth
        .caches
        .iter()
        .flat_map(SynthCache::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"READS_CACHE".to_string()));
    assert!(edge_types.contains(&"EVICTS_CACHE".to_string()));
}

#[test]
fn synthesizes_java_cache_nodes_from_source_annotations_without_metadata() {
    let repo = temp_repo("java-cache-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/cache")).unwrap();
    let file = "src/main/java/com/example/cache/OrderCache.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.cache;

import org.springframework.cache.annotation.CacheEvict;
import org.springframework.cache.annotation.CachePut;
import org.springframework.cache.annotation.Cacheable;

class OrderCache {
    @Cacheable(
        cacheNames = "orders")
    public OrderDto loadOrder(String id) {
        return null;
    }

    @CachePut(cacheNames = "orders")
    public OrderDto warmOrder(String id) {
        return null;
    }

    @CacheEvict(value = "orders")
    public void evictOrder(String id) {}
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "loadOrder",
        "com.example.cache.OrderCache.loadOrder",
        file,
        (10, 12),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "warmOrder",
        "com.example.cache.OrderCache.warmOrder",
        file,
        (15, 17),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "evictOrder",
        "com.example.cache.OrderCache.evictOrder",
        file,
        (20, 20),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let cache = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders" && cache.backend == "spring-cache")
        .expect("spring orders cache from source annotations");
    assert_eq!(cache.readers.len(), 1);
    assert_eq!(cache.writers.len(), 1);
    assert_eq!(cache.evictors.len(), 1);
    let edge_types: Vec<_> = cache
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"READS_CACHE".to_string()));
    assert!(edge_types.contains(&"WRITES_CACHE".to_string()));
    assert!(edge_types.contains(&"EVICTS_CACHE".to_string()));
}

#[test]
fn synthesizes_python_cache_nodes() {
    let repo = temp_repo("python-cache");
    std::fs::write(
        repo.join("cache_ops.py"),
        r#"from django.core.cache import cache

def load_order(order_id, redis):
    value = cache.get("orders:list")
    cached = cache.get_many(["orders:summary", "orders:stats"])
    redis.mget("orders:count", "orders:latest")
    redis.set("orders:last", order_id)
    return redis.get("orders:last")

def warm_order_cache(redis):
    cache.set_many({"orders:summary": "ok", "orders:stats": "ok"})
    redis.mset({"orders:count": "1", "orders:latest": "42"})

def evict_order():
    cache.delete("orders:list")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "load_order",
        "cache_ops.load_order",
        "cache_ops.py",
        (3, 6),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "warm_order_cache",
        "cache_ops.warm_order_cache",
        "cache_ops.py",
        (8, 10),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "evict_order",
        "cache_ops.evict_order",
        "cache_ops.py",
        (12, 13),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let django = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders:list" && cache.backend == "django-cache")
        .expect("django cache key");
    assert_eq!(django.readers.len(), 1);
    assert_eq!(django.evictors.len(), 1);
    let redis = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders:last" && cache.backend == "redis")
        .expect("redis cache key");
    assert_eq!(redis.readers.len(), 1);
    assert_eq!(redis.writers.len(), 1);
    assert!(synth.caches.iter().any(|cache| {
        cache.name == "orders:summary"
            && cache.backend == "django-cache"
            && cache.readers.len() == 1
            && cache.writers.len() == 1
    }));
    assert!(synth.caches.iter().any(|cache| {
        cache.name == "orders:count"
            && cache.backend == "redis"
            && cache.readers.len() == 1
            && cache.writers.len() == 1
    }));
    let edge_types: Vec<_> = synth
        .caches
        .iter()
        .flat_map(SynthCache::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"READS_CACHE".to_string()));
    assert!(edge_types.contains(&"WRITES_CACHE".to_string()));
    assert!(edge_types.contains(&"EVICTS_CACHE".to_string()));
}
