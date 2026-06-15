//! BM25 全文索引 — tantivy 实现。
//!
//! 读写分离：[`SearchIndex`] 是**只读**查询句柄（只持 `Index` + reader，不取
//! tantivy 写锁，多进程可并发打开）；[`SearchIndexWriter`] 才持有 `IndexWriter`
//! （目录级独占锁），仅在 analyze / ingest 期间短暂存活，drop 即释放锁。

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, STORED, STRING,
};
use tantivy::snippet::SnippetGenerator;
use tantivy::tokenizer::{LowerCaser, SimpleTokenizer, TextAnalyzer, TokenStream};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

use aka_core::types::{ChunkRec, NodeRec};

use crate::tokenizer::{CodeTokenizer, CODE_TOKENIZER_NAME};
use crate::Result;

/// file_path 字段的 tokenizer 名（按非字母数字字符拆分，如 `/` `.`）。
const PATH_TOKENIZER_NAME: &str = "path";
const PATH_CLASS_CLEAN: &str = "clean";
const PATH_CLASS_NOISY: &str = "noisy";
/// name 字段查询时的权重倍数。
const NAME_BOOST: f32 = 3.0;
/// text 字段存储截断上限（字节），用于 snippet。
const TEXT_STORE_LIMIT: usize = 2048;
/// snippet 最大字符数。
const SNIPPET_MAX_CHARS: usize = 160;
/// 触发 fuzzy 兜底的查询词数上限（词数 < 该值时追加 fuzzy 子查询）。
const FUZZY_WORD_LIMIT: usize = 3;
/// fuzzy 子查询只对长度 ≥ 该值的 term 生效（太短的 term fuzzy 噪声过大）。
const FUZZY_MIN_TERM_CHARS: usize = 3;
/// IndexWriter 总内存预算（2 线程平分）。
const WRITER_MEM_BUDGET: usize = 64 * 1024 * 1024;
const WRITER_THREADS: usize = 2;
/// label 是真实代码符号时的轻量加权；文件/目录类节点不吃这档。
const SYMBOL_LABEL_BOOST: f32 = 1.20;
/// exact symbol name 命中比仅命中文本/路径更可信。
const EXACT_NAME_BOOST: f32 = 2.40;
/// query tokens 全部覆盖 name 时给一个中间档，提升 `prompt server` 这类查类名的体验。
const NAME_TOKEN_COVERAGE_BOOST: f32 = 1.45;
/// File / Folder / Project 这类容器节点通常是导航结果，不应压过真实符号。
const CONTAINER_LABEL_PENALTY: f32 = 0.45;
/// generated/vendor/dist/json/lockfile 等路径噪声降权。
const NOISY_PATH_PENALTY: f32 = 0.35;

/// 一条检索命中结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    /// 图谱节点 id（chunk 命中会归并到所属节点）。
    pub node_id: String,
    /// 分数：BM25 检索时为 tantivy 打分；经 [`crate::rrf_merge`] 后为 RRF 融合分。
    pub score: f32,
    /// 节点名（来自 nodes 的 `name` 属性；纯 chunk 命中可能为空串）。
    pub name: String,
    /// 文件路径。
    pub file_path: String,
    /// 节点真实 label（Function / Class / Const …）。chunk 文档入索引时携带
    /// 所属节点的 label，仅当 ingest 会话查不到所属节点时回落 chunk kind。
    pub label: String,
    /// 切块类型（ast-function / ast-declaration / char …）；纯 node 命中为 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 命中片段（HTML，命中词用 `<b>` 包裹）；text 为空或无命中词时为 `None`。
    pub snippet: Option<String>,
    /// 起始行号（未知时为 0）。
    pub start_line: u32,
}

/// schema 字段句柄集合。
#[derive(Clone, Copy)]
struct Fields {
    node_id: Field,
    name: Field,
    text: Field,
    file_path: Field,
    /// Exact, untokenized path for file-scoped deletes; old indexes may lack it.
    path_exact: Option<Field>,
    label: Field,
    /// 路径噪声分类字段；旧版索引（schema 无 `path_class`）打开时为 `None`。
    path_class: Option<Field>,
    /// 切块类型字段；旧版索引（schema 无 `kind`）打开时为 `None`。
    kind: Option<Field>,
    start_line: Field,
    end_line: Field,
}

/// BM25 全文索引的**只读**查询句柄。
///
/// schema：`node_id`(STRING stored) / `name`(TEXT code-tokenizer，查询权重 x3) /
/// `text`(TEXT code-tokenizer，存储截断 2KB 供 snippet) / `file_path`(TEXT 按
/// `/` `.` 等拆分) / `label`(STRING stored) / `kind`(STRING stored，chunk 切块类型) /
/// `start_line` `end_line`(u64 stored)。
///
/// 只持 `Index` + `IndexReader`，**不取 tantivy 写锁**——任意多个进程可同时打开
/// 同一目录做查询（serve 与 mcp 并存）。写入走 [`SearchIndexWriter`]。
pub struct SearchIndex {
    index: Index,
    reader: IndexReader,
    fields: Fields,
}

