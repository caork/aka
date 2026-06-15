---
name: aka-code-graph
description: 用 aka 代码知识图谱（14 个 MCP 工具：list_repos/query/search_code/context/find_definition/search_references/impact/detect_changes/route_map/tool_map/shape_check/api_impact/analyze/augment）高效检索和理解仓库；stdio MCP 会自动为当前工作区排队索引。当需要在大代码库里找符号定义、搜实现、评估改动影响面（blast radius）、检查当前改动、或查看 API route/tool 映射时使用。比逐文件 grep/read 更省 token、更准。
---

# aka 代码知识图谱使用策略

aka 把仓库解析成「符号节点 + 调用/引用/应用语义边」的图，并建了 BM25 全文索引。十四个 MCP 工具覆盖四类任务：**检索**（query/search_code/augment）、**定位**（find_definition/context/search_references）、**影响分析**（impact/detect_changes/api_impact/shape_check）、**应用映射**（route_map/tool_map），外加 **管理**（list_repos/analyze）。

其中 Route/Tool/FETCHES/HANDLES_ROUTE/HANDLES_TOOL/ENTRY_POINT_OF/STEP_IN_PROCESS 等是 GitNexus-like 的索引语义：可用于流程分组、API/工具入口、消费者和响应字段检查，但不是完整 GitNexus 图模型、Cypher 查询或完全等价的跨语言语义层。索引缺少相应节点/边/字段时，相关工具会返回空结果或提示缺数据。

> OpenCode 里 MCP 工具按 `<server>_<tool>` 命名：上面十四个工具显示为 `aka_list_repos`、`aka_query` 等。下文用短名。

## 第一步：永远先 list_repos

调用任何检索工具前先 `list_repos`。stdio MCP 启动时会自动发现当前工作区，并在缺少索引时排队分析，所以第一次看到当前仓库 `status: "indexing"` 是正常的：

- `status: "indexing"` → 稍后重试；`"failed"` → 看 `detail` 字段，必要时用 `analyze` 重建。
- 目标仓库不在列表里 → 如果它不是 MCP 当前工作区，用 `analyze`（参数 `repo_path` 必须是**绝对路径**）触发索引；当前工作区通常已被自动排队。
- 多仓库时，后续所有工具都带 `repo` 参数（用 list_repos 返回的 `name`）锁定范围，避免跨库噪音。

## 选哪个工具：决策表

| 你想做什么 | 用 | 不要用 |
|---|---|---|
| "这个库里处理 X 的代码在哪" （模糊、关键词式） | `query` | 逐目录 grep |
| "我要看包含 X 的原始代码行/上下文/目录分布" | `search_code` | query（会按符号聚合，缺少 raw line 证据） |
| "符号 Foo 定义在哪个文件哪一行"（**知道确切名字**） | `find_definition` | query（会混入近似命中） |
| "Foo 是干嘛的、谁调它、它调谁"（陌生符号建立全景） | `context`（一次拿到定义+callers+callees+引用） | 连续调三四个单项工具 |
| "谁直接用了 Foo"（一跳引用清单） | `search_references` | impact（多跳，结果更大） |
| "改/删 Foo 会波及哪些代码"（重构前评估） | `impact`（传 `depth`，默认够用，最大 10） | search_references（只看一跳会低估） |
| "当前 git 改动碰到了哪些符号/流程" | `detect_changes`（scope: unstaged/staged/all/compare） | 手动 diff 后凭感觉判断 |
| "改 API route 前看 handler、消费者、响应字段、流程" | `route_map`，再 `api_impact` | 只 grep 路由字符串 |
| "检查消费者访问的字段是否在 route 响应里" | `shape_check` | 把空结果当成无风险证明 |
| "查 MCP/RPC/agent tool 定义和 handler" | `tool_map` | 只搜工具名 |
| 编辑器钩子/自动补充上下文（要快要省） | `augment`（top-3 命中 + 各自一跳邻居） | context（更重） |

经验法则：

