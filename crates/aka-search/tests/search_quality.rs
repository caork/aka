//! 检索质量测试：手造 ~20 个 chunk + 若干 node，验证召回与排序。

use aka_core::types::{ChunkRec, NodeRec};
use aka_search::SearchIndex;

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
    let mut index = SearchIndex::create(dir).unwrap();
    index.add_chunks(sample_chunks().into_iter()).unwrap();
    index
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
                node("fn:kernel_add", "Function", "kernel_add", "src/kernels/math.rs"),
            ]
            .into_iter(),
        )
        .unwrap();
    index.commit().unwrap();
    index
}

#[test]
fn pipeline_repo_recalls_and_ranks_first() {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());

    let hits = index.search("pipeline repo", 10).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].node_id, "fn:runPipelineFromRepo");
    // 同一 node_id（node 文档 + chunk 文档）必须去重。
    let unique: std::collections::HashSet<&str> =
        hits.iter().map(|h| h.node_id.as_str()).collect();
    assert_eq!(unique.len(), hits.len(), "duplicate node_id in results");
    // 去重归并后元数据完整：name 来自 node 文档，snippet 来自 chunk 文档。
    assert_eq!(hits[0].name, "runPipelineFromRepo");
    assert!(hits[0].snippet.is_some());
    assert_eq!(hits[0].file_path, "src/pipeline.rs");
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
    let mut index = SearchIndex::open(dir.path()).unwrap();
    let hits = index.search("pipeline repo", 10).unwrap();
    assert_eq!(hits[0].node_id, "fn:runPipelineFromRepo");

    // 增量追加后新文档可检索。
    index
        .add_chunks(
            std::iter::once(chunk(
                "fn:freshlyAddedSymbol",
                "src/fresh.rs",
                "fn freshlyAddedSymbol() { zanzibar_quokka() }",
            )),
        )
        .unwrap();
    index.commit().unwrap();
    let hits = index.search("zanzibar quokka", 5).unwrap();
    assert_eq!(hits[0].node_id, "fn:freshlyAddedSymbol");
}

#[test]
fn empty_query_and_zero_limit() {
    let dir = tempfile::tempdir().unwrap();
    let index = build_index(dir.path());
    assert!(index.search("", 10).unwrap().is_empty());
    assert!(index.search("   ::: ", 10).unwrap().is_empty());
    assert!(index.search("pipeline", 0).unwrap().is_empty());
}