/// BM25 全文索引的写入句柄（持目录级独占写锁，ingest 用完即 drop 释放）。
///
/// 写入端基于 tantivy 原生段合并，增量友好：多次 `add_*` + [`commit`](Self::commit)
/// 即可追加；重新 [`open`](Self::open) 后继续写入。
///
/// [`add_nodes`](Self::add_nodes) 会在会话内记录 node_id → label 映射，供随后的
/// [`add_chunks`](Self::add_chunks) 给 chunk 文档盖上所属节点的真实 label——
/// 因此 ingest 必须**先 add_nodes 再 add_chunks**（工件管线本就如此）。
pub struct SearchIndexWriter {
    writer: IndexWriter<TantivyDocument>,
    fields: Fields,
    /// ingest 会话内的 node_id → label 映射（节点先于 chunk 摄取）。
    node_labels: HashMap<String, String>,
}

fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    let code_indexing = TextFieldIndexing::default()
        .set_tokenizer(CODE_TOKENIZER_NAME)
        .set_index_option(IndexRecordOption::WithFreqsAndPositions);
    let path_indexing = TextFieldIndexing::default()
        .set_tokenizer(PATH_TOKENIZER_NAME)
        .set_index_option(IndexRecordOption::WithFreqs);

    builder.add_text_field("node_id", STRING | STORED);
    builder.add_text_field(
        "name",
        TextOptions::default()
            .set_indexing_options(code_indexing.clone())
            .set_stored(),
    );
    builder.add_text_field(
        "text",
        TextOptions::default()
            .set_indexing_options(code_indexing)
            .set_stored(),
    );
    builder.add_text_field(
        "file_path",
        TextOptions::default()
            .set_indexing_options(path_indexing)
            .set_stored(),
    );
    builder.add_text_field("path_exact", STRING | STORED);
    builder.add_text_field("label", STRING | STORED);
    builder.add_text_field("path_class", STRING | STORED);
    builder.add_text_field("kind", STRING | STORED);
    builder.add_u64_field("start_line", STORED);
    builder.add_u64_field("end_line", STORED);
    builder.build()
}

/// 从（可能是旧版本的）schema 解析字段句柄；`kind` 字段缺失时容忍为 `None`。
fn resolve_fields(schema: &Schema) -> Fields {
    let field = |name: &str| schema.get_field(name).expect("schema field exists");
    Fields {
        node_id: field("node_id"),
        name: field("name"),
        text: field("text"),
        file_path: field("file_path"),
        path_exact: schema.get_field("path_exact").ok(),
        label: field("label"),
        path_class: schema.get_field("path_class").ok(),
        kind: schema.get_field("kind").ok(),
        start_line: field("start_line"),
        end_line: field("end_line"),
    }
}

fn register_tokenizers(index: &Index) {
    index.tokenizers().register(
        CODE_TOKENIZER_NAME,
        TextAnalyzer::builder(CodeTokenizer).build(),
    );
    index.tokenizers().register(
        PATH_TOKENIZER_NAME,
        TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(LowerCaser)
            .build(),
    );
}

