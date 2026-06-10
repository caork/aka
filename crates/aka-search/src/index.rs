//! BM25 全文索引 — tantivy 实现，schema 与查询策略见 [`SearchIndex`]。

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
    /// 节点 label（nodes 文档）或 chunk kind（chunks 文档）。
    pub label: String,
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
    label: Field,
    start_line: Field,
    end_line: Field,
}

/// BM25 全文索引。
///
/// schema：`node_id`(STRING stored) / `name`(TEXT code-tokenizer，查询权重 x3) /
/// `text`(TEXT code-tokenizer，存储截断 2KB 供 snippet) / `file_path`(TEXT 按
/// `/` `.` 等拆分) / `label`(STRING stored) / `start_line` `end_line`(u64 stored)。
///
/// 写入端基于 tantivy 原生段合并，增量友好：多次 `add_*` + [`commit`](Self::commit)
/// 即可追加；重新 [`open`](Self::open) 后继续写入。
pub struct SearchIndex {
    index: Index,
    writer: IndexWriter<TantivyDocument>,
    reader: IndexReader,
    fields: Fields,
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
    builder.add_text_field("label", STRING | STORED);
    builder.add_u64_field("start_line", STORED);
    builder.add_u64_field("end_line", STORED);
    builder.build()
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

impl SearchIndex {
    /// 在 `dir` 下新建索引（目录不存在会创建；已有索引则报错）。
    pub fn create(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let index = Index::create_in_dir(dir, build_schema())?;
        Self::finish_open(index)
    }

    /// 打开 `dir` 下既有索引。
    pub fn open(dir: &Path) -> Result<Self> {
        let index = Index::open_in_dir(dir)?;
        Self::finish_open(index)
    }

    fn finish_open(index: Index) -> Result<Self> {
        register_tokenizers(&index);
        let schema = index.schema();
        let field = |name: &str| schema.get_field(name).expect("schema field exists");
        let fields = Fields {
            node_id: field("node_id"),
            name: field("name"),
            text: field("text"),
            file_path: field("file_path"),
            label: field("label"),
            start_line: field("start_line"),
            end_line: field("end_line"),
        };
        let writer = index.writer_with_num_threads(WRITER_THREADS, WRITER_MEM_BUDGET)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self {
            index,
            writer,
            reader,
            fields,
        })
    }

    /// 索引图谱节点：用 `name` / `filePath` / `label` 建文档（`text` 留空）。
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
            }
            doc.add_text(self.fields.label, &node.label);
            /* 行号统一存 1-based（工件是 tree-sitter 0-based row） */
            if let Some(line) = node.start_line_1based() {
                doc.add_u64(self.fields.start_line, u64::from(line));
            }
            if let Some(line) = node.end_line_1based() {
                doc.add_u64(self.fields.end_line, u64::from(line));
            }
            self.writer.add_document(doc)?;
        }
        Ok(())
    }

    /// 索引代码切块：`text` = chunk 正文（截断到 2KB），`label` = chunk kind。
    ///
    /// 写入后需调用 [`commit`](Self::commit) 才对检索可见。
    pub fn add_chunks(&mut self, chunks: impl Iterator<Item = ChunkRec>) -> Result<()> {
        for chunk in chunks {
            let mut doc = TantivyDocument::new();
            doc.add_text(self.fields.node_id, &chunk.node_id);
            doc.add_text(self.fields.text, truncate_utf8(&chunk.text, TEXT_STORE_LIMIT));
            doc.add_text(self.fields.file_path, &chunk.file_path);
            doc.add_text(self.fields.label, &chunk.kind);
            doc.add_u64(self.fields.start_line, u64::from(chunk.start_line_1based()));
            doc.add_u64(self.fields.end_line, u64::from(chunk.end_line_1based()));
            self.writer.add_document(doc)?;
        }
        Ok(())
    }

    /// 提交写入并刷新 reader（tantivy 后台自动做段合并）。
    pub fn commit(&mut self) -> Result<()> {
        self.writer.commit()?;
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

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for term in &terms {
            let name_term = TermQuery::new(
                Term::from_field_text(self.fields.name, term),
                IndexRecordOption::WithFreqs,
            );
            clauses.push((
                Occur::Should,
                Box::new(BoostQuery::new(Box::new(name_term), NAME_BOOST)),
            ));
            clauses.push((
                Occur::Should,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.text, term),
                    IndexRecordOption::WithFreqs,
                )),
            ));
            clauses.push((
                Occur::Should,
                Box::new(TermQuery::new(
                    Term::from_field_text(self.fields.file_path, term),
                    IndexRecordOption::WithFreqs,
                )),
            ));
        }
        if query.split_whitespace().count() < FUZZY_WORD_LIMIT {
            for term in &terms {
                if term.chars().count() < FUZZY_MIN_TERM_CHARS {
                    continue;
                }
                clauses.push((
                    Occur::Should,
                    Box::new(FuzzyTermQuery::new(
                        Term::from_field_text(self.fields.name, term),
                        1,
                        true,
                    )),
                ));
                clauses.push((
                    Occur::Should,
                    Box::new(FuzzyTermQuery::new(
                        Term::from_field_text(self.fields.text, term),
                        1,
                        true,
                    )),
                ));
            }
        }
        let bool_query = BooleanQuery::new(clauses);

        let searcher = self.reader.searcher();
        // 多抓一些以便同 node_id 去重后仍能凑满 limit。
        let fetch = limit.saturating_mul(4).max(limit.saturating_add(16));
        let top_docs = searcher.search(&bool_query, &TopDocs::with_limit(fetch).order_by_score())?;

        let mut snippet_gen = SnippetGenerator::create(&searcher, &bool_query, self.fields.text)?;
        snippet_gen.set_max_num_chars(SNIPPET_MAX_CHARS);

        let mut order: Vec<String> = Vec::new();
        let mut best: HashMap<String, Hit> = HashMap::new();
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
                // top_docs 按分数降序，首次出现即该 node_id 的最高分；
                // 后续重复文档只用来补全缺失字段。
                Entry::Occupied(mut slot) => {
                    self.fill_missing(slot.get_mut(), &doc, &snippet_gen);
                }
            }
        }

        Ok(order
            .into_iter()
            .take(limit)
            .filter_map(|id| best.remove(&id))
            .collect())
    }

    fn make_hit(&self, score: f32, doc: &TantivyDocument, snippets: &SnippetGenerator) -> Hit {
        Hit {
            node_id: str_field(doc, self.fields.node_id).unwrap_or_default(),
            score,
            name: str_field(doc, self.fields.name).unwrap_or_default(),
            file_path: str_field(doc, self.fields.file_path).unwrap_or_default(),
            label: str_field(doc, self.fields.label).unwrap_or_default(),
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
    seen
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
    use super::truncate_utf8;

    #[test]
    fn truncate_respects_char_boundary() {
        let s = "a中文测试";
        let t = truncate_utf8(s, 4);
        assert!(t.len() <= 4);
        assert_eq!(t, "a中");
        assert_eq!(truncate_utf8("short", 2048), "short");
    }
}
