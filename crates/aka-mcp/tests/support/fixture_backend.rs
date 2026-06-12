//! 内存 FixtureBackend — 测试与手测用的假数据实现。
//!
//! 数据集（repo `fixture`，5 个函数 + 调用关系）：
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

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use aka_mcp::{
    Backend, ChangeDetection, ChangedRange, ChangedSymbol, CodeLineMatch, CodeSearchHit,
    CodeSearchResult, DirectoryCount, ProcessHit, QueryEnrichment, RepoInfo, RouteConsumer,
    RouteMapEntry, SearchHit, SymbolRef, ToolMapEntry,
};

#[derive(Debug, Clone)]
struct FixtureNode {
    id: &'static str,
    name: &'static str,
    label: &'static str,
    repo: &'static str,
    file: &'static str,
    line: u32,
}

/// (source idx, target idx, edge type)
type FixtureEdge = (usize, usize, &'static str);

struct FixtureSource {
    repo: &'static str,
    file: &'static str,
    lines: &'static [(u32, &'static str)],
}

const NODES: &[FixtureNode] = &[
    FixtureNode {
        id: "fixture:fn:main",
        name: "main",
        label: "Function",
        repo: "fixture",
        file: "src/main.rs",
        line: 3,
    },
    FixtureNode {
        id: "fixture:fn:handle_request",
        name: "handle_request",
        label: "Function",
        repo: "fixture",
        file: "src/handler.rs",
        line: 12,
    },
    FixtureNode {
        id: "fixture:fn:parse_config",
        name: "parse_config",
        label: "Function",
        repo: "fixture",
        file: "src/config.rs",
        line: 8,
    },
    FixtureNode {
        id: "fixture:fn:read_file",
        name: "read_file",
        label: "Function",
        repo: "fixture",
        file: "src/io.rs",
        line: 5,
    },
    FixtureNode {
        id: "fixture:fn:write_output",
        name: "write_output",
        label: "Function",
        repo: "fixture",
        file: "src/io.rs",
        line: 21,
    },
    FixtureNode {
        id: "beta:fn:beta_main",
        name: "beta_main",
        label: "Function",
        repo: "beta",
        file: "src/main.rs",
        line: 1,
    },
    FixtureNode {
        id: "fixture:fn:duplicate_lib",
        name: "duplicate",
        label: "Function",
        repo: "fixture",
        file: "src/a.rs",
        line: 2,
    },
    FixtureNode {
        id: "fixture:fn:duplicate_ui",
        name: "duplicate",
        label: "Function",
        repo: "fixture",
        file: "src/b.rs",
        line: 9,
    },
];

const EDGES: &[FixtureEdge] = &[
    (0, 1, "CALLS"), // main -> handle_request
    (1, 2, "CALLS"), // handle_request -> parse_config
    (1, 4, "CALLS"), // handle_request -> write_output
    (2, 3, "CALLS"), // parse_config -> read_file
];

const SOURCES: &[FixtureSource] = &[
    FixtureSource {
        repo: "fixture",
        file: "src/handler.rs",
        lines: &[
            (11, "pub fn handle_request(req: Request) -> Response {"),
            (12, "    let config = parse_config(req.path());"),
            (13, "    write_output(config)"),
            (14, "}"),
        ],
    },
    FixtureSource {
        repo: "fixture",
        file: "src/config.rs",
        lines: &[
            (7, "pub fn parse_config(path: &str) -> Config {"),
            (8, "    read_file(path).parse_config()"),
            (9, "}"),
        ],
    },
    FixtureSource {
        repo: "beta",
        file: "src/main.rs",
        lines: &[(1, "pub fn beta_main() { println!(\"beta\"); }")],
    },
];

/// Process 合成节点的归属数据（`符号-[STEP_IN_PROCESS]->Process` 的 fixture 版）。
struct FixtureProcess {
    id: &'static str,
    name: &'static str,
    process_type: &'static str,
    /// (NODES 下标, 步号)，按步号升序。
    steps: &'static [(usize, u32)],
}

const PROCESSES: &[FixtureProcess] = &[
    FixtureProcess {
        id: "fixture:proc:request-flow",
        name: "main → read_file",
        process_type: "call_chain",
        steps: &[(0, 1), (1, 2), (2, 3), (3, 4)],
    },
    FixtureProcess {
        id: "fixture:proc:output-flow",
        name: "main → write_output",
        process_type: "call_chain",
        steps: &[(0, 1), (1, 2), (4, 3)],
    },
];