impl SearchIndexWriter {
    /// 在 `dir` 下新建索引（目录不存在会创建；已有索引则报错）。
    pub fn create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let index = Index::create_in_dir(dir, build_schema())?;
        Self::finish_open(index)
    }

    /// 打开 `dir` 下既有索引继续追加写入。
    ///
    /// 注意：node_id → label 映射只在会话内有效，追加 chunk 时若其节点是
    /// 此前会话写入的，label 回落 chunk kind。
    pub fn open(dir: &Path) -> Result<Self> {
        let index = Index::open_in_dir(dir)?;
        Self::finish_open(index)
    }

    fn finish_open(index: Index) -> Result<Self> {
        register_tokenizers(&index);
        let fields = resolve_fields(&index.schema());
        let writer = index.writer_with_num_threads(WRITER_THREADS, WRITER_MEM_BUDGET)?;
        Ok(Self {
            writer,
            fields,
            node_labels: HashMap::new(),
        })
    }

    /// 索引图谱节点：用 `name` / `filePath` / `label` 建文档（`text` 留空），
    /// 并记录 node_id → label 供 [`add_chunks`](Self::add_chunks) 查询。
    ///
    /// 写入后需调用 [`commit`](Self::commit) 才对检索可见。
    pub fn add_nodes(&mut self, nodes: impl Iterator<Item = NodeRec>) -> Result<()> {
        for node in nodes {
            let mut doc = TantivyDocument::new();
            doc.add_text(self.fields.node_id, &node.id);
            if let Some(name) = node.name() {
                doc.add_text(self.fields.name, name);
            }
            if let Some(fp) = node.file_path() {
                doc.add_text(self.fields.file_path, fp);
                if let Some(path_exact) = self.fields.path_exact {
                    doc.add_text(path_exact, fp);
                }
                if let Some(path_class) = self.fields.path_class {
                    doc.add_text(path_class, path_class_value(fp));
                }
            } else if let Some(path_class) = self.fields.path_class {
                doc.add_text(path_class, PATH_CLASS_CLEAN);
            }
            let search_text = node_search_text(&node);
            if !search_text.is_empty() {
                doc.add_text(
                    self.fields.text,
                    truncate_utf8(&search_text, TEXT_STORE_LIMIT),
                );
            }
            doc.add_text(self.fields.label, &node.label);
            /* 行号统一存 1-based（工件是 tree-sitter 0-based row） */
            if let Some(line) = node.start_line_1based() {
                doc.add_u64(self.fields.start_line, u64::from(line));
            }
            if let Some(line) = node.end_line_1based() {
                doc.add_u64(self.fields.end_line, u64::from(line));
            }
            self.node_labels.insert(node.id.clone(), node.label.clone());
            self.writer.add_document(doc)?;
        }
        Ok(())
    }

    /// 索引代码切块：`text` = chunk 正文（截断到 2KB），`label` = 所属节点的
    /// 真实 label（本会话 [`add_nodes`](Self::add_nodes) 记录的映射；查不到时
    /// 回落 chunk kind），`kind` = chunk kind（切块策略：ast-function / char …）。
    ///
    /// 写入后需调用 [`commit`](Self::commit) 才对检索可见。
    pub fn add_chunks(&mut self, chunks: impl Iterator<Item = ChunkRec>) -> Result<()> {
        for chunk in chunks {
            let mut doc = TantivyDocument::new();
            doc.add_text(self.fields.node_id, &chunk.node_id);
            doc.add_text(
                self.fields.text,
                truncate_utf8(&chunk.text, TEXT_STORE_LIMIT),
            );
            doc.add_text(self.fields.file_path, &chunk.file_path);
            if let Some(path_exact) = self.fields.path_exact {
                doc.add_text(path_exact, &chunk.file_path);
            }
            if let Some(path_class) = self.fields.path_class {
                doc.add_text(path_class, path_class_value(&chunk.file_path));
            }
            let label = self
                .node_labels
                .get(&chunk.node_id)
                .map(String::as_str)
                .unwrap_or(&chunk.kind);
            doc.add_text(self.fields.label, label);
            if let Some(kind) = self.fields.kind {
                doc.add_text(kind, &chunk.kind);
            }
            doc.add_u64(self.fields.start_line, u64::from(chunk.start_line_1based()));
            doc.add_u64(self.fields.end_line, u64::from(chunk.end_line_1based()));
            self.writer.add_document(doc)?;
        }
        Ok(())
    }

    /// Whether this index schema supports exact file-scoped deletion.
    ///
    /// Indexes created before `path_exact` existed cannot safely delete by the
    /// tokenized `file_path` field, so callers should fall back to a full rebuild.
    pub fn supports_file_deletes(&self) -> bool {
        self.fields.path_exact.is_some()
    }

    /// Delete all committed and same-transaction documents owned by `file_path`.
    ///
    /// Returns `Ok(false)` when opening an old index whose schema lacks the
    /// exact path field; callers should rebuild the search index in that case.
    pub fn delete_file(&mut self, file_path: &str) -> Result<bool> {
        let Some(path_exact) = self.fields.path_exact else {
            return Ok(false);
        };
        self.writer
            .delete_term(Term::from_field_text(path_exact, file_path));
        Ok(true)
    }

    /// 提交写入（tantivy 后台自动做段合并）。写锁在 `self` drop 时释放。
    pub fn commit(&mut self) -> Result<()> {
        self.writer.commit()?;
        Ok(())
    }
}

