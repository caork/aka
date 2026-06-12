---
name: aka-code-graph
description: 用 aka 代码知识图谱（MCP 工具 list_repos/query/search_code/context/find_definition/search_references/impact/analyze/augment）高效检索和理解已索引仓库。当需要在大代码库里找符号定义、搜实现、评估改动影响面（blast radius）、或快速建立对陌生符号的全景认知时使用。比逐文件 grep/read 更省 token、更准。
---

# aka 代码知识图谱使用策略

aka 把仓库解析成「符号节点 + 调用/引用边」的图，并建了 BM25 全文索引。九个 MCP 工具覆盖三类任务：**检索**（query/search_code/augment）、**定位**（find_definition/context/search_references）、**分析**（impact），外加 **管理**（list_repos/analyze）。

## 第一步：永远先 list_repos

调用任何检索工具前先 `list_repos`，确认目标仓库已索引且 `status: "ready"`：

- `status: "indexing"` → 稍后重试；`"failed"` → 看 `detail` 字段，必要时用 `analyze` 重建。
- 目标仓库不在列表里 → 用 `analyze`（参数 `repo_path` 必须是**绝对路径**）触发索引，索引需要时间，先做别的事再回来。
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
| 编辑器钩子/自动补充上下文（要快要省） | `augment`（top-3 命中 + 各自一跳邻居） | context（更重） |

经验法则：

- **名字确切 → find_definition；名字模糊 → query。** query 是代码感知分词的 BM25（默认纯 BM25，无语义向量），关键词选「会出现在标识符或代码里的词」，如 `parse ndjson stream` 而不是自然语言整句。
- **要 grep-like 证据 → search_code。** 它返回原始匹配行、上下文和顶层目录分布；适合确认字符串/配置/API path 是否真的出现。
- **探索陌生符号首选 context**，一次调用顶四次，token 最划算。
- **动手重构前必跑 impact**：结果里 `depth` 是反向依赖的跳数，depth=1 是直接调用方（必须逐个检查），depth≥2 是传递波及（扫一眼判断是否行为变化会穿透）。`count` 很大时说明是热点符号，考虑兼容性包装而非直接改签名。

## 怎么读输出

所有工具返回紧凑 JSON（短字段名，为省 token 设计）：

- **query**：返回 `{processes, process_symbols, definitions, hits}`。优先读 `processes`（执行流，含 `summary/priority/symbol_count/process_type/step_count`），再看对应 `process_symbols`（`id/name/type/filePath/startLine/step_index/module?`）；`definitions` 是不在流程里的独立定义；`hits` 只是兼容旧客户端的扁平命中。
- **find_definition**：返回 `{defs:[{id, name, label, file, line, score, snip?}]}` — 知道确切符号名时用它定位定义。
- **源码行命中**（search_code）：`{hits:[{id,name,label,file,line,score,matches:[{line,text,matched}]}], directories:[{dir,count}]}` — `matched=true` 是原始命中行，`matched=false` 是上下文；`directories` 用于判断命中集中在哪个模块。
- **图引用**（search_references/impact）：`{id, name, label, file, line, edge, depth}` — `edge` 是关系类型（CALLS/IMPORTS…），`depth` 是跳数。
- **context**：分组返回 `defs` / `callers` / `callees` / `refs` 四段。

拿到 `file:line` 后只 Read 命中的那一小段，不要整文件读——这是用 aka 的全部意义。

## 反模式

- ❌ 不先 list_repos 就 query（撞上未索引仓库白白报错）。
- ❌ 用 query 找确切符号名（用 find_definition）。
- ❌ 用自然语言长句喂 query（用代码里会出现的关键词）。
- ❌ 重构只看 search_references 一跳就动手（用 impact）。
- ❌ 拿到命中后仍然整文件 Read（只读 file:line 附近）。