- **名字确切 → find_definition；名字模糊 → query。** query 是代码感知分词的 BM25（默认纯 BM25，无语义向量），关键词选「会出现在标识符或代码里的词」，如 `parse ndjson stream` 而不是自然语言整句。
- **要 grep-like 证据 → search_code。** 它返回原始匹配行、上下文和顶层目录分布；适合确认字符串/配置/API path 是否真的出现。
- **探索陌生符号首选 context**，一次调用顶四次，token 最划算。
- **动手重构前必跑 impact**：结果里 `depth` 是反向依赖的跳数，depth=1 是直接调用方（必须逐个检查），depth≥2 是传递波及（扫一眼判断是否行为变化会穿透）。`count` 很大时说明是热点符号，考虑兼容性包装而非直接改签名。
- **提交前或接手别人改动时跑 detect_changes**：默认看 `unstaged`，也可用 `staged`、`all`、`compare + base_ref`。它把 diff hunk 映射到已索引符号，并列出受影响流程。
- **API/工具类改动先看应用语义图**：`route_map` 看 Route 节点、handler、middleware、consumers、responseKeys/errorKeys 和 flows；`tool_map` 看 Tool 节点、定义文件、description、handlers 和 flows。`shape_check` 依赖 Route responseKeys/errorKeys 与 FETCHES 访问字段元数据；空结果通常表示索引没有足够 shape 数据，不等于没有 API 风险。

## 怎么读输出

所有工具返回紧凑 JSON（短字段名，为省 token 设计）：

- **query**：返回 `{processes, process_symbols, definitions, hits}`。优先读 `processes`（执行流，含 `summary/priority/symbol_count/process_type/step_count`），再看对应 `process_symbols`（`id/name/type/filePath/startLine/step_index/module?`）；`definitions` 是不在流程里的独立定义；`hits` 只是兼容旧客户端的扁平命中。
- **find_definition**：返回 `{defs:[{id, name, label, file, line, score, snip?}]}` — 知道确切符号名时用它定位定义。
- **源码行命中**（search_code）：`{hits:[{id,name,label,file,line,score,matches:[{line,text,matched}]}], directories:[{dir,count}]}` — `matched=true` 是原始命中行，`matched=false` 是上下文；`directories` 用于判断命中集中在哪个模块。
- **图引用**（search_references/impact）：`{id, name, label, file, line, edge, depth}` — `edge` 是关系类型（CALLS/IMPORTS…），`depth` 是跳数。
- **context**：分组返回 `defs` / `callers` / `callees` / `refs` / `processes`。
- **detect_changes**：返回 `{changed_ranges, changed_symbols, changed_count, affected_processes}`；`affected_processes` 里看 `first_affected_step` 和 `affected_symbols`。
- **route_map**：返回 `{routes,total,message?}`；每个 route 含 `route/handler/middleware/responseKeys/errorKeys/consumers/flows`。
- **tool_map**：返回 `{tools,total,message?}`；每个 tool 含 `name/filePath/description/handlers/flows`。
- **shape_check**：返回 `{routes,total,routesWithShapes,mismatches?,message}`；`status:"MISMATCH"` 和 consumer `mismatched` 是需要核查的字段。
- **api_impact**：传 `route` 或 `file`，返回单个 `route` 或多条 `routes`；重点看 `consumers`、`mismatches`、`executionFlows`、`impactSummary.riskLevel`。

拿到 `file:line` 后只 read 命中的那一小段，不要整文件读——这是用 aka 的全部意义。

## 反模式

- ❌ 不先 list_repos 就 query（可能正处于自动索引中，先看 status/progress）。
- ❌ 用 query 找确切符号名（用 find_definition）。
- ❌ 用自然语言长句喂 query（用代码里会出现的关键词）。
- ❌ 重构只看 search_references 一跳就动手（用 impact）。
- ❌ 把 Route/Tool/shape 工具的空结果当成"没有调用方/没有风险"（可能只是索引缺少应用语义数据）。
- ❌ 把 GitNexus-like 能力描述成完整 GitNexus/Cypher 等价实现。
- ❌ 拿到命中后仍然整文件 read（只读 file:line 附近）。
