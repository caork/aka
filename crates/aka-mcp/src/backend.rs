//! 数据层接缝 — MCP / HTTP 工具逻辑全部面向这个 trait 编写。
//!
//! 真实实现（tantivy 检索 + SQLite/CSR 图查询）由集成批次接入；
//! 本 crate 自带 [`crate::mock::MockBackend`] 供测试与手测。
//!
//! 约定：
//! - `repo = None` 表示「所有已注册仓库」；`Some(name)` 按 registry 里的仓库名过滤。
//! - 查不到不是错误：返回空 Vec。`Err` 只用于真正的故障（索引损坏、IO 等）。
//! - 方法是同步签名；调用方（async 上下文）负责用 `spawn_blocking` 包装。

use serde::{Deserialize, Serialize};

/// 全文 / 符号检索命中。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub node_id: String,
    pub name: String,
    /// 节点类型（Function / Class / Method / File …），对应图谱 label。
    pub label: String,
    pub file_path: String,
    pub start_line: u32,
    pub score: f32,
    pub snippet: Option<String>,
}

/// 图遍历得到的符号引用（callers / callees / references / impact）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolRef {
    pub node_id: String,
    pub name: String,
    pub label: String,
    pub file_path: String,
    pub start_line: u32,
    /// 到达该节点经过的边类型（CALLS / IMPORTS / …）。
    pub edge_type: String,
    /// 距离起点的跳数，从 1 开始。
    pub depth: u32,
}

/// 已注册仓库的概要信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoInfo {
    pub name: String,
    pub path: String,
    pub nodes: u64,
    pub edges: u64,
    /// 索引完成时间（unix 秒）；None = 注册过但尚未成功索引。
    pub indexed_at: Option<u64>,
    pub embeddings_enabled: bool,
}

/// 数据层抽象。所有工具（MCP 八件套 + HTTP API）只依赖这个 trait。
pub trait Backend: Send + Sync + 'static {
    fn list_repos(&self) -> anyhow::Result<Vec<RepoInfo>>;

    /// 关键词 / 语义混合检索。
    fn search(&self, repo: Option<&str>, query: &str, limit: usize)
        -> anyhow::Result<Vec<SearchHit>>;

    /// 按符号名精确定位定义（可能重名，返回多条）。
    fn find_definition(&self, repo: Option<&str>, symbol: &str)
        -> anyhow::Result<Vec<SearchHit>>;

    /// 指向该符号的所有引用边（任意边类型，一跳）。
    fn references(&self, repo: Option<&str>, symbol: &str, limit: usize)
        -> anyhow::Result<Vec<SymbolRef>>;

    /// 反向调用链（谁调用了它），BFS 到 `depth` 跳。
    fn callers(&self, repo: Option<&str>, symbol: &str, depth: u32)
        -> anyhow::Result<Vec<SymbolRef>>;

    /// 正向调用链（它调用了谁），BFS 到 `depth` 跳。
    fn callees(&self, repo: Option<&str>, symbol: &str, depth: u32)
        -> anyhow::Result<Vec<SymbolRef>>;

    /// 改动影响面：可达的反向依赖集合，截断到 `limit`。
    fn impact(&self, repo: Option<&str>, symbol: &str, depth: u32, limit: usize)
        -> anyhow::Result<Vec<SymbolRef>>;

    /// 触发（重新）分析一个仓库，返回任务描述 / 结果摘要。
    fn analyze(&self, repo_path: &str) -> anyhow::Result<String>;
}
