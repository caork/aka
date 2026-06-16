use std::collections::HashSet;

use super::{cache_name_literals, CacheAccessKind, CacheDetection};
use crate::engine::SynthNode;
use crate::engine::{find_call_args, node_at_offset, pick_handler_node, split_top_level_commas};

pub(super) fn extract_python_redis_receiver_detections(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<CacheDetection> {
    if !has_python_redis_context(text) {
        return Vec::new();
    }
    let aliases = python_redis_aliases(text);
    let mut out = Vec::new();
    for (method, kind, strategy) in [
        ("get", CacheAccessKind::Read, "python-redis-client-get"),
        ("mget", CacheAccessKind::Read, "python-redis-client-mget"),
        ("hget", CacheAccessKind::Read, "python-redis-client-hget"),
        (
            "hgetall",
            CacheAccessKind::Read,
            "python-redis-client-hgetall",
        ),
        ("hmget", CacheAccessKind::Read, "python-redis-client-hmget"),
        (
            "exists",
            CacheAccessKind::Read,
            "python-redis-client-exists",
        ),
        ("ttl", CacheAccessKind::Read, "python-redis-client-ttl"),
        ("set", CacheAccessKind::Write, "python-redis-client-set"),
        ("setex", CacheAccessKind::Write, "python-redis-client-setex"),
        (
            "psetex",
            CacheAccessKind::Write,
            "python-redis-client-psetex",
        ),
        ("mset", CacheAccessKind::Write, "python-redis-client-mset"),
        ("hset", CacheAccessKind::Write, "python-redis-client-hset"),
        ("hmset", CacheAccessKind::Write, "python-redis-client-hmset"),
        ("incr", CacheAccessKind::Write, "python-redis-client-incr"),
        (
            "incrby",
            CacheAccessKind::Write,
            "python-redis-client-incrby",
        ),
        ("decr", CacheAccessKind::Write, "python-redis-client-decr"),
        (
            "decrby",
            CacheAccessKind::Write,
            "python-redis-client-decrby",
        ),
        (
            "expire",
            CacheAccessKind::Write,
            "python-redis-client-expire",
        ),
        (
            "pexpire",
            CacheAccessKind::Write,
            "python-redis-client-pexpire",
        ),
        ("lpush", CacheAccessKind::Write, "python-redis-client-lpush"),
        ("rpush", CacheAccessKind::Write, "python-redis-client-rpush"),
        ("sadd", CacheAccessKind::Write, "python-redis-client-sadd"),
        ("zadd", CacheAccessKind::Write, "python-redis-client-zadd"),
        (
            "delete",
            CacheAccessKind::Evict,
            "python-redis-client-delete",
        ),
        (
            "unlink",
            CacheAccessKind::Evict,
            "python-redis-client-unlink",
        ),
    ] {
        let callee = format!(".{method}");
        for call in find_call_args(text, &callee) {
            let Some(receiver) = python_receiver_before_dot(text, call.start) else {
                continue;
            };
            if !is_python_redis_receiver(receiver, &aliases) {
                continue;
            }
            let Some(node) =
                node_at_offset(text, nodes, call.start).or_else(|| pick_handler_node(nodes))
            else {
                continue;
            };
            let args = split_top_level_commas(call.args);
            let Some(key_arg) = args.first() else {
                continue;
            };
            for name in cache_name_literals(key_arg) {
                out.push(CacheDetection {
                    name,
                    backend: "redis".into(),
                    kind,
                    node_id: node.aka_id.clone(),
                    strategy: strategy.into(),
                });
            }
        }
    }
    out
}

fn has_python_redis_context(text: &str) -> bool {
    text.contains("import redis")
        || text.contains("from redis")
        || text.contains("aioredis")
        || text.contains("redis.asyncio")
        || text.contains("Redis(")
        || text.contains("StrictRedis(")
        || text.contains("from_url(")
}

fn python_redis_aliases(text: &str) -> HashSet<String> {
    let mut aliases = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            continue;
        };
        let rhs_lower = rhs.to_ascii_lowercase();
        if !rhs_lower.contains("redis")
            && !rhs_lower.contains("aioredis")
            && !rhs_lower.contains("from_url(")
        {
            continue;
        }
        let lhs = lhs.trim();
        if lhs.is_empty() || lhs.contains(',') {
            continue;
        }
        let normalized = lhs.trim_start_matches("self.").trim_start_matches("cls.");
        if is_python_receiver_ident(normalized) {
            aliases.insert(normalized.to_string());
        }
        if is_python_receiver_path(lhs) {
            aliases.insert(lhs.to_string());
        }
    }
    aliases
}

fn is_python_redis_receiver(receiver: &str, aliases: &HashSet<String>) -> bool {
    if aliases.contains(receiver) {
        return true;
    }
    let tail = receiver.rsplit('.').next().unwrap_or(receiver);
    aliases.contains(tail) || tail.to_ascii_lowercase().contains("redis")
}

fn python_receiver_before_dot(text: &str, dot_start: usize) -> Option<&str> {
    if text.as_bytes().get(dot_start) != Some(&b'.') {
        return None;
    }
    let mut start = dot_start;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if ch == '.' || ch == '_' || ch.is_ascii_alphanumeric() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let receiver = text[start..dot_start].trim_matches('.');
    is_python_receiver_path(receiver).then_some(receiver)
}

fn is_python_receiver_path(value: &str) -> bool {
    !value.is_empty()
        && value
            .split('.')
            .all(|part| is_python_receiver_ident(part) || matches!(part, "self" | "cls"))
}

fn is_python_receiver_ident(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}
