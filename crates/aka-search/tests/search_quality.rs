//! 检索质量测试：手造 ~20 个 chunk + 若干 node，验证召回与排序。

use aka_core::types::{ChunkRec, NodeRec};
use aka_search::{SearchIndex, SearchIndexWriter};

fn chunk(node_id: &str, file: &str, text: &str) -> ChunkRec {
    ChunkRec {
        node_id: node_id.to_owned(),
        kind: "function".to_owned(),
        file_path: file.to_owned(),
        start_line: 10,
        end_line: 30,
        text: text.to_owned(),
    }
}

fn node(id: &str, label: &str, name: &str, file: &str) -> NodeRec {
    let props = serde_json::json!({
        "name": name,
        "filePath": file,
        "startLine": 5,
        "endLine": 50,
    });
    NodeRec {
        id: id.to_owned(),
        label: label.to_owned(),
        properties: props.as_object().unwrap().clone(),
    }
}

fn sample_chunks() -> Vec<ChunkRec> {
    vec![
        chunk(
            "fn:runPipelineFromRepo",
            "src/pipeline.rs",
            "fn runPipelineFromRepo(repo: &Path) -> Result<Artifacts> { \
             let pipeline = Pipeline::new(repo); pipeline.execute() }",
        ),
        chunk(
            "fn:kernel_add",
            "src/kernels/math.rs",
            "fn kernel_add(a: f32, b: f32) -> f32 { a + b }",
        ),
        chunk(
            "class:KnowledgeGraph",
            "src/graph.rs",
            "pub struct KnowledgeGraph { nodes: Vec<Node>, edges: Vec<Edge> } \
             impl KnowledgeGraph { fn insert_node(&mut self, n: Node) {} }",
        ),
        chunk(
            "fn:parseSourceFile",
            "src/parser.rs",
            "fn parseSourceFile(path: &Path) -> Ast { tree_sitter::parse(path) }",
        ),
        chunk(
            "fn:buildCallGraph",
            "src/graph_builder.rs",
            "fn buildCallGraph(ast: &Ast) -> CallGraph { walk(ast) }",
        ),
        chunk(
            "fn:computeEmbeddings",
            "src/embed.rs",
            "fn computeEmbeddings(chunks: &[Chunk]) -> Vec<Embedding> { model.encode(chunks) }",
        ),
        chunk(
            "struct:VectorIndex",
            "src/vector.rs",
            "pub struct VectorIndex { dim: usize } impl VectorIndex { fn nearest(&self, q: &[f32]) {} }",
        ),
        chunk(
            "fn:rrfMerge",
            "src/rank.rs",
            "fn rrfMerge(lexical: &[Doc], semantic: &[Doc]) -> Vec<Doc> { fuse(lexical, semantic) }",
        ),
        chunk(
            "fn:tokenizeIdentifier",
            "src/tokenize.rs",
            "fn tokenizeIdentifier(ident: &str) -> Vec<String> { split_camel(ident) }",
        ),
        chunk(
            "fn:loadManifest",
            "src/artifact.rs",
            "fn loadManifest(dir: &Path) -> Manifest { serde_json::from_reader(open(dir)) }",
        ),
        chunk(
            "fn:writeArtifacts",
            "src/emit.rs",
            "fn writeArtifacts(dir: &Path, stats: &Stats) { ndjson::write(dir, stats) }",
        ),
        chunk(
            "fn:detectLanguage",
            "src/lang.rs",
            "fn detectLanguage(path: &Path) -> Language { by_extension(path) }",
        ),
        chunk(
            "fn:resolveImports",
            "src/imports.rs",
            "fn resolveImports(module: &Module) -> Vec<Symbol> { walk_imports(module) }",
        ),
        chunk(
            "fn:rankResults",
            "src/rank.rs",
            "fn rankResults(hits: &mut Vec<Doc>) { hits.sort_by_score() }",
        ),
        chunk(
            "class:HTTPServer",
            "src/server.rs",
            "pub struct HTTPServer { port: u16 } impl HTTPServer { fn listen(&self) {} }",
        ),
        chunk(
            "fn:fetchRemoteRepo",
            "src/remote.rs",
            "fn fetchRemoteRepo(url: &str) -> Result<PathBuf> { git::clone(url) }",
        ),
        chunk(
            "struct:CacheLayer",
            "src/cache.rs",
            "pub struct CacheLayer { entries: HashMap<String, Bytes> }",
        ),
        chunk(
            "fn:configLoader",
            "src/config.rs",
            "fn configLoader(path: &Path) -> Config { toml::parse(read(path)) }",
        ),
        chunk(
            "fn:mergeSegments",
            "src/segment.rs",
            "fn mergeSegments(segments: &[Segment]) -> Segment { compact(segments) }",
        ),
        chunk(
            "fn:walkDirectory",
            "src/fs.rs",
            "fn walkDirectory(root: &Path) -> Vec<PathBuf> { ignore::Walk::new(root).collect() }",
        ),
    ]
}

