# aka

感知所有代码的知识引擎（GitNexus 的 Rust 重写，名字源自 Akasha records）：tree-sitter 解析（继承的 TS engine 作 sidecar）→ NDJSON 工件 → Rust 索引（tantivy BM25 + SQLite/CSR 图）→ CLI / MCP / HTTP / 液态玻璃桌面端。私有库 `caork/aka`。

**工作路径**：本仓库 `~/Documents/github/aka`；engine 上游开发在 `~/Documents/github/GitNexus-engine`（fork `caork/GitNexus` 的 `engine/aka` 分支 worktree）；运行时数据在 `~/.aka/`（registry.json + repos/<slug-hash>/{artifact,graph.db,search}/ + checkouts/）。

## 硬约束（必须遵守）

- **License**：上游 GitNexus 是 PolyForm Noncommercial 1.0，engine 是其衍生作品 → **整个项目非商用**，不得用于任何商业化场景。
- **engine/ 是 vendored 同步副本，不要直接改它的代码**：parser/emit 的改动必须提交到 fork 的 `engine/aka` 分支（在 GitNexus-engine worktree 里改、测、push **fork**，绝不 push origin 上游），再跑 `scripts/sync-engine.sh` 同步过来。`engine/gitnexus*` 不入 git，`engine/ENGINE_SHA` 锚定来源版本。上游 parser 很活跃，**每月 rebase 一次**而不是冻结。
- **工件合同** `docs/contracts/artifacts.md` 是 engine↔Rust 的唯一接口：字段只增不改不删；破坏性变更必须 `contractVersion` +1 并双侧同步，Rust 侧永不 import engine 内部模块。
- **embedding 默认关闭**（用户拍板）：默认纯 BM25；开启只能由用户在 per-repo 设置里手动操作，代码里不许悄悄打开。
- **图查询不引 Cypher / 嵌入式图数据库**（用户拍板）：SQLite 持久 + 内存 CSR 邻接，就这一条路。
- **渲染性能红线**：WebGL 渲染器每帧 draw call O(1)（与图规模无关），pan/zoom 60fps；动 `apps/desktop/src/graph/renderer.ts` 后必须实测 FPS（页面右下角徽章）。
- **验证 Web/UI 用浏览器实际渲染**（Playwright MCP 打开页面看真实结果），不要只凭 curl 下结论。

## 架构

```
仓库源码 ─engine(TS sidecar, npx tsx emit-cli)─▶ NDJSON 工件 ─aka-core 摄取─▶
  ├ aka-graph: SQLite(nodes/edges/positions) + CSR 邻接 + phyllotaxis 布局 / LOD / ego
  ├ aka-search: tantivy BM25(代码感知分词) + usearch 向量 + RRF(K=60)
  └ 服务面: aka-mcp(rmcp 八工具, Backend trait 接缝) + aka-server(axum :4111)
       ▲ apps/cli = `aka` 二进制(backend.rs 实现真实 Backend + 后台导入/更新任务)
       ▲ apps/desktop = Tauri2 + React19 + 自研 WebGL2 渲染器(50万节点/百万边 60fps)
```

- `crates/aka-core`：合同类型、流式 NDJSON 读取、注册表（~/.aka/registry.json）、EngineRunner（spawn engine、解析 stdout 进度事件）。
- `crates/aka-graph`：store(摄取/查询)、adjacency(callers/callees/impact)、layout(确定性两级 phyllotaxis)、lod、ego(BFS 分环径向)。
- `crates/aka-mcp`：`Backend` trait 是数据层接缝（mock 可测）；八工具 list_repos/query/context/find_definition/search_references/impact/analyze/augment。
- `crates/aka-server`：REST 面（repos 的导入/更新/设置/删除、query、symbol/context、graph/lod、graph/ego、node），CORS 仅 localhost。
- `apps/desktop`：三视图 Search/Graph/Symbol 全接真实数据（serve 离线时回退 demo/mock）；图渲染静态布局无力导，LOD 三档 + 标签 Canvas2D overlay + 网格拾取。

