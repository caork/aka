//! aka-graph 集成测试：摄取 / 遍历 / 布局 / LOD / perf smoke。

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use aka_core::{EdgeRec, NodeRec};
use aka_graph::{compute_layout, Adjacency, GraphError, GraphStore};
use serde_json::json;

// ── 工具 ─────────────────────────────────────────────────────────

fn temp_db(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("aka-graph-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let base = format!("{name}-{}", std::process::id());
    for suffix in ["db", "db-wal", "db-shm"] {
        let _ = std::fs::remove_file(dir.join(format!("{base}.{suffix}")));
    }
    dir.join(format!("{base}.db"))
}

fn node(id: &str, label: &str, props: serde_json::Value) -> NodeRec {
    let serde_json::Value::Object(properties) = props else {
        panic!("props must be a json object");
    };
    NodeRec {
        id: id.to_string(),
        label: label.to_string(),
        properties,
    }
}

fn edge(source: &str, target: &str, edge_type: &str) -> EdgeRec {
    EdgeRec {
        id: format!("{source}->{target}:{edge_type}"),
        source_id: source.to_string(),
        target_id: target.to_string(),
        edge_type: edge_type.to_string(),
        confidence: 1.0,
        reason: String::new(),
        step: None,
        evidence: None,
    }
}

/// 手造 mini 图（10 节点 13 边）：
/// main→A→B→C 调用链 + Derived EXTENDS Base + Base HAS_METHOD M
/// + 两个 Community（com1: main/a/b，com2: c/base/derived/m）+ File 节点。
fn mini_graph() -> (Vec<NodeRec>, Vec<EdgeRec>) {
    let nodes = vec![
        node("f1", "File", json!({"filePath": "src/main.rs"})),
        node("main", "Function", json!({"name": "main", "filePath": "src/main.rs", "startLine": 1, "endLine": 5})),
        node("a", "Function", json!({"name": "alpha", "filePath": "src/main.rs", "startLine": 10, "endLine": 15})),
        node("b", "Function", json!({"name": "beta", "filePath": "src/main.rs", "startLine": 20, "endLine": 25})),
        node("c", "Function", json!({"name": "gamma", "filePath": "src/main.rs", "startLine": 30, "endLine": 35})),
        node("base", "Class", json!({"name": "Base", "filePath": "src/lib/types.rs", "startLine": 1})),
        node("derived", "Class", json!({"name": "Derived", "filePath": "src/lib/types.rs", "startLine": 40})),
        node("m", "Method", json!({"name": "run", "filePath": "src/lib/types.rs", "startLine": 5})),
        node("com1", "Community", json!({"name": "community-one"})),
        node("com2", "Community", json!({"name": "community-two"})),
    ];
    let edges = vec![
        edge("main", "a", "CALLS"),
        edge("a", "b", "CALLS"),
        edge("b", "c", "CALLS"),
        edge("derived", "base", "EXTENDS"),
        edge("base", "m", "HAS_METHOD"),
        edge("main", "com1", "MEMBER_OF"),
        edge("a", "com1", "MEMBER_OF"),
        edge("b", "com1", "MEMBER_OF"),
        edge("c", "com2", "MEMBER_OF"),
        edge("base", "com2", "MEMBER_OF"),
        edge("derived", "com2", "MEMBER_OF"),
        edge("m", "com2", "MEMBER_OF"),
        edge("f1", "main", "CONTAINS"),
    ];
    (nodes, edges)
}

fn mini_store() -> GraphStore {
    let mut store = GraphStore::open_in_memory().unwrap();
    let (nodes, edges) = mini_graph();
    let stats = store.ingest(nodes.into_iter(), edges.into_iter()).unwrap();
    assert_eq!(stats.nodes, 10);
    assert_eq!(stats.edges, 13);
    assert_eq!(stats.dangling_edges, 0);
    store
}

fn idx(adj: &Adjacency, id: &str) -> u32 {
    adj.index_of_id(id).unwrap_or_else(|| panic!("node {id} missing"))
}

// ── 摄取与基本查询 ───────────────────────────────────────────────

#[test]
fn create_open_roundtrip() {
    let path = temp_db("roundtrip");
    {
        let mut store = GraphStore::create(&path).unwrap();
        let (nodes, edges) = mini_graph();
        store.ingest(nodes.into_iter(), edges.into_iter()).unwrap();
    }
    let store = GraphStore::open(&path).unwrap();
    assert_eq!(store.node_count().unwrap(), 10);
    assert_eq!(store.edge_count().unwrap(), 13);

    let missing = temp_db("missing-never-created");
    assert!(matches!(
        GraphStore::open(&missing),
        Err(GraphError::DbNotFound(_))
    ));
}

#[test]
fn dangling_edges_skipped_and_counted() {
    let mut store = GraphStore::open_in_memory().unwrap();
    let nodes = vec![
        node("n1", "Function", json!({"name": "one"})),
        node("n2", "Function", json!({"name": "two"})),
    ];
    let edges = vec![
        edge("n1", "n2", "CALLS"),
        edge("n1", "ghost", "CALLS"),
        edge("ghost", "n2", "IMPORTS"),
    ];
    let stats = store.ingest(nodes.into_iter(), edges.into_iter()).unwrap();
    assert_eq!(stats.nodes, 2);
    assert_eq!(stats.edges, 1);
    assert_eq!(stats.dangling_edges, 2);
    assert_eq!(store.edge_count().unwrap(), 1);

    // 重复摄取：节点 id 撞库跳过计数。
    let stats2 = store
        .ingest(
            vec![
                node("n1", "Function", json!({"name": "one"})),
                node("n3", "Function", json!({"name": "three"})),
            ]
            .into_iter(),
            std::iter::empty(),
        )
        .unwrap();
    assert_eq!(stats2.nodes, 1);
    assert_eq!(stats2.duplicate_nodes, 1);
    assert_eq!(store.node_count().unwrap(), 3);

    // 增量摄取的边可以引用旧批次节点。
    let stats3 = store
        .ingest(std::iter::empty(), vec![edge("n3", "n2", "CALLS")].into_iter())
        .unwrap();
    assert_eq!(stats3.edges, 1);
    assert_eq!(stats3.dangling_edges, 0);
}

#[test]
fn basic_queries() {
    let store = mini_store();

    let main = store.node_by_id("main").unwrap().unwrap();
    assert_eq!(main.label, "Function");
    assert_eq!(main.name.as_deref(), Some("main"));
    assert_eq!(main.file_path.as_deref(), Some("src/main.rs"));
    /* 工件 startLine=1（0-based）→ 存储为 1-based 的 2 */
    assert_eq!(main.start_line, Some(2));
    assert_eq!(main.end_line, Some(6));
    assert_eq!(main.props["name"], "main");
    assert!(store.node_by_id("nope").unwrap().is_none());

    // 精确命中排最前。
    let hits = store.nodes_by_name("main", 10).unwrap();
    assert_eq!(hits[0].id, "main");

    // 前缀匹配。
    let hits = store.nodes_by_name("comm", 10).unwrap();
    let names: Vec<_> = hits.iter().filter_map(|n| n.name.clone()).collect();
    assert_eq!(names, vec!["community-one", "community-two"]);

    // limit 生效。
    assert_eq!(store.nodes_by_name("comm", 1).unwrap().len(), 1);

    // 文件内节点按起始行排序（f1 无 startLine 排最前）。
    let in_file = store.nodes_in_file("src/main.rs").unwrap();
    let ids: Vec<_> = in_file.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["f1", "main", "a", "b", "c"]);

    // 文件清单：仅含有 startLine 的节点计数，f1（File，无 startLine）不计；
    // Community 节点无 filePath 被排除；按 path 升序。
    let files = store.file_list().unwrap();
    assert_eq!(
        files,
        vec![
            ("src/lib/types.rs".to_string(), 3), // base + derived + m
            ("src/main.rs".to_string(), 4),       // main + a + b + c（f1 不计）
        ]
    );
}