fn build_index(dir: &std::path::Path) -> SearchIndex {
    let mut writer = SearchIndexWriter::create(dir).unwrap();
    // 摄取顺序与真实管线一致：节点先于 chunk（chunk 文档要携带节点真实 label）。
    writer
        .add_nodes(
            vec![
                node(
                    "class:KnowledgeGraph",
                    "Class",
                    "KnowledgeGraph",
                    "src/graph.rs",
                ),
                node(
                    "fn:runPipelineFromRepo",
                    "Function",
                    "runPipelineFromRepo",
                    "src/pipeline.rs",
                ),
                node(
                    "fn:kernel_add",
                    "Function",
                    "kernel_add",
                    "src/kernels/math.rs",
                ),
            ]
            .into_iter(),
        )
        .unwrap();
    writer.add_chunks(sample_chunks().into_iter()).unwrap();
    writer.commit().unwrap();
    drop(writer); // 释放写锁，模拟 analyze 结束
    SearchIndex::open(dir).unwrap()
}

#[test]
fn pipeline_repo_recalls_and_ranks_first() {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());

    let hits = index.search("pipeline repo", 10).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].node_id, "fn:runPipelineFromRepo");
    // 同一 node_id（node 文档 + chunk 文档）必须去重。
    let unique: std::collections::HashSet<&str> = hits.iter().map(|h| h.node_id.as_str()).collect();
    assert_eq!(unique.len(), hits.len(), "duplicate node_id in results");
    // 去重归并后元数据完整：name 来自 node 文档，snippet 来自 chunk 文档。
    assert_eq!(hits[0].name, "runPipelineFromRepo");
    assert!(hits[0].snippet.is_some());
    assert_eq!(hits[0].file_path, "src/pipeline.rs");
    // label 必须是节点真实 label（不被 chunk kind 污染）；kind 保留切块类型。
    assert_eq!(hits[0].label, "Function");
    assert_eq!(hits[0].kind.as_deref(), Some("function"));
}

#[test]
fn chunk_label_carries_node_label_with_kind_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = SearchIndexWriter::create(dir.path()).unwrap();
    writer
        .add_nodes(
            vec![node(
                "const:emitArtifacts",
                "Const",
                "emitArtifacts",
                "src/emit.ts",
            )]
            .into_iter(),
        )
        .unwrap();
    writer
        .add_chunks(
            vec![
                // 有所属节点：label = 节点真实 label，kind = 切块类型。
                ChunkRec {
                    node_id: "const:emitArtifacts".to_owned(),
                    kind: "char".to_owned(),
                    file_path: "src/emit.ts".to_owned(),
                    start_line: 1,
                    end_line: 3,
                    text: "const emitArtifacts = quixotic_marker_alpha".to_owned(),
                },
                // 孤儿 chunk（找不到所属节点）：label 回落 chunk kind。
                ChunkRec {
                    node_id: "ghost:orphanChunk".to_owned(),
                    kind: "ast-declaration".to_owned(),
                    file_path: "src/ghost.ts".to_owned(),
                    start_line: 9,
                    end_line: 12,
                    text: "let orphan = quixotic_marker_beta".to_owned(),
                },
            ]
            .into_iter(),
        )
        .unwrap();
    writer.commit().unwrap();
    drop(writer);

    let index = SearchIndex::open(dir.path()).unwrap();
    let hits = index.search("quixotic marker alpha", 5).unwrap();
    let hit = hits
        .iter()
        .find(|h| h.node_id == "const:emitArtifacts")
        .unwrap();
    assert_eq!(
        hit.label, "Const",
        "chunk 命中必须携带节点真实 label，而非 chunk kind"
    );
    assert_eq!(
        hit.kind.as_deref(),
        Some("char"),
        "切块类型保留在 kind 字段"
    );

    let hits = index.search("quixotic marker beta", 5).unwrap();
    let hit = hits
        .iter()
        .find(|h| h.node_id == "ghost:orphanChunk")
        .unwrap();
    assert_eq!(
        hit.label, "ast-declaration",
        "孤儿 chunk 的 label 回落 chunk kind"
    );
    assert_eq!(hit.kind.as_deref(), Some("ast-declaration"));
}