fn fixture_routes(repo: Option<&str>, route: Option<&str>) -> Vec<RouteMapEntry> {
    if repo.is_some_and(|r| r != "fixture") {
        return Vec::new();
    }
    let entry = RouteMapEntry {
        id: "fixture:route:/api/config".into(),
        route: "/api/config".into(),
        handler: "src/routes/config.rs".into(),
        middleware: vec!["withAuth".into()],
        response_keys: vec!["data".into(), "pagination".into()],
        error_keys: vec!["error".into()],
        consumers: vec![
            RouteConsumer {
                name: "ConfigPanel".into(),
                file_path: "src/ui/config_panel.tsx".into(),
                accessed_keys: vec!["data".into(), "missing".into()],
                fetch_count: Some(1),
            },
            RouteConsumer {
                name: "ConfigList".into(),
                file_path: "src/ui/config_list.tsx".into(),
                accessed_keys: vec!["pagination".into()],
                fetch_count: Some(2),
            },
        ],
        flows: vec!["main → read_file".into()],
        properties: None,
    };
    [entry]
        .into_iter()
        .filter(|r| route.is_none_or(|needle| r.route.contains(needle)))
        .collect()
}

fn fixture_tools(repo: Option<&str>, tool: Option<&str>) -> Vec<ToolMapEntry> {
    if repo.is_some_and(|r| r != "fixture") {
        return Vec::new();
    }
    let handler = NODES
        .iter()
        .find(|n| n.name == "handle_request")
        .map(|n| FixtureBackend::hit(n, 1.0, false))
        .into_iter()
        .collect();
    let entry = ToolMapEntry {
        id: "fixture:tool:index_repo".into(),
        name: "index_repo".into(),
        file_path: "src/tools/index_repo.rs".into(),
        description: "Index a repository and build the code graph.".into(),
        handlers: handler,
        flows: vec!["main → write_output".into()],
        properties: None,
    };
    [entry]
        .into_iter()
        .filter(|t| tool.is_none_or(|needle| t.name.contains(needle)))
        .collect()
}

/// 内存假数据 Backend。`FixtureBackend::fixture()` 即可用。
#[derive(Debug, Default, Clone)]
pub struct FixtureBackend;

impl FixtureBackend {
    pub fn fixture() -> Self {
        Self
    }

    fn node_in_repo(node: &FixtureNode, repo: Option<&str>) -> bool {
        repo.is_none_or(|r| r == node.repo)
    }

    fn hit(node: &FixtureNode, score: f32, snippet: bool) -> SearchHit {
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

    fn symbol_ref(node: &FixtureNode, edge_type: &str, depth: u32) -> SymbolRef {
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

impl Backend for FixtureBackend {
    fn list_repos(&self) -> anyhow::Result<Vec<RepoInfo>> {
        Ok(vec![
            RepoInfo {
                name: "fixture".into(),
                path: "/tmp/fixture".into(),
                nodes: 5,
                edges: 4,
                indexed_at: Some(1_750_000_000),
                embeddings_enabled: false,
                status: "ready".into(),
                source_kind: "local".into(),
                source_url: None,
                detail: None,
                render_max_nodes: None,
                progress: None,
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
                progress: None,
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

    fn search_code(
        &self,
        repo: Option<&str>,
        query: &str,
        limit: usize,
        context: usize,
        regex: bool,
        path_filter: Option<&str>,
    ) -> anyhow::Result<CodeSearchResult> {
        let needle = query.to_ascii_lowercase();
        let re = if regex {
            Some(regex::Regex::new(query)?)
        } else {
            None
        };
        let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
        let mut hits = Vec::new();
        for source in SOURCES {
            if repo.is_some_and(|r| r != source.repo) {
                continue;
            }
            if path_filter.is_some_and(|f| !source.file.contains(f)) {
                continue;
            }
            let mut matched: Vec<CodeLineMatch> = Vec::new();
            let mut raw_count = 0;
            for (idx, &(_line, text)) in source.lines.iter().enumerate() {
                let ok = if let Some(re) = &re {
                    re.is_match(text)
                } else {
                    text.to_ascii_lowercase().contains(&needle)
                };
                if !ok {
                    continue;
                }
                raw_count += 1;
                let from = idx.saturating_sub(context);
                let to = (idx + context + 1).min(source.lines.len());
                for &(ctx_line, ctx_text) in &source.lines[from..to] {
                    if let Some(existing) = matched.iter_mut().find(|m| m.line == ctx_line) {
                        existing.matched |= ctx_line == source.lines[idx].0;
                    } else {
                        matched.push(CodeLineMatch {
                            line: ctx_line,
                            text: ctx_text.to_string(),
                            matched: ctx_line == source.lines[idx].0,
                        });
                    }
                }
            }
            if matched.is_empty() {
                continue;
            }
            let dir = source
                .file
                .split('/')
                .next()
                .unwrap_or("(root)")
                .to_string();
            *dirs.entry(dir).or_default() += raw_count;
            let node = NODES
                .iter()
                .find(|n| n.repo == source.repo && n.file == source.file)
                .unwrap_or(&NODES[0]);
            hits.push(CodeSearchHit {
                node_id: node.id.to_string(),
                name: node.name.to_string(),
                label: node.label.to_string(),
                file_path: source.file.to_string(),
                start_line: node.line,
                score: raw_count as f32,
                matches: matched,
            });
        }
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.file_path.cmp(&b.file_path))
        });
        hits.truncate(limit);
        let mut directories: Vec<DirectoryCount> = dirs
            .into_iter()
            .map(|(dir, count)| DirectoryCount { dir, count })
            .collect();
        directories.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.dir.cmp(&b.dir)));
        Ok(CodeSearchResult { hits, directories })
    }

    fn find_definition(&self, repo: Option<&str>, symbol: &str) -> anyhow::Result<Vec<SearchHit>> {
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
            "fixture analyze: queued indexing for {repo_path} (nodes=5 edges=4, no-op)"
        ))
    }

