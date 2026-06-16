use super::{cache_name_literals, CacheAccessKind, CacheDetection};
use crate::engine::SynthNode;
use crate::engine::{find_call_args, node_at_offset, pick_handler_node, split_top_level_commas};

pub(super) fn extract_java_redis_template_detections(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<CacheDetection> {
    if !has_java_redis_context(text) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for access in [
        JavaRedisAccess::new(
            ".opsForHash().get",
            CacheAccessKind::Read,
            "java-redis-hash-get",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForHash().multiGet",
            CacheAccessKind::Read,
            "java-redis-hash-multi-get",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForHash().entries",
            CacheAccessKind::Read,
            "java-redis-hash-entries",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForHash().put",
            CacheAccessKind::Write,
            "java-redis-hash-put",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForHash().putAll",
            CacheAccessKind::Write,
            "java-redis-hash-put-all",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForList().leftPush",
            CacheAccessKind::Write,
            "java-redis-list-left-push",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForList().rightPush",
            CacheAccessKind::Write,
            "java-redis-list-right-push",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForList().range",
            CacheAccessKind::Read,
            "java-redis-list-range",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForSet().add",
            CacheAccessKind::Write,
            "java-redis-set-add",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForSet().members",
            CacheAccessKind::Read,
            "java-redis-set-members",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForZSet().add",
            CacheAccessKind::Write,
            "java-redis-zset-add",
            0,
        ),
        JavaRedisAccess::new(
            ".opsForZSet().range",
            CacheAccessKind::Read,
            "java-redis-zset-range",
            0,
        ),
        JavaRedisAccess::new(".expire", CacheAccessKind::Write, "java-redis-expire", 0),
        JavaRedisAccess::new(".hasKey", CacheAccessKind::Read, "java-redis-has-key", 0),
        JavaRedisAccess::new(
            ".opsForValue().increment",
            CacheAccessKind::Write,
            "java-redis-value-increment",
            0,
        ),
    ] {
        out.extend(extract_java_redis_access(text, nodes, access));
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct JavaRedisAccess {
    callee: &'static str,
    kind: CacheAccessKind,
    strategy: &'static str,
    arg_index: usize,
}

impl JavaRedisAccess {
    const fn new(
        callee: &'static str,
        kind: CacheAccessKind,
        strategy: &'static str,
        arg_index: usize,
    ) -> Self {
        Self {
            callee,
            kind,
            strategy,
            arg_index,
        }
    }
}

fn extract_java_redis_access(
    text: &str,
    nodes: &[&SynthNode],
    access: JavaRedisAccess,
) -> Vec<CacheDetection> {
    let mut out = Vec::new();
    for call in find_call_args(text, access.callee) {
        let Some(receiver) = java_receiver_before_dot_chain(text, call.start) else {
            continue;
        };
        if !is_java_redis_receiver(receiver) {
            continue;
        }
        let Some(node) =
            node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
        else {
            continue;
        };
        let args = split_top_level_commas(call.args);
        let Some(arg) = args.get(access.arg_index) else {
            continue;
        };
        for name in cache_name_literals(arg) {
            out.push(CacheDetection {
                name,
                backend: "redis".into(),
                kind: access.kind,
                node_id: node.aka_id.clone(),
                strategy: access.strategy.into(),
            });
        }
    }
    out
}

fn has_java_redis_context(text: &str) -> bool {
    text.contains("RedisTemplate")
        || text.contains("StringRedisTemplate")
        || text.contains("BoundHashOperations")
        || text.contains("BoundValueOperations")
        || text.contains("org.springframework.data.redis")
}

fn java_receiver_before_dot_chain(text: &str, dot_start: usize) -> Option<&str> {
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

fn is_java_redis_receiver(receiver: &str) -> bool {
    let tail = receiver.rsplit('.').next().unwrap_or(receiver);
    let lower = tail.to_ascii_lowercase();
    lower.contains("redis") || matches!(lower.as_str(), "template" | "stringredistemplate")
}