#[test]
fn node_and_chunk_double_hit_merges_into_one() {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());
    // "knowledge graph" 同时命中 class:KnowledgeGraph 的 node 文档与 chunk 文档。
    let hits = index.search("knowledge graph", 10).unwrap();
    let dup: Vec<&str> = hits
        .iter()
        .filter(|h| h.node_id == "class:KnowledgeGraph")
        .map(|h| h.node_id.as_str())
        .collect();
    assert_eq!(
        dup.len(),
        1,
        "同一符号 id 的 node/chunk 双命中必须合并成一条"
    );
    let merged = hits
        .iter()
        .find(|h| h.node_id == "class:KnowledgeGraph")
        .unwrap();
    // 合并保留双方信息：node 侧 name/label + chunk 侧 snippet/kind。
    assert_eq!(merged.name, "KnowledgeGraph");
    assert_eq!(merged.label, "Class");
    assert!(merged.snippet.is_some());
}

#[test]
fn kernel_add_recalled() {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());

    let hits = index.search("kernel add", 10).unwrap();
    assert_eq!(hits[0].node_id, "fn:kernel_add");
}

#[test]
fn full_identifier_query_matches() {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());

    let hits = index.search("runPipelineFromRepo", 5).unwrap();
    assert_eq!(hits[0].node_id, "fn:runPipelineFromRepo");

    let hits = index.search("KnowledgeGraph", 5).unwrap();
    assert_eq!(hits[0].node_id, "class:KnowledgeGraph");
}

#[test]
fn fuzzy_fallback_catches_typo() {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());

    // "pipelne" 距 "pipeline" 编辑距离 1，短 query 应触发 fuzzy 兜底。
    let hits = index.search("pipelne", 10).unwrap();
    assert!(
        hits.iter().any(|h| h.node_id == "fn:runPipelineFromRepo"),
        "fuzzy fallback should recall runPipelineFromRepo, got {:?}",
        hits.iter().map(|h| &h.node_id).collect::<Vec<_>>()
    );
}

#[test]
fn subword_of_uppercase_run_matches() {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());

    let hits = index.search("http server", 10).unwrap();
    assert_eq!(hits[0].node_id, "class:HTTPServer");
}

#[test]
fn reopen_then_search_and_append() {
    let dir = tempfile::tempdir().unwrap();
    {
        let index = build_index(dir.path());
        drop(index);
    }
    let index = SearchIndex::open(dir.path()).unwrap();
    let hits = index.search("pipeline repo", 10).unwrap();
    assert_eq!(hits[0].node_id, "fn:runPipelineFromRepo");

    // 增量追加（writer 重开）后新文档对 reload 过的读句柄可检索。
    let mut writer = SearchIndexWriter::open(dir.path()).unwrap();
    writer
        .add_chunks(std::iter::once(chunk(
            "fn:freshlyAddedSymbol",
            "src/fresh.rs",
            "fn freshlyAddedSymbol() { zanzibar_quokka() }",
        )))
        .unwrap();
    writer.commit().unwrap();
    drop(writer);
    index.reload().unwrap();
    let hits = index.search("zanzibar quokka", 5).unwrap();
    assert_eq!(hits[0].node_id, "fn:freshlyAddedSymbol");
}