// ── 遍历 ─────────────────────────────────────────────────────────

#[test]
fn traversals_exact() {
    let store = mini_store();
    let adj = Adjacency::build(&store).unwrap();
    assert_eq!(adj.len(), 10);

    let (main, a, b, c) = (idx(&adj, "main"), idx(&adj, "a"), idx(&adj, "b"), idx(&adj, "c"));
    let (base, derived, m) = (idx(&adj, "base"), idx(&adj, "derived"), idx(&adj, "m"));
    let com2 = idx(&adj, "com2");

    // u32 ↔ id 双向映射。
    assert_eq!(adj.id_of(main), "main");
    assert_eq!(adj.index_of_rowid(adj.rowid_of(main)), Some(main));

    // callees：只走 CALLS，正向。
    assert_eq!(adj.callees(main, 10, 100), vec![(a, 1), (b, 2), (c, 3)]);
    assert_eq!(adj.callees(main, 1, 100), vec![(a, 1)]);
    assert_eq!(adj.callees(c, 10, 100), vec![]);

    // callers：只走 CALLS，反向。
    assert_eq!(adj.callers(c, 10, 100), vec![(b, 1), (a, 2), (main, 3)]);
    assert_eq!(adj.callers(main, 10, 100), vec![]);

    // impact：反向多类型。CONTAINS 不在白名单 → f1 不出现。
    assert_eq!(adj.impact(c, 10, 100), vec![(b, 1), (a, 2), (main, 3)]);
    // m 的影响面：HAS_METHOD 反向到 base，再 EXTENDS 反向到 derived。
    assert_eq!(adj.impact(m, 10, 100), vec![(base, 1), (derived, 2)]);
    // limit 截断。
    assert_eq!(adj.impact(c, 10, 2), vec![(b, 1), (a, 2)]);
    // com2 一跳影响面 = 四个成员。
    let hit: HashSet<u32> = adj.impact(com2, 1, 100).iter().map(|&(n, _)| n).collect();
    assert_eq!(hit, HashSet::from([c, base, derived, m]));

    // neighbors：任意类型一跳，带类型名与方向。
    let neigh: HashSet<(u32, String, bool)> = adj
        .neighbors(a)
        .into_iter()
        .map(|nb| (nb.node, nb.edge_type.to_string(), nb.outgoing))
        .collect();
    assert_eq!(
        neigh,
        HashSet::from([
            (b, "CALLS".to_string(), true),
            (idx(&adj, "com1"), "MEMBER_OF".to_string(), true),
            (main, "CALLS".to_string(), false),
        ])
    );

    // 度数。
    assert_eq!(adj.degree(com2), 4);
    assert_eq!(adj.degree(main), 3);
}

