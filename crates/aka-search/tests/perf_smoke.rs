//! perf smoke：批量索引 + 重复查询计时。
//!
//! - `perf_smoke_small`：5 千 chunk / 20 次查询，默认运行（debug 下也应 < 50ms/次）。
//! - `perf_smoke_100k`：10 万 chunk / 100 次查询，`#[ignore]`，建议
//!   `cargo test -p aka-search --release -- --ignored perf_smoke_100k --nocapture`。

use std::time::Instant;

use aka_core::types::ChunkRec;
use aka_search::{SearchIndex, SearchIndexWriter};

const VERBS: [&str; 12] = [
    "run", "build", "parse", "load", "fetch", "merge", "rank", "cache", "resolve", "emit",
    "scan", "index",
];
const NOUNS: [&str; 12] = [
    "pipeline", "repo", "kernel", "graph", "vector", "token", "file", "segment", "query",
    "node", "edge", "chunk",
];
const SUFFIXES: [&str; 6] = ["Manager", "Builder", "Handler", "Worker", "Service", "Engine"];

fn synth_chunks(n: usize) -> impl Iterator<Item = ChunkRec> {
    (0..n).map(|i| {
        let verb = VERBS[i % VERBS.len()];
        let noun = NOUNS[(i / VERBS.len()) % NOUNS.len()];
        let suffix = SUFFIXES[(i / (VERBS.len() * NOUNS.len())) % SUFFIXES.len()];
        let mut noun_cap = noun.to_owned();
        noun_cap[..1].make_ascii_uppercase();
        let name = format!("{verb}{noun_cap}{suffix}{i}");
        ChunkRec {
            node_id: format!("fn:{name}"),
            kind: "function".to_owned(),
            file_path: format!("src/{noun}/{verb}_{i}.rs"),
            start_line: 1,
            end_line: 40,
            text: format!(
                "fn {name}(input: &{noun_cap}) -> Result<Output> {{ \
                 let staged = {verb}_{noun}(input); staged.finalize() }}"
            ),
        }
    })
}

const QUERIES: [&str; 5] = [
    "pipeline repo",
    "kernel index",
    "graph builder",
    "parse file",
    "vector query",
];

fn run_perf(n_chunks: usize, n_queries: usize) {
    let dir = tempfile::tempdir().unwrap();

    let t0 = Instant::now();
    let mut writer = SearchIndexWriter::create(dir.path()).unwrap();
    writer.add_chunks(synth_chunks(n_chunks)).unwrap();
    writer.commit().unwrap();
    drop(writer);
    let index = SearchIndex::open(dir.path()).unwrap();
    let index_elapsed = t0.elapsed();
    println!(
        "indexed {n_chunks} chunks in {:.2?} ({:.0} docs/s)",
        index_elapsed,
        n_chunks as f64 / index_elapsed.as_secs_f64()
    );

    // 预热：首次查询要构建 Levenshtein DFA 等一次性开销，不计入计时。
    let _ = index.search("warmup query", 10).unwrap();

    let mut max_ms = 0.0f64;
    let mut total_ms = 0.0f64;
    for i in 0..n_queries {
        let q = QUERIES[i % QUERIES.len()];
        let t = Instant::now();
        let hits = index.search(q, 10).unwrap();
        let ms = t.elapsed().as_secs_f64() * 1e3;
        assert!(!hits.is_empty(), "query {q:?} returned nothing");
        total_ms += ms;
        if ms > max_ms {
            max_ms = ms;
        }
    }
    println!(
        "{n_queries} queries over {n_chunks} chunks: avg {:.2}ms, max {:.2}ms",
        total_ms / n_queries as f64,
        max_ms
    );
    assert!(max_ms < 50.0, "single query took {max_ms:.2}ms (limit 50ms)");
}

#[test]
fn perf_smoke_small() {
    run_perf(5_000, 20);
}

#[test]
#[ignore = "10 万 chunk 大号版，建议 --release 跑：cargo test -p aka-search --release -- --ignored --nocapture"]
fn perf_smoke_100k() {
    run_perf(100_000, 100);
}