impl SearchIndex {
    /// 以只读方式打开 `dir` 下既有索引（不取写锁，可与写入端/其他读取端并存）。
    pub fn open(dir: &Path) -> Result<Self> {
        let index = Index::open_in_dir(dir)?;
        register_tokenizers(&index);
        let fields = resolve_fields(&index.schema());
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            fields,
        })
    }

    /// 重新加载 reader，使打开之后其他句柄提交的写入对检索可见。
    pub fn reload(&self) -> Result<()> {
        self.reader.reload()?;
        Ok(())
    }

    /// BM25 检索。
    ///
    /// 查询策略：query 经代码感知 tokenizer 切词后各 term 之间 OR；每个 term 同时查
    /// `name`（权重 x3）/ `text` / `file_path` 三个字段；短 query（< 3 个词）额外追加
    /// distance=1 的 fuzzy 子查询兜底拼写误差。同一 `node_id` 多文档命中去重取最高分，
    /// 并用低分文档补全缺失的元数据 / snippet。
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let terms = query_terms(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }

        let searcher = self.reader.searcher();
        // 多抓一些以便同 node_id 去重后仍能凑满 limit。
        let fetch = limit.saturating_mul(4).max(limit.saturating_add(16));

        let normalized_query = normalize_for_rank(query);
        let query_tokens = terms.clone();
        let mut order: Vec<String> = Vec::new();
        let mut best: HashMap<String, Hit> = HashMap::new();
        if self.fields.path_class.is_some() {
            let clean_query = build_query(self.fields, query, &terms, Some(PATH_CLASS_CLEAN));
            self.collect_query_hits(&searcher, &clean_query, fetch, &mut order, &mut best)?;
            if best.len() < limit {
                let noisy_query = build_query(self.fields, query, &terms, Some(PATH_CLASS_NOISY));
                self.collect_query_hits(&searcher, &noisy_query, fetch, &mut order, &mut best)?;
            }
        } else {
            let bool_query = build_query(self.fields, query, &terms, None);
            self.collect_query_hits(&searcher, &bool_query, fetch, &mut order, &mut best)?;
        }

        let mut hits: Vec<Hit> = order
            .into_iter()
            .filter_map(|id| best.remove(&id))
            .map(|mut hit| {
                hit.score = rerank_score(hit.score, &hit, &normalized_query, &query_tokens);
                hit
            })
            .collect();
        hits.sort_by(|a, b| {
            rank_bucket(a, &normalized_query, &query_tokens)
                .cmp(&rank_bucket(b, &normalized_query, &query_tokens))
                .then_with(|| b.score.total_cmp(&a.score))
                .then_with(|| a.file_path.cmp(&b.file_path))
                .then_with(|| a.start_line.cmp(&b.start_line))
                .then_with(|| a.node_id.cmp(&b.node_id))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    fn collect_query_hits(
        &self,
        searcher: &tantivy::Searcher,
        query: &BooleanQuery,
        fetch: usize,
        order: &mut Vec<String>,
        best: &mut HashMap<String, Hit>,
    ) -> Result<()> {
        let top_docs = searcher.search(query, &TopDocs::with_limit(fetch).order_by_score())?;
        let mut snippet_gen = SnippetGenerator::create(searcher, query, self.fields.text)?;
        snippet_gen.set_max_num_chars(SNIPPET_MAX_CHARS);
        for (score, addr) in top_docs {
            let doc: TantivyDocument = searcher.doc(addr)?;
            let Some(node_id) = str_field(&doc, self.fields.node_id) else {
                continue;
            };
            match best.entry(node_id.clone()) {
                Entry::Vacant(slot) => {
                    order.push(node_id);
                    slot.insert(self.make_hit(score, &doc, &snippet_gen));
                }
                // top_docs 按分数降序，首次出现即该 node_id 在本轮查询的最高分；
                // 后续重复文档只用来补全缺失字段。
                Entry::Occupied(mut slot) => {
                    self.fill_missing(slot.get_mut(), &doc, &snippet_gen);
                }
            }
        }
        Ok(())
    }

    fn make_hit(&self, score: f32, doc: &TantivyDocument, snippets: &SnippetGenerator) -> Hit {
        Hit {
            node_id: str_field(doc, self.fields.node_id).unwrap_or_default(),
            score,
            name: str_field(doc, self.fields.name).unwrap_or_default(),
            file_path: str_field(doc, self.fields.file_path).unwrap_or_default(),
            label: str_field(doc, self.fields.label).unwrap_or_default(),
            kind: self.fields.kind.and_then(|f| str_field(doc, f)),
            snippet: make_snippet(doc, snippets),
            start_line: u64_field(doc, self.fields.start_line).unwrap_or(0) as u32,
        }
    }

    /// 用同 node_id 的低分文档补全缺失字段（如 chunk 命中缺 name、node 命中缺 snippet）。
    fn fill_missing(&self, hit: &mut Hit, doc: &TantivyDocument, snippets: &SnippetGenerator) {
        if hit.name.is_empty() {
            if let Some(name) = str_field(doc, self.fields.name) {
                hit.name = name;
            }
        }
        if hit.file_path.is_empty() {
            if let Some(fp) = str_field(doc, self.fields.file_path) {
                hit.file_path = fp;
            }
        }
        if hit.label.is_empty() {
            if let Some(label) = str_field(doc, self.fields.label) {
                hit.label = label;
            }
        }
        if hit.kind.is_none() {
            hit.kind = self.fields.kind.and_then(|f| str_field(doc, f));
        }
        if hit.snippet.is_none() {
            hit.snippet = make_snippet(doc, snippets);
        }
        if hit.start_line == 0 {
            if let Some(line) = u64_field(doc, self.fields.start_line) {
                hit.start_line = line as u32;
            }
        }
    }

    /// 底层 tantivy [`Index`]（高级用法：自定义 collector、统计等）。
    pub fn tantivy_index(&self) -> &Index {
        &self.index
    }

    /// 当前可检索的文档数（含 node 与 chunk 文档，未 commit 的不计）。
    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }
}

/// 用代码感知 tokenizer 切 query，返回去重后的小写 term 列表（保持出现顺序）。
fn query_terms(query: &str) -> Vec<String> {
    let mut analyzer = TextAnalyzer::builder(CodeTokenizer).build();
    let mut stream = analyzer.token_stream(query);
    let mut seen = Vec::new();
    while stream.advance() {
        let text = &stream.token().text;
        if !seen.iter().any(|t| t == text) {
            seen.push(text.clone());
        }
    }
    expand_query_terms(&mut seen);
    seen
}

fn expand_query_terms(terms: &mut Vec<String>) {
    let original = terms.clone();
    for term in original {
        let expansions: &[&str] = match term.as_str() {
            "jwt" => &["token", "claims"],
            "authentication" | "auth" | "authenticate" => {
                &["token", "security", "credential", "login"]
            }
            "authorization" | "authorize" => &["permission", "role", "scope"],
            _ => &[],
        };
        for expansion in expansions {
            if !terms.iter().any(|t| t == expansion) {
                terms.push((*expansion).to_string());
            }
        }
    }
}