// ── 布局 ─────────────────────────────────────────────────────────

#[test]
fn layout_deterministic_and_clustered() {
    let store = mini_store();
    let adj = Adjacency::build(&store).unwrap();

    compute_layout(&store, &adj).unwrap();
    let first = store.positions().unwrap();
    assert_eq!(first.len(), 10);
    for p in &first {
        assert!(p.x.is_finite() && p.y.is_finite() && p.size.is_finite(), "NaN in {p:?}");
    }

    // 确定性：重跑坐标逐位一致。
    compute_layout(&store, &adj).unwrap();
    let second = store.positions().unwrap();
    assert_eq!(first, second);

    // 簇结构：com2(5 成员) > com1(4) > src 目录(1, 只有 f1)。
    let cg = store.cluster_graph().unwrap();
    let labels: Vec<_> = cg.nodes.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, vec!["community-two", "community-one", "src"]);
    let counts: Vec<_> = cg.nodes.iter().map(|c| c.count).collect();
    assert_eq!(counts, vec![5, 4, 1]);

    // 成员归属正确：c/base/derived/m/com2 → 簇 0；main/a/b/com1 → 簇 1；f1 → 簇 2。
    let cluster_of = |id: &str| {
        let rowid = store.node_by_id(id).unwrap().unwrap().rowid;
        first.iter().find(|p| p.node == rowid).unwrap().cluster
    };
    for id in ["com2", "c", "base", "derived", "m"] {
        assert_eq!(cluster_of(id), 0, "{id} should be in cluster 0");
    }
    for id in ["com1", "main", "a", "b"] {
        assert_eq!(cluster_of(id), 1, "{id} should be in cluster 1");
    }
    assert_eq!(cluster_of("f1"), 2);

    // 同簇成员距簇心 < 簇间距（任意两簇中心的最小距离）。
    let centers: Vec<(f64, f64)> = cg.nodes.iter().map(|c| (c.x as f64, c.y as f64)).collect();
    let mut min_center_dist = f64::INFINITY;
    for i in 0..centers.len() {
        for j in (i + 1)..centers.len() {
            let d = (centers[i].0 - centers[j].0).hypot(centers[i].1 - centers[j].1);
            min_center_dist = min_center_dist.min(d);
        }
    }
    for p in &first {
        let (cx, cy) = centers[p.cluster as usize];
        let d = (p.x - cx).hypot(p.y - cy);
        assert!(
            d < min_center_dist,
            "node {} dist-to-center {d:.2} >= min cluster spacing {min_center_dist:.2}",
            p.node
        );
    }

    // 簇间聚合边：b→c 跨 com1/com2，f1→main 跨 src/com1。
    let agg: Vec<(u32, u32, u32)> = cg.edges.iter().map(|e| (e.s, e.t, e.w)).collect();
    assert_eq!(agg, vec![(0, 1, 1), (1, 2, 1)]);
}

