# aka

感知所有代码的知识引擎（名字源自 Akasha records）：AKA engine native C 解析 → SQLite->NDJSON adapter → Rust 索引（tantivy BM25 + SQLite/CSR 图）→ MCP / HTTP / 液态玻璃桌面端 + 插件包。私有库 `caork/aka`。

**工作路径**：本仓库 `~/Documents/github/aka`；AKA engine 源码维护在 `engine/aka-engine-src/`，由 `scripts/sync-engine.sh` 构建产出 `engine/aka-engine` / `engine/aka-engine.exe`；运行时数据在 `~/.aka/`（registry.json + repos/<slug-hash>/{artifact,graph.db,search}/ + checkouts/）。

## 硬约束（必须遵守）

- **License**：AKA engine 含 MIT 派生代码；aka 按 MIT 口径接入与分发，保留必要来源说明。
- **AKA engine 是第一方组件，不追求旧上游运行时兼容**：解析能力改动直接在 `engine/aka-engine-src/` 的 C 源码 checkout 中完成并提交。`scripts/sync-engine.sh` 默认只构建当前 checkout，不重置本地分支。只有月度/显式上游评估时使用 `scripts/sync-engine.sh --refresh-upstream` 抓取 aka engine fork 和 upstream，然后手工 merge/rebase/cherry-pick 选择性吸收有价值 feature；脚本不得自动 reset/clean 维护分支。`engine/aka-engine*` 二进制不入 git，`engine/ENGINE_SHA` 锚定当前维护的 engine commit；engine commit 变化后运行 `scripts/pin-engine-ref.sh` 同步 Docker/release 的 `AKA_ENGINE_REF`。
- **工件合同** `docs/contracts/artifacts.md` 是 engine adapter↔Rust 的唯一接口：字段只增不改不删；破坏性变更必须 `contractVersion` +1 并双侧同步，Rust 侧永不 import engine 内部模块。
- **embedding 默认关闭**（用户拍板）：默认纯 BM25；开启只能由用户在 per-repo 设置里手动操作，代码里不许悄悄打开。
- **图查询不引 Cypher / 嵌入式图数据库**（用户拍板）：aka 服务面使用 SQLite 持久 + 内存 CSR 邻接，就这一条路。
- **产品形态只有桌面版 + 插件包**（用户拍板）：对外发布、文档、汇报、阶段状态和发包说明不要再说 aka CLI / CLI 版 / 裸 CLI，也不要写"aka-cli 自身编译/发包"这类口径。即使 cargo/CI 日志里出现 `aka-cli`，也只能称为"内部 runtime crate 编译/验证"。`apps/cli`、`aka-cli` crate、`AKA mcp/analyze/serve` 子命令只视为桌面包/插件 fallback/headless Docker 的内部宿主与源码调试入口；用户正常使用路径是启动 AKA 桌面端和安装 Claude Code / OpenCode / Codex 插件/配置包。
- **发布/阶段状态口径检查**：任何阶段性汇报、发包清单、CI 说明、邮件和 PR 描述都必须围绕"桌面版"与"插件包"展开；如果需要提到 `cargo build -p aka-cli`、`aka-cli` 或 `apps/cli`，只能写成"内部 runtime/宿主 crate 的编译验证"，不能把它列为用户可见产品、独立交付物或发布步骤。
- **渲染性能红线**：WebGL 渲染器每帧 draw call O(1)（与图规模无关），pan/zoom 60fps；动 `apps/desktop/src/graph/renderer.ts` 后必须实测 FPS（页面右下角徽章）。
- **验证 Web/UI 用浏览器实际渲染**（Playwright MCP 打开页面看真实结果），不要只凭 curl 下结论。

## 架构

```
仓库源码 ─AKA engine native C binary─▶ engine SQLite ─aka-core adapter─▶ NDJSON 工件 ─aka-core 摄取─▶
  ├ aka-graph: SQLite(nodes/edges/positions) + CSR 邻接 + phyllotaxis 布局 / LOD / ego
  ├ aka-search: tantivy BM25(代码感知分词) + usearch 向量 + RRF(K=60)
  └ 服务面: aka-mcp(rmcp 工具面, Backend trait 接缝) + aka-server(axum :4111)
       ▲ apps/cli = 内部运行时/宿主 crate（实现真实 Backend + 后台导入/更新任务；不是对外产品形态）
       ▲ apps/desktop = Tauri2 + React19 + 自研 WebGL2 渲染器(50万节点/百万边 60fps)
```

- `crates/aka-core`：合同类型、流式 NDJSON 读取、注册表（~/.aka/registry.json）、EngineRunner（spawn AKA engine、读取 engine SQLite、导出 artifacts、解析 stdout 进度事件）。
- `crates/aka-graph`：store(摄取/查询)、adjacency(callers/callees/impact)、layout(确定性两级 phyllotaxis)、lod、ego(BFS 分环径向)。
- `crates/aka-mcp`：`Backend` trait 是数据层接缝（mock 可测）；工具面包括 list_repos/query/search_code/context/find_definition/search_references/impact/rename/detect_changes/route_map/tool_map/graphql_map/topic_map/shape_check/api_impact/analyze/import_repo/update_repo/augment。
- `crates/aka-server`：REST 面（repos 的导入/更新/设置/删除、query、symbol/context、graph/lod、graph/ego、node），CORS 仅 localhost。
- `apps/desktop`：三视图 Search/Graph/Symbol 全接真实数据（serve 离线时回退 demo/mock）；图渲染静态布局无力导，LOD 三档 + 标签 Canvas2D overlay + 网格拾取。

