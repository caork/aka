//! aka-search — 代码知识图谱的混合检索引擎。
//!
//! 替换旧版 SQLite-FTS 方案，由三部分组成：
//!
//! 1. **BM25 全文检索**（[`SearchIndex`]，基于 tantivy）— 用代码感知 tokenizer
//!    （[`CodeTokenizer`]，camelCase / snake_case / kebab-case / 数字边界拆分子词）
//!    索引 `FactSource` 产出的节点与切块。
//! 2. **向量检索**（[`VectorStore`]，基于 usearch HNSW）— cosine 距离的 f32
//!    近邻索引，node_id ↔ u64 key 双向映射持久化到同目录 sidecar 文件。
//! 3. **RRF 混排**（[`rrf_merge`]）— Reciprocal Rank Fusion（K=60）融合两路结果。
//!
//! 典型用法（写入与查询分离：[`SearchIndexWriter`] 持写锁仅限 ingest 期间，
//! [`SearchIndex`] 只读打开，不取写锁，多进程可并发）：
//!
//! ```no_run
//! use aka_search::{rrf_merge, SearchIndex, SearchIndexWriter, VectorStore};
//! # fn main() -> aka_search::Result<()> {
//! let mut writer = SearchIndexWriter::create(std::path::Path::new("/tmp/idx"))?;
//! // writer.add_nodes(...); writer.add_chunks(...);
//! writer.commit()?;
//! drop(writer); // 释放写锁
//! let index = SearchIndex::open(std::path::Path::new("/tmp/idx"))?;
//! let bm25 = index.search("pipeline repo", 20)?;
//! let store = VectorStore::open(std::path::Path::new("/tmp/vec"))?;
//! let semantic = store.search(&vec![0.0; 384], 20)?;
//! let fused = rrf_merge(&bm25, &semantic, 10, |_| None);
//! # let _ = fused; Ok(()) }
//! ```

mod index;
mod rrf;
mod tokenizer;
mod vector;

pub use index::{Hit, SearchIndex, SearchIndexWriter};
pub use rrf::{rrf_merge, RRF_K};
pub use tokenizer::{CodeTokenStream, CodeTokenizer, CODE_TOKENIZER_NAME};
pub use vector::VectorStore;

use std::path::PathBuf;

/// aka-search 统一错误类型。
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// tantivy 内部错误（索引损坏、IO、schema 不匹配等）。
    #[error("tantivy error: {0}")]
    Tantivy(#[from] tantivy::TantivyError),
    /// 文件系统错误（建目录、sidecar 读写等）。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// sidecar JSON 序列化/反序列化错误。
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// usearch 向量索引错误（FFI 异常透传）。
    #[error("vector index error: {0}")]
    Vector(String),
    /// 向量维度与索引建立时不一致。
    #[error("vector dimension mismatch: index expects {expected}, got {got}")]
    DimensionMismatch {
        /// 索引建立时的维度。
        expected: usize,
        /// 调用方传入的维度。
        got: usize,
    },
    /// 路径包含非 UTF-8 字符（usearch FFI 只接受 &str 路径）。
    #[error("non-utf8 path: {0}")]
    InvalidPath(PathBuf),
    /// Long-running write operation was cancelled by the runtime deadline.
    #[error("operation cancelled: {0}")]
    Cancelled(String),
}

/// aka-search 统一 Result 别名。
pub type Result<T> = std::result::Result<T, SearchError>;