#[test]
fn layout_without_communities_uses_top_dirs() {
    let mut store = GraphStore::open_in_memory().unwrap();
    let nodes = vec![
        node("x1", "Function", json!({"name": "x1", "filePath": "core/a.rs"})),
        node("x2", "Function", json!({"name": "x2", "filePath": "core/b.rs"})),
        node("y1", "Function", json!({"name": "y1", "filePath": "ui/c.rs"})),
        node("z1", "Function", json!({"name": "z1", "filePath": "root.rs"})),
        node("w1", "Function", json!({"name": "w1"})),
    ];
    store
        .ingest(nodes.into_iter(), vec![edge("x1", "x2", "CALLS")].into_iter())
        .unwrap();
    let adj = Adjacency::build(&store).unwrap();
    compute_layout(&store, &adj).unwrap();

    let cg = store.cluster_graph().unwrap();
    let mut summary: Vec<(String, u32)> =
        cg.nodes.iter().map(|c| (c.label.clone(), c.count)).collect();
    summary.sort();
    // core 2 个；ui 1 个；根级文件与无路径节点都归 (root)。
    assert_eq!(
        summary,
        vec![
            ("(root)".to_string(), 2),
            ("core".to_string(), 2),
            ("ui".to_string(), 1)
        ]
    );
}

// ── LOD ──────────────────────────────────────────────────────────

#[test]
fn lod_snapshot_shape_and_truncation() {
    let store = mini_store();
    let adj = Adjacency::build(&store).unwrap();
    compute_layout(&store, &adj).unwrap();

    // max_nodes 截断：按度数取 top5 = com2(4), main/a/b/base(3, rowid 序)。
    let snap = store.lod_snapshot(5).unwrap();
    assert_eq!(snap.nodes.len(), 5);
    let ids: Vec<_> = snap.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["com2", "main", "a", "b", "base"]);
    assert_eq!(snap.classes, vec!["Community", "Function", "Class"]);
    let l: Vec<_> = snap.nodes.iter().map(|n| n.l).collect();
    assert_eq!(l, vec![0, 1, 1, 1, 2]);
    let i: Vec<_> = snap.nodes.iter().map(|n| n.i).collect();
    assert_eq!(i, vec![0, 1, 2, 3, 4]);
    // 两端都入选的边：main→a, a→b, base→com2（按摄取序扫描）。
    assert_eq!(snap.edges, vec![1, 2, 2, 3, 4, 0]);

    // JSON 形状与前端约定一致。
    let v = serde_json::to_value(&snap).unwrap();
    assert!(v["classes"].is_array());
    assert!(v["edges"].is_array());
    let n0 = &v["nodes"][0];
    for key in ["i", "id", "x", "y", "s", "c", "l", "name"] {
        assert!(!n0[key].is_null(), "missing key {key} in lod node json");
    }
    assert_eq!(n0["id"], "com2");
    assert_eq!(n0["name"], "community-two");

    // 全量快照：10 节点，13 条边里 (s,t) 全部唯一。
    let full = store.lod_snapshot(1000).unwrap();
    assert_eq!(full.nodes.len(), 10);
    assert_eq!(full.edges.len(), 13 * 2);

    // 二进制形态与 JSON 形态一致。
    let bin = store.lod_snapshot_binary(5).unwrap();
    assert_eq!(bin.positions.len(), 10);
    assert_eq!(bin.edges, snap.edges);
    for (k, n) in snap.nodes.iter().enumerate() {
        assert_eq!(bin.positions[k * 2], n.x);
        assert_eq!(bin.positions[k * 2 + 1], n.y);
    }
    let meta: serde_json::Value = serde_json::from_str(&bin.meta_json).unwrap();
    assert_eq!(meta["count"], 5);
    assert_eq!(meta["classes"], json!(["Community", "Function", "Class"]));
    assert_eq!(meta["nodes"][0]["id"], "com2");
    assert!(meta["nodes"][0]["x"].is_null(), "meta json should not carry coordinates");

    // 未布局直接取快照 → 明确报错。
    let bare = mini_store();
    assert!(matches!(
        bare.lod_snapshot(10),
        Err(GraphError::Invalid(_))
    ));
}