## 跑起来

```bash
cargo build -p aka-cli
./target/debug/aka analyze <repo>      # engine 解析 + 索引（首次先装 engine 依赖，见下）
./target/debug/aka serve &             # HTTP :4111（桌面端数据源）
cd apps/desktop && npm run dev         # UI :5188（HMR；自动连 serve）
./target/debug/aka mcp                 # MCP stdio（claude mcp add aka -- <绝对路径>/aka mcp）

# engine 首次初始化（同步副本装依赖）
cd engine/gitnexus-shared && npm i && npm run build && cd ../gitnexus && npm i

# 提交门槛
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
cd apps/desktop && npx tsc --noEmit && npm run build
```

## 任务流程（每个任务必走）

1. **需求/问题分析**：把"要做什么/现象/期望"讲清楚，不明确先和用户对齐。
2. **定位**：找到相关 crate/组件和数据流，确认改动点与影响面（别绕过工件合同和 Backend trait 接缝）。
3. **开发**：**多 agent 并行分模块**是本项目的默认工作方式（用户明确要求，嫌单线程慢）——能拆就拆给后台 agent 并行，按 crate/目录划界避免文件冲突，主线程做集成。
4. **拉取最新代码**：commit 前 `git fetch` 同步 `main`，别在过时基线上提交。
5. **提交**：过上面的提交门槛后按规范 commit：`<类型>: <描述>`（feat/fix/refactor/docs/infra），结尾 `Co-Authored-By` 署名。
6. **验证**：浏览器真实渲染/接口冒烟确认达标；UI 改动截图；没过回到第 3 步。

## 建议工作方式

- **设计基线（已定稿，别漂移）**：亮色 macOS 液态玻璃——毛玻璃面板（既有 `glass`/`glass-panel`/`badge` 类直接用）、分层柔和阴影、发光蓝 `#2E7CF6` 极克制（主按钮/聚焦/选中节点）、暖橙 `#F6A623` 只做灯塔点缀、framer-motion spring(stiffness≈300, damping≈30) 优雅不浮夸、无弹跳无旋转。
- **大图思路**：数据层按十亿级设计（磁盘索引、流式摄取），渲染层永远只画聚合/截断视图（LOD/ego），单视口控制在 GPU 舒适区。
- **选型不固定**：关键决策先和用户对齐，别拿旧结论绑死（Maya 记忆 `feedback-consult-selection`）。
- 共享记忆在 `~/agent-memory/maya/project_aka.md`（决策史与进度快照），会话开始值得扫一眼。

## 现状

（2026-06-10）M0–M3 完成：engine 移植（Ascend C 25/25）+ 工件合同 v0；Rust 五 crate 全绿（workspace 73 测试 + clippy -D warnings）；CLI 八命令；MCP/HTTP 双服务；桌面端三视图真实数据 + 仓库全生命周期（git/zip 导入、一键更新、per-repo 设置、删除）+ 节点详情/ego 下钻；WebGL 50 万节点/百万边 60fps 实测。
（2026-06-11）Process 执行流全链路接入：edges.step 摄取（旧库自动迁移）、布局 Processes 专属簇、MCP impact 报 affected_processes（哪条流断在第几步）、query/context 带流程归属、/api/node 对 Process 展开 entry/terminal/steps、桌面端流程详情视图（入口源码+步骤时间线）+ 普通符号"参与流程"。改完需对已有仓库重跑 `aka index` 补 step 数据。
**待办**：M4 Docker 化 + Jensen 部署 + 远程模式；embedding 实现（本地 fastembed，默认关）；增量索引（fileHashes/parse-cache）；Tauri 正式打包（engine Bun compile 成 sidecar）；wiki/group 按需移植；导入中 update 返回 404 的小语义瑕疵。
