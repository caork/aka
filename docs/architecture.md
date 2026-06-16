# aka 架构设计

立项 2026-06-10。当前方向：使用第一方 AKA engine 原生 C 引擎负责多语言 tree-sitter/LSP 解析并写入 SQLite；aka 保留 Rust 存储、搜索、服务和桌面体验，通过 SQLite->NDJSON adapter 维持稳定工件合同。

## 总体策略

Rust workspace 承担存储、搜索、服务、UI；AKA engine 作为 native C binary 进程运行，解析结果先落到 engine SQLite，再由 `aka-core` adapter 导出为 NDJSON 工件。两者之间用工件合同隔离（见 [contracts/artifacts.md](contracts/artifacts.md)）。

不在 Rust 侧手写多语言 parser，也不引入 WASM tree-sitter worker 池。解析层使用 AKA engine 的 native C 实现，aka 只承担 adapter 和后续索引，避免 JS 解析运行时进入 indexing 热路径。

## 解析引擎层（engine/，AKA engine native C）

- 来源：AKA engine 是第一方组件，含 MIT 派生代码；维护仓库为 `caork/aka-engine`，不追求和官方仓兼容。
- 产物：`engine/aka-engine`（Windows 为 `aka-engine.exe`）是给 Docker/Tauri/内部 runtime 使用的原生二进制；源码 checkout/build 目录为忽略文件，不随 aka 主仓库入 git。
- 调用：`aka-engine cli --progress --json index_repository <json>`；`AKA_ENGINE_CACHE_DIR` 指向 repo 级 engine cache，`AKA_ENGINE_MODE=fast|moderate|full` 控制解析深度。
- Adapter：AKA engine 维护 SQLite graph；`aka-core` 读取 engine SQLite，导出 `manifest.json`、`nodes.ndjson`、`edges.ndjson`、可选 `chunks.ndjson`，再进入既有 Rust ingest。
- 同步：日常直接在 `engine/aka-engine-src/` 修改 C 源码并提交到维护 fork，`scripts/sync-engine.sh` 默认构建当前 checkout；月度/显式上游评估才用 `scripts/sync-engine.sh --refresh-upstream` 或手工 merge/rebase 上游，再选择性吸收有价值 feature。工件合同不变则 Rust 搜索/图/服务层零改动。

## 存储 + 搜索层（Rust，性能主战场）

性能目标：解析层脱离旧 JS 运行时后，aka 侧继续把全文检索、图遍历和服务面放在 Rust 热路径。

- 全文检索 tantivy：代码感知 tokenizer（camelCase/snake_case 拆分 + 原词保留 + 可选 trigram 模糊），毫秒级 BM25，原生增量 commit，配合 fileHashes 增量索引。
- 向量 usearch（HNSW）：本地持久化，百万级切块在 16GB 机器可承受。
- 混排 RRF（K=60）：移植上游 mergeWithRRF 逻辑。
- 图存储：SQLite（rusqlite）持久 + 启动构建内存 CSR 邻接；callers/callees/impact 走内存遍历。aka 服务面不暴露 Cypher 查询语义，仍走 Rust 邻接 API。
- 元数据 SQLite：repo 注册表、文件哈希、parse cache 索引。
- embedding：**本地优先（fastembed ONNX）且默认关闭**（已拍板）。默认纯 BM25；设置中手动开启 → 下载模型 → 回填向量 → 启用混合检索；关闭则回退 BM25，向量索引保留。Jensen LiteLLM 远程为高级选项。

## 服务层（Rust）

- MCP：rmcp（官方 SDK），stdio + Streamable HTTP。十九个工具：list_repos / query / search_code / context / find_definition / search_references / impact / rename / detect_changes / route_map / tool_map / graphql_map / topic_map / shape_check / api_impact / analyze / import_repo / update_repo / augment（cypher 已砍）。
- GitNexus-like 能力：query/context/impact 会消费合成 Community/Process/Command/Config/Migration/Transaction，新增 detect_changes/route_map/tool_map/graphql_map/topic_map/shape_check/api_impact 消费 Route/GraphQL/Tool/Topic/Channel/Command/Config/Table/Repository/Migration/FETCHES/HANDLES_ROUTE/HANDLES_GRAPHQL/HANDLES_TOOL/CONSUMES_TOPIC/PUBLISHES_TOPIC/HANDLES_COMMAND/USES_CONFIG/MIGRATES_TABLE/HAS_TRANSACTION_BOUNDARY/ENTRY_POINT_OF/STEP_IN_PROCESS 等索引语义，覆盖流程分组、改动到流程映射、API 路由/GraphQL operation/工具入口、消息 topic/queue/channel 生产消费关系、CLI/management command、配置/env/settings、schema migration、事务边界和响应形状检查。它是面向 agent 工作流的保守兼容层，不提供完整 GitNexus 图模型、Cypher 查询或完全等价的跨语言语义。
- HTTP：axum，承接 query/repos/graph-stream/search-code/detect-changes/route-map/tool-map/graphql-map/topic-map/shape-check/api-impact REST 面，给远程模式和浏览器。
- augmentation（编辑器 hook 增强）：BM25-only 路径，目标 <100ms。

## 桌面 + 前端（Tauri 2）

- React 19 + Vite + Tailwind 4 + shadcn/ui。
- 图谱可视化 Cosmograph（GPU/WebGL）+ 分层 LOD（已拍板，要求美观 + 高性能 + 支撑十亿级数据）：数据层按十亿级设计（磁盘索引、流式摄取、天花板在磁盘）；渲染层永远只画聚合视图——默认社区/模块级聚合图（数千节点），下钻文件层、符号层，每层视口内元素控制在 GPU 舒适区。aka-graph 提供 LOD/聚合查询（分层快照）。
- 前端直接 invoke Rust 命令；索引进度走 Tauri event（AKA engine stdout + adapter 阶段事件透传）。

## 部署形态（一套核心，三种交付）

| 形态 | 内容 | 场景 |
|---|---|---|
| 桌面 app | Tauri（UI + core + AKA engine 全打包） | Mac 日常使用，本地分析 |
| 插件 fallback / MCP stdio | 同一 core 的内部宿主入口 | Claude Code / OpenCode / Codex 集成 |
| headless daemon | core + axum，Docker 化 | Jensen 部署，远程模式 |

桌面 app 内置"连接远程 daemon"模式，替代 fork 做过又删掉的 service/client 抽象。

## 里程碑

- M0 地基：AKA engine native + SQLite->NDJSON 工件层 + demo/golden 冒烟
- M1 Rust 索引核心：摄取 + tantivy + usearch + CLI，新旧基准对比
- M2 MCP 工具面齐平并扩展 search_code，Claude Code dogfood，旧版退役
- M3 Tauri 桌面 MVP
- M4 headless + Docker + 远程模式；wiki/group/process 语义按需在 Rust 层补齐

## 风险

- License：AKA engine 含 MIT 派生代码，aka 按 MIT 口径打包；Rust 侧与 engine 以进程边界 + 工件合同隔离，将来如需换血只换 engine adapter。
- 上游漂移：合同字段只增不改；上游 feature 只选择性吸收，先通过 adapter/golden 冒烟确认。
- 跨平台打包：macOS/Linux 直接内置 `aka-engine`，Windows 内置 `aka-engine.exe`；Tauri 与 Docker 的解析链路都不再依赖 JS runtime。