fn build_query(
    fields: Fields,
    raw_query: &str,
    terms: &[String],
    path_class: Option<&str>,
) -> BooleanQuery {
    let mut lexical: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    for term in terms {
        let name_term = TermQuery::new(
            Term::from_field_text(fields.name, term),
            IndexRecordOption::WithFreqs,
        );
        lexical.push((
            Occur::Should,
            Box::new(BoostQuery::new(Box::new(name_term), NAME_BOOST)),
        ));
        lexical.push((
            Occur::Should,
            Box::new(TermQuery::new(
                Term::from_field_text(fields.text, term),
                IndexRecordOption::WithFreqs,
            )),
        ));
        lexical.push((
            Occur::Should,
            Box::new(TermQuery::new(
                Term::from_field_text(fields.file_path, term),
                IndexRecordOption::WithFreqs,
            )),
        ));
    }
    if raw_query.split_whitespace().count() < FUZZY_WORD_LIMIT {
        for term in terms {
            if term.chars().count() < FUZZY_MIN_TERM_CHARS {
                continue;
            }
            lexical.push((
                Occur::Should,
                Box::new(FuzzyTermQuery::new(
                    Term::from_field_text(fields.name, term),
                    1,
                    true,
                )),
            ));
            lexical.push((
                Occur::Should,
                Box::new(FuzzyTermQuery::new(
                    Term::from_field_text(fields.text, term),
                    1,
                    true,
                )),
            ));
        }
    }
    let lexical_query = BooleanQuery::new(lexical);
    if let (Some(field), Some(class)) = (fields.path_class, path_class) {
        return BooleanQuery::new(vec![
            (Occur::Must, Box::new(lexical_query) as Box<dyn Query>),
            (
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_text(field, class),
                    IndexRecordOption::Basic,
                )),
            ),
        ]);
    }
    lexical_query
}

fn rerank_score(base: f32, hit: &Hit, normalized_query: &str, query_tokens: &[String]) -> f32 {
    let mut factor = 1.0;
    if is_symbol_label(&hit.label) {
        factor *= SYMBOL_LABEL_BOOST;
    }
    if is_container_label(&hit.label) {
        factor *= CONTAINER_LABEL_PENALTY;
    }
    if is_noisy_path(&hit.file_path) {
        factor *= NOISY_PATH_PENALTY;
    }

    let normalized_name = normalize_for_rank(&hit.name);
    if !normalized_name.is_empty() && normalized_name == normalized_query {
        factor *= EXACT_NAME_BOOST;
    } else if !query_tokens.is_empty() {
        let name_tokens = query_terms(&hit.name);
        if query_tokens
            .iter()
            .all(|q| name_tokens.iter().any(|n| n == q))
        {
            factor *= NAME_TOKEN_COVERAGE_BOOST;
        }
    }

    base * factor
}

fn rank_bucket(hit: &Hit, normalized_query: &str, query_tokens: &[String]) -> u8 {
    if is_noisy_path(&hit.file_path) {
        return 5;
    }
    if is_container_label(&hit.label) {
        return 4;
    }

    let normalized_name = normalize_for_rank(&hit.name);
    if is_symbol_label(&hit.label)
        && !normalized_name.is_empty()
        && normalized_name == normalized_query
    {
        return 0;
    }
    if is_primary_code_symbol(&hit.label) {
        return 1;
    }
    if is_symbol_label(&hit.label) {
        return 2;
    }
    if !query_tokens.is_empty() {
        let name_tokens = query_terms(&hit.name);
        if query_tokens
            .iter()
            .all(|q| name_tokens.iter().any(|n| n == q))
        {
            return 2;
        }
    }
    3
}

fn is_symbol_label(label: &str) -> bool {
    matches!(
        label,
        "Function"
            | "Method"
            | "Class"
            | "Interface"
            | "Struct"
            | "Enum"
            | "Trait"
            | "Type"
            | "Const"
            | "Variable"
            | "Route"
            | "Tool"
            | "Command"
            | "Config"
            | "Job"
            | "Table"
            | "Repository"
            | "Migration"
            | "Cache"
            | "Event"
            | "Policy"
            | "Resource"
            | "Transaction"
    )
}

fn is_primary_code_symbol(label: &str) -> bool {
    matches!(
        label,
        "Function"
            | "Method"
            | "Class"
            | "Interface"
            | "Struct"
            | "Enum"
            | "Trait"
            | "Type"
            | "Route"
            | "Tool"
            | "Command"
            | "Config"
            | "Job"
            | "Table"
            | "Repository"
            | "Migration"
            | "Cache"
            | "Event"
            | "Policy"
            | "Transaction"
    )
}

fn is_container_label(label: &str) -> bool {
    matches!(
        label,
        "File" | "Folder" | "Project" | "Package" | "Module" | "Community" | "Process"
    )
}