    fn detect_changes(
        &self,
        repo: Option<&str>,
        scope: &str,
        base_ref: Option<&str>,
    ) -> anyhow::Result<ChangeDetection> {
        let repo_name = repo.unwrap_or("fixture");
        let range = ChangedRange {
            file_path: "src/handler.rs".into(),
            start_line: 12,
            end_line: 13,
        };
        Ok(ChangeDetection {
            repo: repo_name.into(),
            scope: scope.into(),
            base_ref: base_ref.map(str::to_string),
            ranges: vec![range.clone()],
            symbols: vec![ChangedSymbol {
                node_id: "fixture:fn:handle_request".into(),
                name: "handle_request".into(),
                label: "Function".into(),
                file_path: "src/handler.rs".into(),
                start_line: 12,
                end_line: 14,
                ranges: vec![range],
            }],
        })
    }

    fn route_map(
        &self,
        repo: Option<&str>,
        route: Option<&str>,
    ) -> anyhow::Result<Vec<RouteMapEntry>> {
        Ok(fixture_routes(repo, route))
    }

    fn tool_map(
        &self,
        repo: Option<&str>,
        tool: Option<&str>,
    ) -> anyhow::Result<Vec<ToolMapEntry>> {
        Ok(fixture_tools(repo, tool))
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
                p.steps
                    .iter()
                    .find(|(i, _)| *i == idx)
                    .map(|&(_, step)| ProcessHit {
                        process_id: p.id.to_string(),
                        name: p.name.to_string(),
                        process_type: p.process_type.to_string(),
                        step: Some(step),
                        step_count: Some(p.steps.len() as u32),
                    })
            })
            .collect())
    }

    fn query_enrichment(
        &self,
        repo: Option<&str>,
        node_ids: &[String],
        include_content: bool,
    ) -> anyhow::Result<HashMap<String, QueryEnrichment>> {
        let mut out = HashMap::new();
        for id in node_ids {
            let processes = self.processes_of(repo, id)?;
            let Some(node) = NODES
                .iter()
                .find(|n| n.id == id.as_str() && Self::node_in_repo(n, repo))
            else {
                continue;
            };
            out.insert(
                id.clone(),
                QueryEnrichment {
                    processes,
                    module: Some("IO Pipeline".into()),
                    cohesion: 0.8,
                    content: include_content.then(|| format!("fn {}() {{}}", node.name)),
                },
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callers_bfs_depth() {
        let b = FixtureBackend::fixture();
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
        let b = FixtureBackend::fixture();
        assert!(b.search(Some("beta"), "main", 10).unwrap().len() == 1);
        assert!(b.find_definition(Some("beta"), "main").unwrap().is_empty());
    }

    #[test]
    fn processes_of_membership() {
        let b = FixtureBackend::fixture();
        let hits = b.processes_of(None, "fixture:fn:handle_request").unwrap();
        assert_eq!(hits.len(), 2, "handle_request 在两条执行流里");
        assert_eq!(hits[0].process_id, "fixture:proc:request-flow");
        assert_eq!(hits[0].process_type, "call_chain");
        assert_eq!(hits[0].step, Some(2));
        assert_eq!(hits[0].step_count, Some(4));
        assert_eq!(hits[1].process_id, "fixture:proc:output-flow");
        assert_eq!(hits[1].step_count, Some(3));

        // repo 过滤：beta 仓库里没有 fixture 的节点。
        assert!(b
            .processes_of(Some("beta"), "fixture:fn:handle_request")
            .unwrap()
            .is_empty());
        // 未知节点 → 空 Vec 而非错误（合同：查不到不是错误）。
        assert!(b.processes_of(None, "nope").unwrap().is_empty());
        // 不在任何流程里的节点 → 空 Vec。
        assert!(b
            .processes_of(None, "beta:fn:beta_main")
            .unwrap()
            .is_empty());
    }
}
