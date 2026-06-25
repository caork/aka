# aka 架构设计

立项 2026-06-10。当前方向：使用第一方 AKA engine 原生 C 引擎负责基础多语言解析，并以“engine / SCIP / stack-graphs / LSP adapter 直接产出 `aka-facts`”作为唯一运行路径。Rust 侧保留存储、搜索、服务和桌面体验，并把 facts 直接写入 graph/search。

## 总体策略

Rust workspace 承担存储、搜索、服务、UI。唯一热路径合同是 `aka-facts`（见 [contracts/artifacts.md](contracts/artifacts.md)）：engine、SCIP importer、tree-sitter stack-graphs adapter、LSP adapter 都产出可重放 facts，graph/search writer 直接消费 `FactSource`。旧 AKA engine binary、facts sidecar NDJSON、engine SQLite artifact adapter 不再作为 fallback 或调试通道。

不在 Rust 侧手写多语言 parser，也不引入 WASM tree-sitter worker 池。解析层优先使用 AKA engine embedded/direct fact API；语言生态已有能力通过 SCIP、stack-graphs 或成熟 LSP 接入。后续 enrichment 只接 rust-analyzer / pyright / jdtls / typescript-language-server / gopls 这类热门开源实现，作为 baseline index ready 之后的可跳过事实源；失败、超时或缺 provider 都不能影响 graph/search 可用。

## 解析引擎层（engine/，AKA engine native C）

- 来源：AKA engine 是第一方组件，含 MIT 派生代码；维护仓库为 `caork/aka-engine`，不追求和官方仓兼容。
- 产物：运行产物是 embedded/direct fact API（macOS/Linux static library、Windows `aka_engine.dll` + header）。源码 checkout/build 目录为忽略文件，不随 aka 主仓库入 git；旧 `engine/aka-engine` / `aka-engine.exe` 不再作为运行时 fallback。
- 调用：主线由 Rust 调度并行文件任务，engine/library 回调或返回 `aka-facts`。
- Adapter：索引入口是 `index_facts*`，不要求 `nodes.ndjson` / `edges.ndjson` 落盘；legacy artifact adapter 不再是产品/runtime 路径。
- 同步：日常直接在 `engine/aka-engine-src/` 修改 C 源码并提交到维护 fork，`scripts/sync-engine.sh` 默认构建当前 checkout；月度/显式上游评估才用 `scripts/sync-engine.sh --refresh-upstream` 或手工 merge/rebase/cherry-pick 选择性吸收有价值 feature。fact ABI/API 版本不变则 Rust 搜索/图/服务层零改动。

## 存储 + 搜索层（Rust，性能主战场）

性能目标：解析层脱离旧 JS 运行时后，aka 侧继续把全文检索、图遍历和服务面放在 Rust 热路径。

- 全文检索 tantivy：代码感知 tokenizer（camelCase/snake_case 拆分 + 原词保留 + 可选 trigram 模糊），毫秒级 BM25，原生增量 commit，配合 fileHashes 增量索引。
- 向量 usearch（HNSW）：本地持久化，百万级切块在 16GB 机器可承受。
- 混排 RRF（K=60）：移植上游 mergeWithRRF 逻辑。
- 图存储：SQLite（rusqlite）持久 + 启动构建内存 CSR 邻接；callers/callees/impact 走内存遍历。aka 服务面不暴露 Cypher 查询语义，仍走 Rust 邻接 API。
- 元数据 SQLite：repo 注册表、文件哈希、parse cache 索引。
- embedding：**暂不作为当前重点，默认关闭**（已拍板）。默认纯 BM25；未来只有用户在设置中手动开启后才下载模型、回填向量、启用混合检索；关闭则回退 BM25，向量索引保留。

## 服务层（Rust）

- MCP：rmcp（官方 SDK），stdio + Streamable HTTP。十九个工具：list_repos / query / search_code / context / find_definition / search_references / impact / rename / detect_changes / route_map / tool_map / graphql_map / topic_map / shape_check / api_impact / analyze / import_repo / update_repo / augment（cypher 已砍）。
- 应用语义能力：query/context/impact 会消费 facts 中已存在的 Route/GraphQL/Tool/Topic/Channel/Command/Config/Table/Repository/Migration/Process/Community 等节点和边；detect_changes/route_map/tool_map/graphql_map/topic_map/shape_check/api_impact 只是这些 facts 的查询视图。它不再依赖 Rust 侧自研 synthesis/enrichment 阶段，也不提供完整 GitNexus 图模型、Cypher 查询或完全等价的跨语言语义。
- HTTP：axum，承接 query/repos/graph-stream/search-code/detect-changes/route-map/tool-map/graphql-map/topic-map/shape-check/api-impact REST 面，给远程模式和浏览器。
- augmentation（编辑器 hook 增强）：BM25-only 路径，目标 <100ms。

## 桌面 + 前端（Tauri 2）

- React 19 + Vite + Tailwind 4 + shadcn/ui。
- 图谱可视化 Cosmograph（GPU/WebGL）+ 分层 LOD（已拍板，要求美观 + 高性能 + 支撑十亿级数据）：数据层按十亿级设计（磁盘索引、流式摄取、天花板在磁盘）；渲染层永远只画聚合视图——默认社区/模块级聚合图（数千节点），下钻文件层、符号层，每层视口内元素控制在 GPU 舒适区。aka-graph 提供 LOD/聚合查询（分层快照）。
- 前端直接 invoke Rust 命令；索引进度走 Tauri event（facts discover/parse/fuse、graph/search、optional LSP enrichment 分阶段展示）。不再展示 legacy `export-artifacts` 作为运行阶段。

## 部署形态（一套核心，三种交付）

| 形态 | 内容 | 场景 |
|---|---|---|
| 桌面 app | Tauri（UI + core + AKA engine 全打包） | Mac 日常使用，本地分析 |
| 插件 / MCP stdio | 同一 core 的内部宿主入口 | Claude Code / OpenCode / Codex 集成 |
| headless daemon | core + axum，Docker 化 | Jensen 部署，远程模式 |

桌面 app 内置"连接远程 daemon"模式，替代 fork 做过又删掉的 service/client 抽象。

## 里程碑

- M0 地基：AKA engine embedded/direct facts contract + demo/golden 冒烟
- M1 Rust 索引核心：摄取 + tantivy + usearch + 内部 runtime 验证
- M2 MCP 工具面齐平并扩展 search_code，Claude Code dogfood，旧版退役
- M3 Tauri 桌面 MVP
- M4 headless + Docker + 远程模式；LSP/SCIP/stack-graphs provider 按热门开源实现逐个接入，并以大仓基准决定是否默认可用

## 风险

- License：AKA engine 含 MIT 派生代码，aka 按 MIT 口径打包；embedded/direct API 没有进程边界隔离，因此 engine 侧内存 ownership、错误传播和 panic/abort 策略必须由 FFI 合同约束。
- 上游漂移：facts 字段只增不改；上游 feature 只选择性吸收，先通过 fact/golden 冒烟确认。engine SQLite schema 不再是 aka-core 主合同。
- 跨平台打包：macOS/Linux 使用 embedded static lib，Windows 单文件 `AKA.exe` 内置 `aka_engine.dll` 并通过 embedded/direct-facts 路径驱动 engine；Tauri 与 Docker 的解析链路都不再依赖 JS runtime，也不依赖外置 `aka-engine.exe`。