fn node_search_text(node: &NodeRec) -> String {
    if !matches!(
        node.label.as_str(),
        "Process"
            | "Route"
            | "Tool"
            | "Command"
            | "Config"
            | "Job"
            | "Table"
            | "Repository"
            | "Migration"
            | "Cache"
            | "Event"
            | "Policy"
            | "Community"
            | "Resource"
            | "Transaction"
    ) {
        return String::new();
    }
    let mut parts = Vec::new();
    push_prop_str(&mut parts, &node.properties, "name");
    push_prop_str(&mut parts, &node.properties, "summary");
    push_prop_str(&mut parts, &node.properties, "processType");
    push_prop_str(&mut parts, &node.properties, "commandType");
    push_prop_str(&mut parts, &node.properties, "key");
    push_prop_str(&mut parts, &node.properties, "configType");
    push_prop_str(&mut parts, &node.properties, "configSource");
    push_prop_str(&mut parts, &node.properties, "valueHint");
    push_prop_str(&mut parts, &node.properties, "jobType");
    push_prop_str(&mut parts, &node.properties, "schedule");
    push_prop_str(&mut parts, &node.properties, "handlerName");
    push_prop_str(&mut parts, &node.properties, "strategy");
    push_prop_str(&mut parts, &node.properties, "tableName");
    push_prop_str(&mut parts, &node.properties, "tableSource");
    push_prop_str(&mut parts, &node.properties, "entityName");
    push_prop_str(&mut parts, &node.properties, "repositorySource");
    push_prop_str(&mut parts, &node.properties, "migrationType");
    push_prop_str(&mut parts, &node.properties, "migrationSource");
    push_prop_str(&mut parts, &node.properties, "version");
    push_prop_str(&mut parts, &node.properties, "backend");
    push_prop_str(&mut parts, &node.properties, "cacheSource");
    push_prop_str(&mut parts, &node.properties, "bus");
    push_prop_str(&mut parts, &node.properties, "eventSource");
    push_prop_str(&mut parts, &node.properties, "policyType");
    push_prop_str(&mut parts, &node.properties, "policySource");
    push_prop_str(&mut parts, &node.properties, "url");
    push_prop_str(&mut parts, &node.properties, "resourceType");
    push_prop_str(&mut parts, &node.properties, "resourceSource");
    push_prop_str(&mut parts, &node.properties, "route");
    push_prop_str(&mut parts, &node.properties, "tool");
    push_prop_array(&mut parts, &node.properties, "trace");
    push_prop_array(&mut parts, &node.properties, "steps");
    push_prop_array(&mut parts, &node.properties, "sources");
    push_prop_array(&mut parts, &node.properties, "columns");
    push_prop_array(&mut parts, &node.properties, "tables");
    push_prop_array(&mut parts, &node.properties, "operations");
    push_prop_array(&mut parts, &node.properties, "responseKeys");
    push_prop_array(&mut parts, &node.properties, "errorKeys");
    push_prop_array(&mut parts, &node.properties, "middleware");
    parts.join(" ")
}

fn push_prop_str(
    parts: &mut Vec<String>,
    props: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) {
    if let Some(value) = props.get(key).and_then(|v| v.as_str()) {
        parts.push(value.to_owned());
    }
}

fn push_prop_array(
    parts: &mut Vec<String>,
    props: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) {
    let Some(values) = props.get(key).and_then(|v| v.as_array()) else {
        return;
    };
    for value in values {
        if let Some(s) = value.as_str() {
            parts.push(s.to_owned());
        } else if let Some(obj) = value.as_object() {
            for key in ["name", "summary", "label", "kind"] {
                if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
                    parts.push(s.to_owned());
                }
            }
        }
    }
}

fn path_class_value(path: &str) -> &'static str {
    if is_noisy_path(path) {
        PATH_CLASS_NOISY
    } else {
        PATH_CLASS_CLEAN
    }
}

fn is_noisy_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    let segments: Vec<&str> = path.split('/').collect();
    if segments.iter().any(|segment| {
        matches!(
            *segment,
            ".git"
                | "node_modules"
                | "vendor"
                | "vendors"
                | "dist"
                | "build"
                | "target"
                | "coverage"
                | "__pycache__"
                | ".venv"
                | "venv"
                | ".next"
                | ".nuxt"
                | ".turbo"
                | "generated"
                | "gen"
                | "third_party"
                | "third-party"
        )
    }) {
        return true;
    }
    path.ends_with(".min.js")
        || path.ends_with(".min.css")
        || path.ends_with(".json")
        || path.ends_with(".jsonl")
        || path.ends_with(".lock")
        || path.ends_with("package-lock.json")
        || path.ends_with("pnpm-lock.yaml")
        || path.ends_with("yarn.lock")
        || path.ends_with("composer.lock")
        || path.ends_with("cargo.lock")
        || path.ends_with("go.sum")
        || path.ends_with("tokenizer.json")
        || path.ends_with("vocab.json")
        || path.ends_with("merges.txt")
}