## 跑起来（源码开发 / 内部调试）

用户侧只交付 AKA 桌面端和插件包；下面命令只用于源码开发、CI、stdio fallback 或 headless Docker 调试，不作为独立 CLI 产品宣传。状态汇报里要写"内部 runtime 编译/验证"，不要写"aka-cli 自身编译"。

```bash
cargo build -p aka-cli                 # 内部 runtime crate 编译验证，不是产品发包
./target/debug/aka analyze <repo>      # 内部调试：AKA engine 解析 + SQLite->NDJSON adapter + 索引
./target/debug/aka serve &             # 内部调试：HTTP :4111（桌面端数据源）
cd apps/desktop && npm run dev         # UI :5188（HMR；自动连 serve）
./target/debug/aka mcp                 # 插件 fallback：MCP stdio（优先用桌面端 HTTP MCP）

# engine 首次初始化/构建 AKA engine native binary
scripts/sync-engine.sh

# 提交门槛；发包门槛以桌面端和插件包为准，aka-cli 只作为内部 runtime 被 workspace 验证覆盖
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
cd apps/desktop && npm run build
```

## 任务流程（每个任务必走）

1. **需求/问题分析**：把"要做什么/现象/期望"讲清楚，不明确先和用户对齐。
2. **定位**：找到相关 crate/组件和数据流，确认改动点与影响面（别绕过工件合同和 Backend trait 接缝）。
3. **开发**：**多 agent 并行分模块**是本项目的默认工作方式（用户明确要求，嫌单线程慢）——能拆就拆给后台 agent 并行，按 crate/目录划界避免文件冲突，主线程做集成。
4. **拉取最新代码**：commit 前 `git fetch` 同步 `main`，别在过时基线上提交。
5. **提交与工作区卫生**：**一个独立小任务完成后必须立刻单独提交一个 commit**，不要把多个任务/修复攒成一个大提交，也不要把修改遗留在工作区成为无人看管的改动；提交时只纳入本任务相关文件，保持工作区尽量干净。过上面的提交门槛后按规范 commit：`<类型>: <描述>`（feat/fix/refactor/docs/infra），结尾 `Co-Authored-By` 署名。
6. **合入**：**一个完整大任务完成并验证通过后必须及时合入目标分支**，不要让已完成分支长期漂在外面；合入前再次同步目标分支并确认工作区没有本任务遗留改动。
7. **验证**：浏览器真实渲染/接口冒烟确认达标；UI 改动截图；没过回到第 3 步。

## 建议工作方式

- **设计基线（已定稿，别漂移）**：亮色 macOS 液态玻璃——毛玻璃面板（既有 `glass`/`glass-panel`/`badge` 类直接用）、分层柔和阴影、发光蓝 `#2E7CF6` 极克制（主按钮/聚焦/选中节点）、暖橙 `#F6A623` 只做灯塔点缀、framer-motion spring(stiffness≈300, damping≈30) 优雅不浮夸、无弹跳无旋转。
- **大图思路**：数据层按十亿级设计（磁盘索引、流式摄取），渲染层永远只画聚合/截断视图（LOD/ego），单视口控制在 GPU 舒适区。
- **选型不固定**：关键决策先和用户对齐，别拿旧结论绑死（Maya 记忆 `feedback-consult-selection`）。
- 共享记忆在 `~/agent-memory/maya/project_aka.md`（决策史与进度快照），会话开始值得扫一眼。

## 现状

（2026-06-10）M0–M3 完成：AKA engine native 接入 + SQLite->NDJSON 工件合同 v0；Rust 五 crate 全绿（workspace 73 测试 + clippy -D warnings）；MCP/HTTP 双服务；桌面端三视图真实数据 + 仓库全生命周期（git/zip 导入、一键更新、per-repo 设置、删除）+ 节点详情/ego 下钻；WebGL 50 万节点/百万边 60fps 实测。
（2026-06-11）Process 执行流全链路接入：edges.step 摄取（旧库自动迁移）、布局 Processes 专属簇、MCP impact 报 affected_processes（哪条流断在第几步）、query/context 带流程归属、/api/node 对 Process 展开 entry/terminal/steps、桌面端流程详情视图（入口源码+步骤时间线）+ 普通符号"参与流程"。改完需对已有仓库重跑 `aka index` 补 step 数据。
**待办**：M4 Docker 化 + Jensen 部署 + 远程模式；embedding 实现（本地 fastembed，默认关）；增量索引（fileHashes/parse-cache）；Tauri 正式打包（内置 AKA engine native binary）；wiki/group 按需补齐；导入中 update 返回 404 的小语义瑕疵。