// ── ego 子图 ─────────────────────────────────────────────────────

#[test]
fn ego_graph_center_rings_and_edges() {
    let store = mini_store();
    let adj = Adjacency::build(&store).unwrap();

    let ego = store.ego_graph(&adj, "a", 2, 2000).unwrap();

    // 中心 i=0 且固定在原点。
    assert_eq!(ego.nodes[0].id, "a");
    assert_eq!(ego.nodes[0].i, 0);
    assert_eq!(ego.nodes[0].x, 0.0);
    assert_eq!(ego.nodes[0].y, 0.0);
    assert_eq!(ego.nodes[0].c, 0);

    // BFS 无向分环：ring1 = {main,b,com1}（同度数按稠密下标），ring2 = {c,f1}（度数降序）。
    let ids: Vec<_> = ego.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "main", "b", "com1", "c", "f1"]);
    let i: Vec<_> = ego.nodes.iter().map(|n| n.i).collect();
    assert_eq!(i, vec![0, 1, 2, 3, 4, 5], "i 必须从 0 紧凑重编号");

    // 环半径 = 深度 × 固定步长；c 字段 = 环号。
    for n in &ego.nodes {
        let r = (n.x * n.x + n.y * n.y).sqrt();
        let expect = n.c as f32 * aka_graph::EGO_RING_STEP;
        assert!(
            (r - expect).abs() < 1e-3,
            "node {} ring {} radius {} != {}",
            n.id,
            n.c,
            r,
            expect
        );
    }
    let rings: Vec<_> = ego.nodes.iter().map(|n| n.c).collect();
    assert_eq!(rings, vec![0, 1, 1, 1, 2, 2]);

    // 子图边 = 两端都入选（按 i 序扫描出边）。
    assert_eq!(ego.edges, vec![0, 2, 0, 3, 1, 0, 1, 3, 2, 4, 2, 3, 5, 1]);

    // 与 LodGraph JSON 完全同形。
    let v = serde_json::to_value(&ego).unwrap();
    for key in ["i", "id", "x", "y", "s", "c", "l", "name"] {
        assert!(!v["nodes"][0][key].is_null(), "missing key {key} in ego node json");
    }
}

#[test]
fn ego_graph_budget_truncates_by_degree() {
    let store = mini_store();
    let adj = Adjacency::build(&store).unwrap();

    // 预算 4：center + ring1 全部，ring2 进不来。
    let ego = store.ego_graph(&adj, "a", 2, 4).unwrap();
    let ids: Vec<_> = ego.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "main", "b", "com1"]);

    // 预算 3：ring1 按 (度数降序, 下标升序) 截到 2 个。
    let ego = store.ego_graph(&adj, "a", 2, 3).unwrap();
    let ids: Vec<_> = ego.nodes.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "main", "b"]);

    // depth 0 → 只有 center。
    let ego = store.ego_graph(&adj, "a", 0, 2000).unwrap();
    assert_eq!(ego.nodes.len(), 1);
    assert!(ego.edges.is_empty());
}

