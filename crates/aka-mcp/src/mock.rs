//! 内存 MockBackend — 测试与手测用的假数据实现。
//!
//! 数据集（repo `demo`，5 个函数 + 调用关系）：
//!
//! ```text
//! main ──CALLS──▶ handle_request ──CALLS──▶ parse_config ──CALLS──▶ read_file
//!                       │
//!                       └─CALLS──▶ write_output
//! ```
//!
//! 两条执行流程（Process 合成节点的归属数据，验证流程相关字段）：
//! `main → read_file`（main/handle_request/parse_config/read_file，步号 1–4）、
//! `main → write_output`（main/handle_request/write_output，步号 1–3）。
//!
//! 另有 repo `beta`（1 个孤立函数，无流程归属），用于验证 repo 过滤与字段省略。

use std::collections::{HashSet, VecDeque};

use crate::backend::{Backend, ProcessHit, RepoInfo, SearchHit, SymbolRef};

#[derive(Debug, Clone)]
struct MockNode {
    id: &'static str,
    name: &'static str,
    label: &'static str,
    repo: &'static str,
    file: &'static str,
    line: u32,
}

/// (source idx, target idx, edge type)
type MockEdge = (usize, usize, &'static str);

const NODES: &[MockNode] = &[
    MockNode { id: "demo:fn:main", name: "main", label: "Function", repo: "demo", file: "src/main.rs", line: 3 },
    MockNode { id: "demo:fn:handle_request", name: "handle_request", label: "Function", repo: "demo", file: "src/handler.rs", line: 12 },
    MockNode { id: "demo:fn:parse_config", name: "parse_config", label: "Function", repo: "demo", file: "src/config.rs", line: 8 },
    MockNode { id: "demo:fn:read_file", name: "read_file", label: "Function", repo: "demo", file: "src/io.rs", line: 5 },
    MockNode { id: "demo:fn:write_output", name: "write_output", label: "Function", repo: "demo", file: "src/io.rs", line: 21 },
    MockNode { id: "beta:fn:beta_main", name: "beta_main", label: "Function", repo: "beta", file: "src/main.rs", line: 1 },
];

const EDGES: &[MockEdge] = &[
    (0, 1, "CALLS"), // main -> handle_request
    (1, 2, "CALLS"), // handle_request -> parse_config
    (1, 4, "CALLS"), // handle_request -> write_output
    (2, 3, "CALLS"), // parse_config -> read_file
];

/// Process 合成节点的归属数据（`符号-[STEP_IN_PROCESS]->Process` 的 mock 版）。
struct MockProcess {
    id: &'static str,
    name: &'static str,
    process_type: &'static str,
    /// (NODES 下标, 步号)，按步号升序。
    steps: &'static [(usize, u32)],
}

const PROCESSES: &[MockProcess] = &[
    MockProcess {
        id: "demo:proc:request-flow",
        name: "main → read_file",
        process_type: "call_chain",
        steps: &[(0, 1), (1, 2), (2, 3), (3, 4)],
    },
    MockProcess {
        id: "demo:proc:output-flow",
        name: "main → write_output",
        process_type: "call_chain",
        steps: &[(0, 1), (1, 2), (4, 3)],
    },
];

/// 内存假数据 Backend。`MockBackend::demo()` 即可用。
#[derive(Debug, Default, Clone)]
pub struct MockBackend;

impl MockBackend {
    pub fn demo() -> Self {
        Self
    }

    fn node_in_repo(node: &MockNode, repo: Option<&str>) -> bool {
        repo.is_none_or(|r| r == node.repo)
    }

    fn hit(node: &MockNode, score: f32, snippet: bool) -> SearchHit {
        SearchHit {
            node_id: node.id.to_string(),
            name: node.name.to_string(),
            label: node.label.to_string(),
            kind: None,
            file_path: node.file.to_string(),
            start_line: node.line,
            score,
            snippet: snippet.then(|| format!("fn {}(…) {{ … }}", node.name)),
        }
    }

    fn symbol_ref(node: &MockNode, edge_type: &str, depth: u32) -> SymbolRef {
        SymbolRef {
            node_id: node.id.to_string(),
            name: node.name.to_string(),
            label: node.label.to_string(),
            file_path: node.file.to_string(),
            start_line: node.line,
            edge_type: edge_type.to_string(),
            depth,
        }
    }

    fn find_indices(repo: Option<&str>, symbol: &str) -> Vec<usize> {
        NODES
            .iter()
            .enumerate()
            .filter(|(_, n)| Self::node_in_repo(n, repo) && n.name == symbol)
            .map(|(i, _)| i)
            .collect()
    }

    /// 从 `symbol` 出发沿 CALLS 边 BFS；`reverse = true` 走反向边（callers）。
    fn walk(repo: Option<&str>, symbol: &str, depth: u32, reverse: bool) -> Vec<SymbolRef> {
        let mut out = Vec::new();
        let mut seen: HashSet<usize> = HashSet::new();
        let mut queue: VecDeque<(usize, u32)> = VecDeque::new();
        for i in Self::find_indices(repo, symbol) {
            seen.insert(i);
            queue.push_back((i, 0));
        }
        while let Some((cur, d)) = queue.pop_front() {
            if d >= depth {
                continue;
            }
            for &(src, dst, ty) in EDGES {
                let (from, to) = if reverse { (dst, src) } else { (src, dst) };
                if from == cur && seen.insert(to) {
                    out.push(Self::symbol_ref(&NODES[to], ty, d + 1));
                    queue.push_back((to, d + 1));
                }
            }
        }
        out
    }
}

impl Backend for MockBackend {
    fn list_repos(&self) -> anyhow::Result<Vec<RepoInfo>> {
        Ok(vec![
            RepoInfo {
                name: "demo".into(),
                path: "/tmp/demo".into(),
                nodes: 5,
                edges: 4,
                indexed_at: Some(1_750_000_000),
                embeddings_enabled: false,
                status: "ready".into(),
                source_kind: "local".into(),
                source_url: None,
                detail: None,
                render_max_nodes: None,
            },
            RepoInfo {
                name: "beta".into(),
                path: "/tmp/beta".into(),
                nodes: 1,
                edges: 0,
                indexed_at: None,
                embeddings_enabled: false,
                status: "ready".into(),
                source_kind: "git".into(),
                source_url: Some("https://example.com/beta.git".into()),
                detail: None,
                render_max_nodes: None,
            },
        ])
    }

    fn search(
        &self,
        repo: Option<&str>,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let q = query.to_lowercase();
        Ok(NODES
            .iter()
            .filter(|n| Self::node_in_repo(n, repo))
            .filter(|n| n.name.to_lowercase().contains(&q))
            .enumerate()
            .map(|(i, n)| Self::hit(n, 1.0 - 0.1 * i as f32, true))
            .take(limit)
            .collect())
    }

    fn find_definition(
        &self,
        repo: Option<&str>,
        symbol: &str,
    ) -> anyhow::Result<Vec<SearchHit>> {
        Ok(Self::find_indices(repo, symbol)
            .into_iter()
            .map(|i| Self::hit(&NODES[i], 1.0, true))
            .collect())
    }

    fn references(
        &self,
        repo: Option<&str>,
        symbol: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<SymbolRef>> {
        let targets = Self::find_indices(repo, symbol);
        Ok(EDGES
            .iter()
            .filter(|(_, dst, _)| targets.contains(dst))
            .map(|&(src, _, ty)| Self::symbol_ref(&NODES[src], ty, 1))
            .take(limit)
            .collect())
    }

    fn callers(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
    ) -> anyhow::Result<Vec<SymbolRef>> {
        Ok(Self::walk(repo, symbol, depth, true))
    }

    fn callees(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
    ) -> anyhow::Result<Vec<SymbolRef>> {
        Ok(Self::walk(repo, symbol, depth, false))
    }

    fn impact(
        &self,
        repo: Option<&str>,
        symbol: &str,
        depth: u32,
        limit: usize,
    ) -> anyhow::Result<Vec<SymbolRef>> {
        let mut refs = Self::walk(repo, symbol, depth, true);
        refs.truncate(limit);
        Ok(refs)
    }

    fn analyze(&self, repo_path: &str) -> anyhow::Result<String> {
        Ok(format!(
            "mock analyze: queued indexing for {repo_path} (nodes=5 edges=4, no-op)"
        ))
    }

    fn processes_of(&self, repo: Option<&str>, node_id: &str) -> anyhow::Result<Vec<ProcessHit>> {
        // 按节点 id（非符号名）查归属；未知节点 / repo 不匹配 → 空 Vec 而非错误。
        let Some(idx) = NODES
            .iter()
            .position(|n| n.id == node_id && Self::node_in_repo(n, repo))
        else {
            return Ok(Vec::new());
        };
        Ok(PROCESSES
            .iter()
            .filter_map(|p| {
                p.steps.iter().find(|(i, _)| *i == idx).map(|&(_, step)| ProcessHit {
                    process_id: p.id.to_string(),
                    name: p.name.to_string(),
                    process_type: p.process_type.to_string(),
                    step: Some(step),
                    step_count: Some(p.steps.len() as u32),
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callers_bfs_depth() {
        let b = MockBackend::demo();
        let one = b.callers(None, "read_file", 1).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].name, "parse_config");
        assert_eq!(one[0].depth, 1);

        let deep = b.callers(None, "read_file", 5).unwrap();
        let names: Vec<_> = deep.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["parse_config", "handle_request", "main"]);
        assert_eq!(deep.last().unwrap().depth, 3);
    }

    #[test]
    fn repo_filter() {
        let b = MockBackend::demo();
        assert!(b.search(Some("beta"), "main", 10).unwrap().len() == 1);
        assert!(b.find_definition(Some("beta"), "main").unwrap().is_empty());
    }

    #[test]
    fn processes_of_membership() {
        let b = MockBackend::demo();
        let hits = b.processes_of(None, "demo:fn:handle_request").unwrap();
        assert_eq!(hits.len(), 2, "handle_request 在两条执行流里");
        assert_eq!(hits[0].process_id, "demo:proc:request-flow");
        assert_eq!(hits[0].process_type, "call_chain");
        assert_eq!(hits[0].step, Some(2));
        assert_eq!(hits[0].step_count, Some(4));
        assert_eq!(hits[1].process_id, "demo:proc:output-flow");
        assert_eq!(hits[1].step_count, Some(3));

        // repo 过滤：beta 仓库里没有 demo 的节点。
        assert!(b.processes_of(Some("beta"), "demo:fn:handle_request").unwrap().is_empty());
        // 未知节点 → 空 Vec 而非错误（合同：查不到不是错误）。
        assert!(b.processes_of(None, "nope").unwrap().is_empty());
        // 不在任何流程里的节点 → 空 Vec。
        assert!(b.processes_of(None, "beta:fn:beta_main").unwrap().is_empty());
    }
}