fn normalize_for_rank(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn str_field(doc: &TantivyDocument, field: Field) -> Option<String> {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

fn u64_field(doc: &TantivyDocument, field: Field) -> Option<u64> {
    doc.get_first(field).and_then(|v| v.as_u64())
}

fn make_snippet(doc: &TantivyDocument, snippets: &SnippetGenerator) -> Option<String> {
    let snippet = snippets.snippet_from_doc(doc);
    if snippet.fragment().is_empty() {
        None
    } else {
        Some(snippet.to_html())
    }
}

/// 在 UTF-8 字符边界处把 `s` 截断到不超过 `limit` 字节。
fn truncate_utf8(s: &str, limit: usize) -> &str {
    if s.len() <= limit {
        return s;
    }
    let mut end = limit;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::{truncate_utf8, SearchIndex, SearchIndexWriter};
    use aka_core::types::NodeRec;
    use serde_json::{Map, Value};

    #[test]
    fn truncate_respects_char_boundary() {
        let s = "a中文测试";
        let t = truncate_utf8(s, 4);
        assert!(t.len() <= 4);
        assert_eq!(t, "a中");
        assert_eq!(truncate_utf8("short", 2048), "short");
    }

    #[test]
    fn indexes_application_semantic_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let mut job_props = Map::new();
        job_props.insert("name".into(), Value::String("orders.cleanup".into()));
        job_props.insert("jobType".into(), Value::String("apscheduler-job".into()));
        job_props.insert(
            "schedule".into(),
            Value::String("trigger=cron,hour=3".into()),
        );
        job_props.insert("handlerName".into(), Value::String("cleanup_orders".into()));
        job_props.insert(
            "strategy".into(),
            Value::String("python-apscheduler-scheduled-job".into()),
        );
        job_props.insert("filePath".into(), Value::String("tasks.py".into()));

        let mut command_props = Map::new();
        command_props.insert("name".into(), Value::String("orders-reindex".into()));
        command_props.insert(
            "commandType".into(),
            Value::String("django-management-command".into()),
        );
        command_props.insert("handlerName".into(), Value::String("handle".into()));
        command_props.insert(
            "strategy".into(),
            Value::String("python-django-management-command".into()),
        );
        command_props.insert(
            "filePath".into(),
            Value::String("orders/management/commands/orders_reindex.py".into()),
        );

        let mut config_props = Map::new();
        config_props.insert(
            "name".into(),
            Value::String("orders.retry.max-attempts".into()),
        );
        config_props.insert(
            "key".into(),
            Value::String("orders.retry.max-attempts".into()),
        );
        config_props.insert(
            "configType".into(),
            Value::String("spring-property".into()),
        );
        config_props.insert("valueHint".into(), Value::String("3".into()));
        config_props.insert(
            "sources".into(),
            Value::Array(vec![Value::String("yaml-file".into())]),
        );
        config_props.insert(
            "filePath".into(),
            Value::String("src/main/resources/application.yml".into()),
        );

        let mut table_props = Map::new();
        table_props.insert("name".into(), Value::String("orders".into()));
        table_props.insert("tableName".into(), Value::String("orders".into()));
        table_props.insert("entityName".into(), Value::String("Order".into()));
        table_props.insert(
            "columns".into(),
            Value::Array(vec![Value::String("status".into())]),
        );

        let mut repo_props = Map::new();
        repo_props.insert("name".into(), Value::String("OrderRepository".into()));
        repo_props.insert("entityName".into(), Value::String("Order".into()));
        repo_props.insert(
            "repositorySource".into(),
            Value::String("java-spring-data-repository".into()),
        );

        let mut migration_props = Map::new();
        migration_props.insert("name".into(), Value::String("V1__create_orders".into()));
        migration_props.insert(
            "migrationType".into(),
            Value::String("sql-migration".into()),
        );
        migration_props.insert("version".into(), Value::String("1".into()));
        migration_props.insert(
            "tables".into(),
            Value::Array(vec![Value::String("orders".into())]),
        );
        migration_props.insert(
            "operations".into(),
            Value::Array(vec![Value::String("create".into())]),
        );
        migration_props.insert(
            "filePath".into(),
            Value::String("src/main/resources/db/migration/V1__create_orders.sql".into()),
        );

        let mut cache_props = Map::new();
        cache_props.insert("name".into(), Value::String("orders:last".into()));
        cache_props.insert("backend".into(), Value::String("redis".into()));
        cache_props.insert("cacheSource".into(), Value::String("source-scan".into()));

        let mut event_props = Map::new();
        event_props.insert("name".into(), Value::String("OrderCreatedEvent".into()));
        event_props.insert(
            "bus".into(),
            Value::String("spring-application-event".into()),
        );
        event_props.insert("eventSource".into(), Value::String("source-scan".into()));

        let mut policy_props = Map::new();
        policy_props.insert("name".into(), Value::String("orders.view_order".into()));
        policy_props.insert("policyType".into(), Value::String("permission".into()));
        policy_props.insert("policySource".into(), Value::String("source-scan".into()));

        let mut resource_props = Map::new();
        resource_props.insert(
            "name".into(),
            Value::String("payments.example.com/v1/orders/{param}/charge".into()),
        );
        resource_props.insert(
            "url".into(),
            Value::String("https://payments.example.com/v1/orders/{param}/charge".into()),
        );
        resource_props.insert("resourceType".into(), Value::String("http".into()));
        resource_props.insert("resourceSource".into(), Value::String("source-scan".into()));

        let mut transaction_props = Map::new();
        transaction_props.insert(
            "name".into(),
            Value::String("submitOrder transaction".into()),
        );
        transaction_props.insert("manager".into(), Value::String("spring-transaction".into()));
        transaction_props.insert("propagation".into(), Value::String("REQUIRES_NEW".into()));
        transaction_props.insert("readOnly".into(), Value::Bool(false));

        let mut writer = SearchIndexWriter::create(dir.path()).unwrap();
        writer
            .add_nodes(
                [
                    NodeRec {
                        id: "job:orders-cleanup".into(),
                        label: "Job".into(),
                        properties: job_props,
                    },
                    NodeRec {
                        id: "command:orders-reindex".into(),
                        label: "Command".into(),
                        properties: command_props,
                    },
                    NodeRec {
                        id: "config:orders-retry".into(),
                        label: "Config".into(),
                        properties: config_props,
                    },
                    NodeRec {
                        id: "table:orders".into(),
                        label: "Table".into(),
                        properties: table_props,
                    },
                    NodeRec {
                        id: "repository:orders".into(),
                        label: "Repository".into(),
                        properties: repo_props,
                    },
                    NodeRec {
                        id: "migration:create-orders".into(),
                        label: "Migration".into(),
                        properties: migration_props,
                    },
                    NodeRec {
                        id: "cache:orders-last".into(),
                        label: "Cache".into(),
                        properties: cache_props,
                    },
                    NodeRec {
                        id: "event:order-created".into(),
                        label: "Event".into(),
                        properties: event_props,
                    },
                    NodeRec {
                        id: "policy:orders-view".into(),
                        label: "Policy".into(),
                        properties: policy_props,
                    },
                    NodeRec {
                        id: "resource:payments-charge".into(),
                        label: "Resource".into(),
                        properties: resource_props,
                    },
                    NodeRec {
                        id: "transaction:submit-order".into(),
                        label: "Transaction".into(),
                        properties: transaction_props,
                    },
                ]
                .into_iter(),
            )
            .unwrap();
        writer.commit().unwrap();
        drop(writer);

        let index = SearchIndex::open(dir.path()).unwrap();
        let hits = index.search("orders cleanup hour 3", 5).unwrap();
        let hit = hits.first().expect("job search hit");
        assert_eq!(hit.node_id, "job:orders-cleanup");
        assert_eq!(hit.label, "Job");
        assert_eq!(hit.name, "orders.cleanup");

        let command_hits = index.search("django command orders reindex", 5).unwrap();
        let command_hit = command_hits.first().expect("command search hit");
        assert_eq!(command_hit.node_id, "command:orders-reindex");
        assert_eq!(command_hit.label, "Command");

        let config_hits = index.search("orders retry max attempts config", 5).unwrap();
        let config_hit = config_hits.first().expect("config search hit");
        assert_eq!(config_hit.node_id, "config:orders-retry");
        assert_eq!(config_hit.label, "Config");

        let table_hits = index.search("orders status table", 5).unwrap();
        let table_hit = table_hits.first().expect("table search hit");
        assert_eq!(table_hit.node_id, "table:orders");
        assert_eq!(table_hit.label, "Table");

        let repo_hits = index.search("order repository spring data", 5).unwrap();
        let repo_hit = repo_hits.first().expect("repository search hit");
        assert_eq!(repo_hit.node_id, "repository:orders");
        assert_eq!(repo_hit.label, "Repository");

        let migration_hits = index.search("flyway migration create orders", 5).unwrap();
        let migration_hit = migration_hits.first().expect("migration search hit");
        assert_eq!(migration_hit.node_id, "migration:create-orders");
        assert_eq!(migration_hit.label, "Migration");

        let cache_hits = index.search("redis orders last cache", 5).unwrap();
        let cache_hit = cache_hits.first().expect("cache search hit");
        assert_eq!(cache_hit.node_id, "cache:orders-last");
        assert_eq!(cache_hit.label, "Cache");

        let event_hits = index.search("order created spring event", 5).unwrap();
        let event_hit = event_hits.first().expect("event search hit");
        assert_eq!(event_hit.node_id, "event:order-created");
        assert_eq!(event_hit.label, "Event");

        let policy_hits = index.search("orders view permission", 5).unwrap();
        let policy_hit = policy_hits.first().expect("policy search hit");
        assert_eq!(policy_hit.node_id, "policy:orders-view");
        assert_eq!(policy_hit.label, "Policy");

        let resource_hits = index.search("payments charge http resource", 5).unwrap();
        let resource_hit = resource_hits.first().expect("resource search hit");
        assert_eq!(resource_hit.node_id, "resource:payments-charge");
        assert_eq!(resource_hit.label, "Resource");

        let transaction_hits = index
            .search("submit order spring transaction requires new", 5)
            .unwrap();
        let transaction_hit = transaction_hits.first().expect("transaction search hit");
        assert_eq!(transaction_hit.node_id, "transaction:submit-order");
        assert_eq!(transaction_hit.label, "Transaction");
    }
}
