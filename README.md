# aka

感知所有代码——过去、现在与未来——的代码全知引擎。名字源自 Akasha records（阿卡西记录）。桌面端 AKA 是主要入口，也可在命令行下运行 MCP/索引/查询子命令。

解析层切换为 codebase-memory-mcp 原生 C 引擎：CBM 负责多语言 tree-sitter/LSP 解析并写入 SQLite，aka-core 通过 SQLite->NDJSON adapter 产出稳定工件；存储 / 搜索 / 服务 / UI 仍由 Rust + Tauri 承担。

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
解析引擎        engine/ — codebase-memory-mcp native C binary（SQLite -> NDJSON adapter）
```

详细设计见 [docs/architecture.md](docs/architecture.md)。

## 已拍板的决策（2026-06-10）

- License：codebase-memory-mcp 为 MIT，aka 以 MIT 口径分发；解析引擎通过 native C binary 进程边界接入。
- aka 服务面不暴露 CBM 的 Cypher 查询语义：图查询走 Rust 内存邻接 API，不引嵌入式图数据库 FFI。
- 图谱可视化 Cosmograph + 分层 LOD：数据层按十亿级设计（磁盘索引、流式摄取），渲染层只画聚合视图（社区 → 文件 → 符号下钻），单视口控制在 GPU 舒适区。
- embedding 本地优先（fastembed ONNX）且默认关闭：默认搜索纯 BM25；设置中手动开启后才下载模型、回填向量、启用混合检索；Jensen 远程 embedding 为高级选项。

## 里程碑

- **M0** ✅ codebase-memory-mcp native engine 接入、SQLite->NDJSON 工件层（合同 v0）、demo-ts E2E
- **M1** ✅ Rust 索引核心：tantivy(代码感知 tokenizer) + usearch + RRF + SQLite/CSR 图存储；`aka analyze/search/context/lod`
- **M2** ✅ MCP 工具面（rmcp stdio/HTTP，含 `search_code` 行级源码搜索、影响分析和仓库管理工具）+ axum HTTP；`aka mcp` 可直接接 Claude Code
- **M3** ✅ 桌面 MVP：液态玻璃三视图全接真实数据；WebGL2 渲染器 500K 节点/1M 边 60fps
- **M4** ◐ `aka serve` headless 可用；Docker 化 + Jensen 部署 + 远程模式待做
- 待办：embedding 开关落地（本地 fastembed，默认关）、增量索引接 fileHashes、Tauri 打包内置 CBM native binary、wiki/group 按需补齐

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

发行版使用：

- 下载 `aka-desktop-<ver>-<platform>` 即可获得桌面界面 + 本地 HTTP MCP + CLI/MCP 子命令。Windows 推荐运行 `aka-desktop-<ver>-x86_64-pc-windows-msvc-setup.exe`。macOS 开源包默认没有 Apple Developer ID 公证；如果系统提示 damaged，运行：

```bash
bash ~/Downloads/aka-desktop-<ver>-macos-open.sh \
  ~/Downloads/aka-desktop-<ver>-aarch64-apple-darwin.dmg
```

- 想要真实仓库数据：启动桌面 AKA 后直接导入/索引仓库；桌面端会同时启动本地服务。也可以在终端调用桌面可执行文件的子命令，例如 `AKA analyze <repo>`、`AKA mcp`、`AKA serve`。

实测截图见 [docs/assets/](docs/assets/)。

## Docker 部署

```bash
docker build -t aka:0.1.0 .   # 多阶段：rust release + CBM native engine build + slim runtime
docker compose up -d          # http://127.0.0.1:4111（数据卷 aka-data → /data）
```

构建 / 运行 / 导入仓库 / 数据卷 / 远程访问注意，见 [docs/deploy.md](docs/deploy.md)。
镜像内置 `codebase-memory-mcp` 原生二进制，OCI license label 使用 MIT。

## 客户端接入

aka 可作为 MCP server 接入主流编码 agent 客户端，配置与安装脚本在 [clients/](clients/)：

```bash
clients/install.sh --client claude-code   # Claude Code（--plugin 走插件方式，含 skill）
clients/install.sh --client codex         # OpenAI Codex CLI（~/.codex/config.toml）
clients/install.sh --client opencode      # OpenCode（opencode.json）
```

Claude Code 也可直接装插件（捆绑 MCP server + 使用策略 skill）：`claude plugin marketplace add <本仓库>` → `claude plugin install aka@aka`。设计文档（能力矩阵、远程模式演进）见 [docs/clients.md](docs/clients.md)。

Agent 不必先通过桌面端手动导入仓库：HTTP MCP 每次工具调用都会尝试读取客户端 workspace roots 并自动排队索引；stdio fallback 也会自动发现当前工作区。本地仓库没出现在 `list_repos` 时，让 agent 调 `analyze` 并传仓库根目录或任意子目录即可；远程 GitHub/Git 仓库用 `import_repo kind=git`。

## Release 产物

推送 `v*` tag 会生成：

- `aka-desktop-<ver>-aarch64-apple-darwin.dmg` — macOS GUI 安装镜像
- `aka-desktop-<ver>-aarch64-apple-darwin.app.zip` — macOS GUI（zip 内是 `aka.app`）
- `aka-desktop-<ver>-macos-open.sh` — macOS 无公证包打开助手（安装到 `~/Applications` 并移除 quarantine）
- `aka-desktop-<ver>-x86_64-pc-windows-msvc-setup.exe` — Windows GUI 安装包
- `aka-desktop-<ver>-x86_64-pc-windows-msvc-portable.zip` — Windows GUI 免安装包
- `aka-claude-code-plugin-<ver>.zip` — Claude Code 插件包
- `aka-opencode-plugin-<ver>.zip` — OpenCode 本地 plugin + MCP/skill 配置包
- `aka-clients-<ver>.tar.gz` — 全量客户端接入文件
- `aka-<ver>-linux-amd64.docker.tar.gz` — Docker 镜像离线包
- `SHA256SUMS`
- `latest.json` — 桌面端更新清单

不再发布 `aka-<ver>-<platform>` 裸 CLI/server 压缩包，避免用户在桌面包和 CLI 包之间做选择。需要命令行能力时，直接调用桌面包里的 `AKA` 可执行文件；需要 headless 部署时使用 Docker 镜像。

## 相关仓库

- engine：基于上游 [DeusData/codebase-memory-mcp](https://github.com/DeusData/codebase-memory-mcp) 维护自有 fork `caork/codebase-memory-mcp`；`scripts/sync-engine.sh` 默认构建本地维护 checkout，月度/显式同步时再从上游选择性合入。