/// 缺陷回归：读路径不得持有 tantivy 写锁。
/// 旧实现里 `SearchIndex::open` 无条件创建 `IndexWriter`，第二个读句柄
/// （如 serve 运行时再起 mcp 进程）必报 `LockBusy`。
#[test]
fn concurrent_readers_do_not_hold_write_lock() {
    let dir = tempfile::tempdir().unwrap();
    let reader_a = build_index(dir.path());

    // 两个只读句柄并存（模拟 serve + mcp 两个进程同时打开）。
    let reader_b = SearchIndex::open(dir.path()).expect("第二个只读句柄不应撞写锁");
    assert_eq!(
        reader_a.search("pipeline repo", 5).unwrap()[0].node_id,
        "fn:runPipelineFromRepo"
    );
    assert_eq!(
        reader_b.search("pipeline repo", 5).unwrap()[0].node_id,
        "fn:runPipelineFromRepo"
    );

    // 读句柄开着的同时，写句柄仍可取锁写入（serve 后台导入/更新场景）。
    let mut writer = SearchIndexWriter::open(dir.path()).expect("读句柄不应阻塞写锁");
    writer
        .add_chunks(std::iter::once(chunk(
            "fn:hotAppended",
            "src/hot.rs",
            "fn hotAppended() { xylophone_wombat() }",
        )))
        .unwrap();
    writer.commit().unwrap();
    drop(writer); // 写锁释放后……

    // ……新写句柄可立刻再取锁（无常驻持锁者）。
    drop(SearchIndexWriter::open(dir.path()).expect("写锁应已释放"));

    // 旧读句柄 reload 后能看到写入。
    reader_a.reload().unwrap();
    reader_b.reload().unwrap();
    assert_eq!(
        reader_a.search("xylophone wombat", 5).unwrap()[0].node_id,
        "fn:hotAppended"
    );
    assert_eq!(
        reader_b.search("xylophone wombat", 5).unwrap()[0].node_id,
        "fn:hotAppended"
    );
}

#[test]
fn empty_query_and_zero_limit() {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());
    assert!(index.search("", 10).unwrap().is_empty());
    assert!(index.search("   ::: ", 10).unwrap().is_empty());
    assert!(index.search("pipeline", 0).unwrap().is_empty());
}

#[test]
fn exact_symbol_beats_container_path_matches() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = SearchIndexWriter::create(dir.path()).unwrap();
    writer
        .add_nodes(
            vec![
                node(
                    "file:PromptServer",
                    "File",
                    "PromptServer",
                    "src/PromptServer.ts",
                ),
                node(
                    "folder:prompt-server",
                    "Folder",
                    "PromptServer",
                    "src/PromptServer",
                ),
                node(
                    "class:PromptServer",
                    "Class",
                    "PromptServer",
                    "src/server.ts",
                ),
            ]
            .into_iter(),
        )
        .unwrap();
    writer
        .add_chunks(
            vec![ChunkRec {
                node_id: "class:PromptServer".to_owned(),
                kind: "ast-class".to_owned(),
                file_path: "src/server.ts".to_owned(),
                start_line: 42,
                end_line: 80,
                text: "class PromptServer { async startPromptServer() {} }".to_owned(),
            }]
            .into_iter(),
        )
        .unwrap();
    writer.commit().unwrap();
    drop(writer);

    let index = SearchIndex::open(dir.path()).unwrap();
    let hits = index.search("PromptServer", 10).unwrap();
    assert_eq!(hits[0].node_id, "class:PromptServer");
    assert_eq!(hits[0].label, "Class");
}

#[test]
fn generated_vendor_json_is_demoted_below_source_symbol() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = SearchIndexWriter::create(dir.path()).unwrap();
    writer
        .add_nodes(
            vec![
                node(
                    "fn:loadTokenizer",
                    "Function",
                    "loadTokenizer",
                    "src/tokenizer.ts",
                ),
                node(
                    "data:tokenizer",
                    "Resource",
                    "tokenizer",
                    "node_modules/model/tokenizer.json",
                ),
            ]
            .into_iter(),
        )
        .unwrap();
    writer
        .add_chunks(
            vec![
                ChunkRec {
                    node_id: "fn:loadTokenizer".to_owned(),
                    kind: "ast-function".to_owned(),
                    file_path: "src/tokenizer.ts".to_owned(),
                    start_line: 3,
                    end_line: 12,
                    text: "function loadTokenizer() { return parseTokenizerConfig() }".to_owned(),
                },
                ChunkRec {
                    node_id: "data:tokenizer".to_owned(),
                    kind: "char".to_owned(),
                    file_path: "node_modules/model/tokenizer.json".to_owned(),
                    start_line: 1,
                    end_line: 20,
                    text: "tokenizer tokenizer tokenizer tokenizer tokenizer parse config"
                        .to_owned(),
                },
            ]
            .into_iter(),
        )
        .unwrap();
    writer.commit().unwrap();
    drop(writer);

    let index = SearchIndex::open(dir.path()).unwrap();
    let hits = index.search("tokenizer parse config", 10).unwrap();
    assert_eq!(hits[0].node_id, "fn:loadTokenizer");
    assert!(
        hits.iter()
            .position(|h| h.node_id == "data:tokenizer")
            .unwrap()
            > hits
                .iter()
                .position(|h| h.node_id == "fn:loadTokenizer")
                .unwrap(),
        "generated/vendor JSON should not outrank source symbols: {hits:?}"
    );
}