#[test]
fn ego_graph_deterministic_and_missing_center() {
    let store = mini_store();
    let adj = Adjacency::build(&store).unwrap();

    let a = serde_json::to_string(&store.ego_graph(&adj, "main", 2, 100).unwrap()).unwrap();
    let b = serde_json::to_string(&store.ego_graph(&adj, "main", 2, 100).unwrap()).unwrap();
    assert_eq!(a, b, "ego 输出必须逐位确定");

    assert!(matches!(
        store.ego_graph(&adj, "nope", 2, 100),
        Err(GraphError::Invalid(_))
    ));
}

// ── perf smoke：10 万节点 / 50 万边（zipf 度分布） ────────────────

struct Lcg(u64);

impl Lcg {
    fn next_f64(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // 取高 53 位映射到 [0,1)。
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[test]
fn perf_smoke_100k_nodes_500k_edges() {
    const N: usize = 100_000;
    const M: usize = 500_000;
    const EDGE_TYPES: [&str; 5] = ["CALLS", "IMPORTS", "CONTAINS", "DEFINES", "EXTENDS"];
    const LABELS: [&str; 4] = ["Function", "Class", "Method", "Interface"];

    let path = temp_db("perf-smoke");
    let mut store = GraphStore::create(&path).unwrap();

    let nodes = (0..N).map(|i| {
        node(
            &format!("n{i}"),
            LABELS[i % LABELS.len()],
            json!({
                "name": format!("sym_{i}"),
                "filePath": format!("mod{:02}/file{}.rs", i % 40, i % 4000),
                "startLine": 1
            }),
        )
    });
    let mut rng = Lcg(0x5eed_cafe);
    let edges: Vec<EdgeRec> = (0..M)
        .map(|i| {
            let s = (rng.next_f64() * N as f64) as usize % N;
            // zipf 风格偏斜：u^3 把目标端压向低编号节点 → 少数枢纽高度数。
            let t = (N as f64 * rng.next_f64().powi(3)) as usize % N;
            edge(
                &format!("n{s}"),
                &format!("n{t}"),
                EDGE_TYPES[i % EDGE_TYPES.len()],
            )
        })
        .collect();

    let t0 = Instant::now();
    let stats = store.ingest(nodes, edges.into_iter()).unwrap();
    let ingest_t = t0.elapsed();
    assert_eq!(stats.nodes as usize, N);
    assert_eq!(stats.edges as usize, M);
    assert_eq!(stats.dangling_edges, 0);

    let t1 = Instant::now();
    let adj = Adjacency::build(&store).unwrap();
    let build_t = t1.elapsed();
    assert_eq!(adj.len() as usize, N);

    let t2 = Instant::now();
    compute_layout(&store, &adj).unwrap();
    let layout_t = t2.elapsed();

    let t3 = Instant::now();
    let snap = store.lod_snapshot(5_000).unwrap();
    let lod_t = t3.elapsed();
    assert_eq!(snap.nodes.len(), 5_000);

    // 抽查：高度数枢纽节点 BFS。
    let hub = idx(&adj, "n0");
    let t4 = Instant::now();
    let impacted = adj.impact(hub, 3, 10_000);
    let bfs_t = t4.elapsed();

    let positions = store.positions().unwrap();
    assert_eq!(positions.len(), N);
    assert!(positions.iter().all(|p| p.x.is_finite() && p.y.is_finite()));

    println!(
        "perf smoke ({N} nodes / {M} edges):\n  ingest  {:>8.0?}  ({:.0} nodes/s, {:.0} rows/s)\n  build   {:>8.0?}\n  layout  {:>8.0?}\n  lod5k   {:>8.0?}\n  impact3 {:>8.0?} ({} hits)",
        ingest_t,
        N as f64 / ingest_t.as_secs_f64(),
        (N + M) as f64 / ingest_t.as_secs_f64(),
        build_t,
        layout_t,
        lod_t,
        bfs_t,
        impacted.len()
    );

    assert!(build_t.as_secs_f64() < 2.0, "Adjacency::build took {build_t:?} (>= 2s)");
    assert!(layout_t.as_secs_f64() < 5.0, "compute_layout took {layout_t:?} (>= 5s)");
}
