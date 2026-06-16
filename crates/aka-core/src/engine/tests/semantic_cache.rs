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

#[test]
fn synthesizes_python_redis_client_receiver_cache_nodes() {
    let repo = temp_repo("python-redis-client-cache");
    std::fs::write(
        repo.join("redis_ops.py"),
        r#"import redis
import redis.asyncio as aioredis

redis_client = redis.Redis.from_url("redis://cache")

class OrderCache:
    def __init__(self):
        self.redis = aioredis.from_url("redis://cache")

    async def load_order(self, order_id):
        value = await self.redis.hget("orders:data", order_id)
        if await self.redis.exists("orders:lock"):
            return value
        await self.redis.expire("orders:data", 60)
        return value

def warm_order(order_id):
    redis_client.hset("orders:data", order_id, "ok")
    redis_client.incr("orders:count")
    redis_client.delete("orders:lock")

def not_redis(model):
    return model.get("orders:should-not-count")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "load_order",
        "redis_ops.OrderCache.load_order",
        "redis_ops.py",
        (10, 15),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "warm_order",
        "redis_ops.warm_order",
        "redis_ops.py",
        (17, 20),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "not_redis",
        "redis_ops.not_redis",
        "redis_ops.py",
        (22, 23),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let data = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders:data" && cache.backend == "redis")
        .expect("redis hash key");
    assert_eq!(data.readers.len(), 1);
    assert_eq!(data.writers.len(), 2);
    let count = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders:count" && cache.backend == "redis")
        .expect("redis counter key");
    assert_eq!(count.writers.len(), 1);
    let lock = synth
        .caches
        .iter()
        .find(|cache| cache.name == "orders:lock" && cache.backend == "redis")
        .expect("redis lock key");
    assert_eq!(lock.readers.len(), 1);
    assert_eq!(lock.evictors.len(), 1);
    assert!(!synth
        .caches
        .iter()
        .any(|cache| cache.name == "orders:should-not-count"));
    let strategies: BTreeSet<_> = synth
        .caches
        .iter()
        .flat_map(SynthCache::edge_recs)
        .filter_map(|edge| {
            edge.evidence
                .as_ref()
                .and_then(|value| value.get("strategy"))
                .and_then(|value| value.as_str().map(str::to_string))
        })
        .collect();
    assert!(strategies.contains("python-redis-client-hget"));
    assert!(strategies.contains("python-redis-client-hset"));
    assert!(strategies.contains("python-redis-client-incr"));
    assert!(strategies.contains("python-redis-client-delete"));
}