#[test]
fn plain_json_resource_is_demoted_below_source_symbol() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = SearchIndexWriter::create(dir.path()).unwrap();
    writer
        .add_nodes(
            vec![
                node("fn:parsePolicy", "Function", "parsePolicy", "src/policy.ts"),
                node(
                    "resource:policy-schema",
                    "Resource",
                    "policy schema",
                    "src/policy.schema.json",
                ),
            ]
            .into_iter(),
        )
        .unwrap();
    writer
        .add_chunks(
            vec![
                ChunkRec {
                    node_id: "fn:parsePolicy".to_owned(),
                    kind: "ast-function".to_owned(),
                    file_path: "src/policy.ts".to_owned(),
                    start_line: 8,
                    end_line: 18,
                    text: "function parsePolicy(input: unknown) { return validatePolicy(input) }"
                        .to_owned(),
                },
                ChunkRec {
                    node_id: "resource:policy-schema".to_owned(),
                    kind: "char".to_owned(),
                    file_path: "src/policy.schema.json".to_owned(),
                    start_line: 1,
                    end_line: 60,
                    text: "policy policy policy parse validate schema schema schema".to_owned(),
                },
            ]
            .into_iter(),
        )
        .unwrap();
    writer.commit().unwrap();
    drop(writer);

    let index = SearchIndex::open(dir.path()).unwrap();
    let hits = index.search("policy parse validate", 10).unwrap();
    assert_eq!(hits[0].node_id, "fn:parsePolicy");
    assert!(
        hits.iter()
            .position(|h| h.node_id == "resource:policy-schema")
            .unwrap()
            > hits
                .iter()
                .position(|h| h.node_id == "fn:parsePolicy")
                .unwrap(),
        "plain JSON resources should not outrank source symbols: {hits:?}"
    );
}

#[test]
fn noisy_json_pool_cannot_hide_clean_source_symbol() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = SearchIndexWriter::create(dir.path()).unwrap();
    let mut nodes = vec![node(
        "fn:resolvePolicy",
        "Function",
        "resolvePolicy",
        "src/policy.ts",
    )];
    for i in 0..80 {
        nodes.push(node(
            &format!("resource:policy:{i}"),
            "Resource",
            "policy schema",
            &format!("node_modules/pkg{i}/policy.schema.json"),
        ));
    }
    writer.add_nodes(nodes.into_iter()).unwrap();

    let mut chunks = vec![ChunkRec {
        node_id: "fn:resolvePolicy".to_owned(),
        kind: "ast-function".to_owned(),
        file_path: "src/policy.ts".to_owned(),
        start_line: 4,
        end_line: 12,
        text: "function resolvePolicy() { return parsePolicyConfig() }".to_owned(),
    }];
    for i in 0..80 {
        chunks.push(ChunkRec {
            node_id: format!("resource:policy:{i}"),
            kind: "char".to_owned(),
            file_path: format!("node_modules/pkg{i}/policy.schema.json"),
            start_line: 1,
            end_line: 20,
            text: "policy policy policy policy policy parse config".to_owned(),
        });
    }
    writer.add_chunks(chunks.into_iter()).unwrap();
    writer.commit().unwrap();
    drop(writer);

    let index = SearchIndex::open(dir.path()).unwrap();
    let hits = index.search("policy parse config", 5).unwrap();
    assert_eq!(
        hits.first().map(|h| h.node_id.as_str()),
        Some("fn:resolvePolicy"),
        "clean source symbol must not be hidden by noisy JSON candidates: {hits:?}"
    );
}
