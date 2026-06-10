# aka

感知所有代码——过去、现在与未来——的代码全知引擎。名字源自 Akasha records（阿卡西记录），CLI 即 `aka`。

GitNexus 的重写：继承其 tree-sitter 解析管线（TypeScript，作为 sidecar），存储 / 搜索 / 服务 / UI 全部新写（Rust + Tauri）。

## 架构

```
客户端          Tauri 桌面 app · AI agent (MCP) · 浏览器 (远程模式)
                          │
Rust core       aka-search (tantivy BM25 + usearch 向量 + RRF)
                aka-graph  (SQLite 持久 + 内存 CSR 邻接 + LOD 聚合)
                aka-mcp    (rmcp · stdio / Streamable HTTP)
                aka-server (axum)
                aka-core   (域模型 · 仓库注册 · 工件摄取 · 增量)
                          │  NDJSON 工件合同 (docs/contracts/artifacts.md)
解析引擎        engine/ — 继承自 GitNexus fork（tree-sitter 13 阶段管线 + Ascend C）
```

详细设计见 [docs/architecture.md](docs/architecture.md)。

## 已拍板的决策（2026-06-10）

- 非商用项目（上游 License 为 PolyForm Noncommercial 1.0，engine 为其衍生作品，整体遵守非商用约束）。
- 不保 Cypher：图查询走内存邻接 API，不引 LadybugDB/Kuzu FFI。
- 图谱可视化 Cosmograph + 分层 LOD：数据层按十亿级设计（磁盘索引、流式摄取），渲染层只画聚合视图（社区 → 文件 → 符号下钻），单视口控制在 GPU 舒适区。
- embedding 本地优先（fastembed ONNX）且默认关闭：默认搜索纯 BM25；设置中手动开启后才下载模型、回填向量、启用混合检索；Jensen 远程 embedding 为高级选项。

## 里程碑

- **M0** ✅ engine 分支（上游 v1.6.7 + Ascend C 移植 25/25）、emit 工件层（合同 v0）、demo-ts E2E
- **M1** ✅ Rust 索引核心：tantivy(代码感知 tokenizer) + usearch + RRF + SQLite/CSR 图存储；`aka analyze/search/context/lod`
- **M2** ✅ MCP 八工具（rmcp stdio）+ axum HTTP；`aka mcp` 可直接接 Claude Code
- **M3** ✅ 桌面 MVP：液态玻璃三视图全接真实数据；WebGL2 渲染器 500K 节点/1M 边 60fps，gitnexus 8.6K 节点实测
- **M4** ◐ `aka serve` headless 可用；Docker 化 + Jensen 部署 + 远程模式待做
- 待办：embedding 开关落地（本地 fastembed，默认关）、增量索引接 fileHashes、Tauri 打包(sidecar Bun compile)、wiki/group 按需移植

## 快速使用

```bash
cargo build -p aka-cli
./target/debug/aka analyze <repo>     # engine 解析 + 索引
./target/debug/aka search "query"     # BM25 检索
./target/debug/aka context <symbol>   # 符号 360°
./target/debug/aka mcp                # MCP stdio（接 Claude Code: claude mcp add aka -- <abs路径>/aka mcp）
./target/debug/aka serve              # HTTP :4111（桌面端数据源）
cd apps/desktop && npm run dev        # 桌面 UI（自动连 serve，离线回退 demo）
```

实测截图见 [docs/assets/](docs/assets/)。

## 相关仓库

- engine 来源：[caork/GitNexus](https://github.com/caork/GitNexus) 的 `engine/aka` 分支（上游 v1.6.7 基底 + Ascend C 补丁），经 `scripts/sync-engine.sh` 同步。
- 上游：[abhigyanpatwari/GitNexus](https://github.com/abhigyanpatwari/GitNexus)
