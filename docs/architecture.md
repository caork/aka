# aka 架构设计

立项 2026-06-10。背景：GitNexus fork（caork/GitNexus）代码质量不满意，决定重写，但继承其解析管线。上游（abhigyanpatwari/GitNexus）非常活跃且投入集中在 parser 层（472 个新 commit 中 159 个改 ingestion/tree-sitter/vendor），因此 parser 必须保持可持续 rebase，而非冻结继承。

## 总体策略

Rust workspace 承担存储、搜索、服务、UI；fork 的解析管线收缩为只产工件的 sidecar 进程；两者之间用 NDJSON 工件合同隔离（见 [contracts/artifacts.md](contracts/artifacts.md)）。

不把 parser 移植成 Rust（工作量按年计，且丢上游持续修复）；不用 WASM tree-sitter（性能与 worker 池退化）。sidecar 是唯一同时满足"继承 parser"和"跟住上游"的方案。

## 解析引擎层（engine/，TypeScript，继承）

- 来源：caork/GitNexus 的 `engine/aka` 分支 = 上游 v1.6.7 基底 + Ascend C 文件级移植（不做整体 merge——fork 其余 40 个本地 commit 属于被 aka 取代的 server/web/docker 层）。
- Ascend C 足迹：6 个独立新文件（languages/ascend-c.ts、ascend-c-preprocessor.ts、4 个 extractor config）+ 7 个共享文件的小注册点（languages/index.ts、language-provider.ts、parsing-processor.ts、tree-sitter-queries.ts、parser-loader.ts、entry-point-scoring.ts、framework-detection.ts）+ 集成测试与 fixtures。
- 唯一 fork-local 新模块 `emit.ts`：调 `runPipelineFromRepo`（出口天然解耦，输出内存 KnowledgeGraph，无 DB 耦合），序列化为工件。
- 打包：Bun `--compile` 单二进制（支持嵌 .node 原生插件），作为 Tauri sidecar；开发期 `bun run`。
- 上游同步：每月 rebase `engine/aka` 分支到上游 main，重放 Ascend C + emit 补丁；工件合同不变则 Rust 侧零改动。
- 可选后续移植：fork 的 unresolvedCalls 跟踪（cb5ea681，call-processor/emit-references/process-processor 小改），上游 RING4 重构后需重做，暂缓。

## 存储 + 搜索层（Rust，性能主战场）

旧方案病根：LadybugDB FTS 扩展（上游反复修内存问题）+ transformers.js 运行时。

- 全文检索 tantivy：代码感知 tokenizer（camelCase/snake_case 拆分 + 原词保留 + 可选 trigram 模糊），毫秒级 BM25，原生增量 commit，配合 fileHashes 增量索引。
- 向量 usearch（HNSW）：本地持久化，百万级切块在 16GB 机器可承受。
- 混排 RRF（K=60）：移植上游 mergeWithRRF 逻辑。
- 图存储：SQLite（rusqlite）持久 + 启动构建内存 CSR 邻接；callers/callees/impact 走内存遍历。schema 翻译自上游 lbug-config.ts 的声明式定义。**不保 Cypher**（已拍板），不引 LadybugDB/Kuzu FFI。
- 元数据 SQLite：repo 注册表、文件哈希、parse cache 索引。
- embedding：**本地优先（fastembed ONNX）且默认关闭**（已拍板）。默认纯 BM25；设置中手动开启 → 下载模型 → 回填向量 → 启用混合检索；关闭则回退 BM25，向量索引保留。Jensen LiteLLM 远程为高级选项。

## 服务层（Rust）

- MCP：rmcp（官方 SDK），stdio + Streamable HTTP。八个工具：list_repos / query / context / find_definition / search_references / impact / analyze / augment（cypher 已砍）。
- HTTP：axum，承接 query/repos/graph-stream REST 面，给远程模式和浏览器。
- augmentation（编辑器 hook 增强）：BM25-only 路径，目标 <100ms。

## 桌面 + 前端（Tauri 2）

- React 19 + Vite + Tailwind 4 + shadcn/ui。
- 图谱可视化 Cosmograph（GPU/WebGL）+ 分层 LOD（已拍板，要求美观 + 高性能 + 支撑十亿级数据）：数据层按十亿级设计（磁盘索引、流式摄取、天花板在磁盘）；渲染层永远只画聚合视图——默认社区/模块级聚合图（数千节点），下钻文件层、符号层，每层视口内元素控制在 GPU 舒适区。aka-graph 提供 LOD/聚合查询（分层快照）。
- 前端直接 invoke Rust 命令；索引进度走 Tauri event（sidecar stdout NDJSON 透传）。

## 部署形态（一套核心，三种交付）

| 形态 | 内容 | 场景 |
|---|---|---|
| 桌面 app | Tauri（UI + core + sidecar 全打包） | Mac 日常使用，本地分析 |
| `aka` CLI / MCP stdio | 同一 core 的命令行入口 | Claude Code / Cursor 集成 |
| headless daemon | core + axum，Docker 化 | Jensen 部署，远程模式 |

桌面 app 内置"连接远程 daemon"模式，替代 fork 做过又删掉的 service/client 抽象。

## 里程碑

- M0 地基：engine/aka 分支 + emit 工件层 + golden 对比测试（2-3 个真实仓库，含 Ascend C 项目）
- M1 Rust 索引核心：摄取 + tantivy + usearch + CLI，新旧基准对比
- M2 MCP 八工具齐平，Claude Code dogfood，旧版退役
- M3 Tauri 桌面 MVP
- M4 headless + Docker + 远程模式；wiki/group 按需移植

## 风险

- License：上游 PolyForm Noncommercial 1.0，engine 为衍生作品 → 整体非商用（用户已确认不商用）。Rust 侧与 engine 以进程边界 + 工件合同隔离，将来如需换血只换 engine 模块。
- 上游漂移：合同字段只增不改；每月 rebase 节奏。
- Bun compile 嵌十几个 tree-sitter prebuild 体积约 80-120MB；M0 需验证 darwin-arm64 全部 grammar 加载。
