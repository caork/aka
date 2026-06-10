# aka 代码知识图谱使用策略

<!--
  本文件是 skills/aka-code-graph/SKILL.md 的"常驻指令"版（无 frontmatter），给两类场景用：
  1. opencode.json 的 instructions 数组引用本文件（推荐这种方式时改名随意）；
  2. 内容追加进项目根 AGENTS.md 或 ~/.config/opencode/AGENTS.md。
  如果你的 OpenCode 支持原生 skills（2026-06 起），优先装 skill（按需加载更省 token），不要两者同时启用。
-->

aka 把仓库解析成「符号节点 + 调用/引用边」的图，并建了 BM25 全文索引。MCP server `aka` 提供八个工具（OpenCode 里显示为 `aka_list_repos`、`aka_query` 等），覆盖三类任务：**检索**（query/augment）、**定位**（find_definition/context/search_references）、**分析**（impact），外加 **管理**（list_repos/analyze）。在已索引仓库里找符号定义、搜实现、评估改动影响面（blast radius）时优先用它们，比逐文件 grep/read 更省 token、更准。

## 第一步：永远先 list_repos

调用任何检索工具前先 `list_repos`，确认目标仓库已索引且 `status: "ready"`：

- `status: "indexing"` → 稍后重试；`"failed"` → 看 `detail` 字段，必要时用 `analyze` 重建。
- 目标仓库不在列表里 → 用 `analyze`（参数 `repo_path` 必须是**绝对路径**）触发索引，索引需要时间，先做别的事再回来。
- 多仓库时，后续所有工具都带 `repo` 参数（用 list_repos 返回的 `name`）锁定范围，避免跨库噪音。

## 选哪个工具：决策表

| 你想做什么 | 用 | 不要用 |
|---|---|---|
| "这个库里处理 X 的代码在哪" （模糊、关键词式） | `query` | 逐目录 grep |
| "符号 Foo 定义在哪个文件哪一行"（**知道确切名字**） | `find_definition` | query（会混入近似命中） |
| "Foo 是干嘛的、谁调它、它调谁"（陌生符号建立全景） | `context`（一次拿到定义+callers+callees+引用） | 连续调三四个单项工具 |
| "谁直接用了 Foo"（一跳引用清单） | `search_references` | impact（多跳，结果更大） |
| "改/删 Foo 会波及哪些代码"（重构前评估） | `impact`（传 `depth`，默认够用，最大 10） | search_references（只看一跳会低估） |
| 编辑器钩子/自动补充上下文（要快要省） | `augment`（top-3 命中 + 各自一跳邻居） | context（更重） |

经验法则：

- **名字确切 → find_definition；名字模糊 → query。** query 是代码感知分词的 BM25（默认纯 BM25，无语义向量），关键词选「会出现在标识符或代码里的词」，如 `parse ndjson stream` 而不是自然语言整句。
- **探索陌生符号首选 context**，一次调用顶四次，token 最划算。
- **动手重构前必跑 impact**：结果里 `depth` 是反向依赖的跳数，depth=1 是直接调用方（必须逐个检查），depth≥2 是传递波及（扫一眼判断是否行为变化会穿透）。`count` 很大时说明是热点符号，考虑兼容性包装而非直接改签名。

## 怎么读输出

所有工具返回紧凑 JSON（短字段名，为省 token 设计）：

- **检索命中**（query/find_definition）：`{id, name, label, file, line, score, snip?}` — `label` 是节点类型（Function/Class/File…），`file`+`line` 直接可用于后续 read，`snip` 是代码片段，往往不用再开文件。
- **图引用**（search_references/impact）：`{id, name, label, file, line, edge, depth}` — `edge` 是关系类型（CALLS/IMPORTS…），`depth` 是跳数。
- **context**：分组返回 `defs` / `callers` / `callees` / `refs` 四段。

拿到 `file:line` 后只 read 命中的那一小段，不要整文件读——这是用 aka 的全部意义。

## 反模式

- ❌ 不先 list_repos 就 query（撞上未索引仓库白白报错）。
- ❌ 用 query 找确切符号名（用 find_definition）。
- ❌ 用自然语言长句喂 query（用代码里会出现的关键词）。
- ❌ 重构只看 search_references 一跳就动手（用 impact）。
- ❌ 拿到命中后仍然整文件 read（只读 file:line 附近）。
