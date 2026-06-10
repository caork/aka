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
    /// 切块类型（ast-function / char …）；命中来自 chunk 文档时携带（合同只增不改）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
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
    /// 运行时状态：`ready` / `indexing` / `failed`。
    pub status: String,
    /// 来源：`local` / `git` / `zip`。
    pub source_kind: String,
    /// git 来源的 clone URL。
    pub source_url: Option<String>,
    /// 失败原因等补充信息（status = failed 时携带）。
    pub detail: Option<String>,
    /// per-repo 预览渲染节点预算；None = 默认 50_000。
    pub render_max_nodes: Option<u32>,
}

/// per-repo 设置更新（settings 端点与 Backend 接缝共用一个形状）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSettingsUpdate {
    pub embeddings_enabled: bool,
    /// None = 恢复默认渲染预算（50_000）；Some(v) 写入前必须 clamp 到
    /// `aka_core::MIN_RENDER_NODES..=aka_core::MAX_RENDER_NODES`。
    #[serde(default)]
    pub render_max_nodes: Option<u32>,
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

    /// 图 LOD 快照（aka-graph `LodGraph` 的 JSON 形状），给可视化用。
    /// `max_nodes = None` 时由实现解析 per-repo 的 render_max_nodes 设置
    /// （没有则默认 50_000）；无论来源，最终预算必须 clamp 到硬上限。
    /// 默认不支持——只有接了图存储的 Backend 才覆写。
    fn graph_lod(&self, repo: &str, max_nodes: Option<usize>) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, max_nodes);
        anyhow::bail!("graph_lod not supported by this backend")
    }

    // ── 仓库管理（导入 / 更新 / 删除 / 设置）──────────────────────
    // 默认全部 bail "not supported"——只有真实 Backend（aka-cli）覆写。
    // 导入 / 更新都是 202 语义：调用立即返回仓库名，分析任务在后台执行，
    // 进度经 `repo_runtime_status` / `list_repos` 的 status 字段暴露。

    /// 导入新仓库。`kind` = `git`（src 为 clone URL）或 `local`（src 为本地路径）。
    /// 返回最终仓库名；分析在后台执行。
    fn import_repo(&self, kind: &str, src: &str, name: Option<&str>) -> anyhow::Result<String> {
        let _ = (kind, src, name);
        anyhow::bail!("import_repo not supported by this backend")
    }

    /// 重新拉取并分析（git: pull + analyze；local: 直接 analyze；zip: 报错提示走 update-zip）。
    fn update_repo(&self, name: &str) -> anyhow::Result<String> {
        let _ = name;
        anyhow::bail!("update_repo not supported by this backend")
    }

    /// 用新 zip 覆盖更新 zip 来源仓库（清空 checkout 后重新解压 + analyze）。
    fn update_repo_zip(&self, name: &str, zip_path: &std::path::Path) -> anyhow::Result<String> {
        let _ = (name, zip_path);
        anyhow::bail!("update_repo_zip not supported by this backend")
    }

    /// 从 zip 包导入新仓库（解压到受管 checkout 目录 + 后台 analyze）。
    fn import_repo_zip(&self, name: &str, zip_path: &std::path::Path) -> anyhow::Result<String> {
        let _ = (name, zip_path);
        anyhow::bail!("import_repo_zip not supported by this backend")
    }

    /// 移除仓库：注册表 + 数据目录；受管 checkout 一并删除（用户本地路径不动）。
    fn remove_repo(&self, name: &str) -> anyhow::Result<()> {
        let _ = name;
        anyhow::bail!("remove_repo not supported by this backend")
    }

    /// 每仓库设置（embedding 开关 + 渲染节点预算；向量回填是后续版本）。
    fn set_repo_settings(&self, name: &str, settings: RepoSettingsUpdate) -> anyhow::Result<()> {
        let _ = (name, settings);
        anyhow::bail!("set_repo_settings not supported by this backend")
    }

    /// 节点详情（完整 properties + 度数概要），给前端弹窗用。
    fn node_detail(&self, repo: &str, id: &str) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, id);
        anyhow::bail!("node_detail not supported by this backend")
    }

    /// 以某节点为中心的 ego 子图（与 LodGraph 同形 JSON），给中心化重渲用。
    fn ego_graph(
        &self,
        repo: &str,
        id: &str,
        depth: u32,
        max_nodes: usize,
    ) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, id, depth, max_nodes);
        anyhow::bail!("ego_graph not supported by this backend")
    }

    /// 读取仓库内某文件的源码切片（详情面板用）。`start`/`end` 为 1-based 含端
    /// 行号，缺省 = 整个文件（单次最多 2000 行）。返回 JSON 合同：
    /// `{path, abs_path, total_lines, start, end, lines: [..], truncated}`。
    /// 实现必须防路径穿越（canonicalize 后仍须位于 repo 根目录内）。
    /// 默认不支持——只有拿得到 repo checkout 的 Backend 才覆写。
    fn read_source(
        &self,
        repo: &str,
        path: &str,
        start: Option<u32>,
        end: Option<u32>,
    ) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, path, start, end);
        anyhow::bail!("read_source not supported by this backend")
    }

    /// 文件内符号列表（源码预览高亮用）：`path` 与 nodes 表 file_path 精确匹配，
    /// 返回 JSON 合同 `{path, symbols: [{id, name, label, file, line, end_line}]}`，
    /// symbols 按 line 升序；无行号的节点（File / Folder / Community 等）滤掉。
    /// 文件存在但没有符号 → 空数组；repo 未注册 → Err（HTTP 面 404）。
    /// 默认不支持——只有接了图存储的 Backend 才覆写。
    fn file_symbols(&self, repo: &str, path: &str) -> anyhow::Result<serde_json::Value> {
        let _ = (repo, path);
        anyhow::bail!("file_symbols not supported by this backend")
    }

    /// 后台任务运行时状态：仓库名 → (status, detail)。
    /// status ∈ {indexing, failed}；不在 map 里 = ready。
    fn repo_runtime_status(&self) -> std::collections::HashMap<String, (String, Option<String>)> {
        Default::default()
    }
}
