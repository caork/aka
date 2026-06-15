# aka Code Graph Usage For Codex

Use aka MCP tools when you need codebase-level search, symbol context, impact analysis, route/API mapping, GraphQL/tool mapping, or change-risk checks. Prefer aka over broad manual grep/read loops in large repositories.

## First Step

Always call `list_repos` before search/context tools.

aka can index the current project without the user opening the desktop import flow first:

- HTTP MCP asks the Codex client for workspace roots on every tool call and queues any local repo roots for background indexing.
- stdio fallback (`AKA mcp`) also detects the server process working directory and queues that workspace.
- For local repositories, follow-up tool `repo` arguments may be the `list_repos` name or a local root/nested path. If a path is not registered yet, aka resolves it to the workspace root, queues background indexing, and reports that the repo is indexing.
- If the target local repo is not listed, call `analyze` with the repo root, a relative path, or any nested directory inside the repo. aka resolves it to the git/project root, registers it in the shared GUI-visible knowledge base, and schedules background indexing.
- If the target is a remote GitHub/Git repo, call `import_repo` with `kind: "git"` and the clone URL.
- If a repo status is `indexing`, retry `list_repos` later before relying on search results. If status is `failed`, inspect `detail` and retry `analyze` or `update_repo`.
- When multiple repos are listed, pass the `repo` name returned by `list_repos` to all follow-up tools.

## Tool Choice

- Fuzzy code search or "where is X handled?" -> `query`.
- Raw matching lines, config strings, route literals, or grep-like evidence -> `search_code`.
- Exact symbol definition -> `find_definition`.
- A symbol's definition, callers, callees, references, and flows in one call -> `context`.
- Direct references only -> `search_references`.
- Refactor/blast-radius check -> `impact`.
- Current git diff to touched symbols and affected flows -> `detect_changes`.
- Route handlers, consumers, middleware, response keys, and API risk -> `route_map`, then `api_impact` or `shape_check`.
- GraphQL operation/resolver mapping -> `graphql_map`.
- MCP/RPC/agent tool definitions and handlers -> `tool_map`.
- Fast editor/prompt augmentation -> `augment`.
- Explicit local indexing/update -> `analyze` / `update_repo`.

Use code-like query terms. For example, prefer `parse ndjson stream` or `OrderRepository findNative` over long natural-language sentences.

## Reading Results

- `query` returns process groups plus matched symbols. Prefer `processes` and `process_symbols` before the backward-compatible flat `hits`.
- `search_code` returns raw matched lines and surrounding context. Use it when you need evidence that text actually appears.
- `impact` and `search_references` return graph refs with `edge` and `depth`; `depth=1` is direct, larger depth is transitive.
- `detect_changes` maps diff hunks to indexed symbols and affected execution flows.
- Route/GraphQL/tool/shape tools may return empty or "missing data" results when the index lacks that semantic layer. Treat that as unknown risk, not as proof of no callers or no API risk.

After aka gives `file:line`, read only the relevant local slice around that location.

## Avoid

- Do not skip `list_repos` at the start of a task.
- Do not assume a repository must be indexed manually in the desktop UI; prefer automatic roots, then `analyze` as fallback.
- Do not use `query` for exact symbol lookup when `find_definition` fits.
- Do not use only one-hop `search_references` for refactor safety; use `impact`.
- Do not describe aka as full GitNexus/Cypher equivalence. It has GitNexus-like semantic nodes/edges for many Java/Python business-service cases, but tools should report missing semantic data honestly.
