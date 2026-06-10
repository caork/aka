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

- **M0** 地基：engine 分支（上游 v1.6.7 + Ascend C 移植）、emit 工件层、golden 对比测试 ← 当前
- **M1** Rust 索引核心：工件摄取 + tantivy + usearch，`aka analyze` / `aka search`
- **M2** MCP 八工具齐平，接入 Claude Code dogfood
- **M3** Tauri 桌面 MVP（仓库管理 / 搜索 / 符号 360° / 图谱 LOD）
- **M4** headless daemon 部署 Jensen + 远程模式

## 相关仓库

- engine 来源：[caork/GitNexus](https://github.com/caork/GitNexus) 的 `engine/aka` 分支（上游 v1.6.7 基底 + Ascend C 补丁），经 `scripts/sync-engine.sh` 同步。
- 上游：[abhigyanpatwari/GitNexus](https://github.com/abhigyanpatwari/GitNexus)
