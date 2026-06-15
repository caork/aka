# aka 客户端接入

把 aka 接进编码 agent 客户端：默认都连接 AKA 桌面端内置的本地 Streamable HTTP MCP（`http://127.0.0.1:4112/mcp`），让 AI 索引/查询和 GUI 图谱预览复用同一份知识库。Claude Code / Codex 仍可用 `AKA mcp` 作为 stdio fallback。设计文档见 [docs/clients.md](../docs/clients.md)。

## 一键安装

```bash
clients/install.sh --client opencode             # 默认连接已运行的 AKA 桌面端，需要 jq
clients/install.sh --client claude-code          # 默认连接已运行的 AKA 桌面端
clients/install.sh --client codex                # 默认连接已运行的 AKA 桌面端
clients/install.sh --client claude-code --plugin # 插件方式（HTTP MCP + skill）
clients/install.sh --client codex --stdio --bin /path/to/AKA  # 桌面不常开时的 fallback
# 任意客户端都可加 --dry-run 预览
```

脚本幂等：已配置过会提示并跳过。

## 三客户端速览（2026-06 核实的格式）

| 客户端 | 配置位置 | 格式要点 | 验证 |
|---|---|---|---|
| **Claude Code** | 插件（`.claude-plugin/plugin.json` + `.mcp.json`）或 `claude mcp add` | 默认 HTTP MCP 指向 `http://127.0.0.1:4112/mcp`；stdio fallback 用 `AKA mcp` | `claude mcp list` / `claude plugin list` |
| **Codex CLI** | `~/.codex/config.toml` | 默认 `[mcp_servers.aka] url = "http://127.0.0.1:4112/mcp"`；stdio fallback 用 `command` + `args` | `codex mcp list`，TUI 里 `/mcp` |
| **OpenCode** | `~/.config/opencode/opencode.json` 或项目根 `opencode.json`；插件在 `~/.config/opencode/plugins/` 或 `.opencode/plugins/` | `mcp.aka` 对象，`type: "remote"`，`url: "http://127.0.0.1:4112/mcp"`；本地 plugin 是 JS/TS module | 启动 AKA 桌面端后，在 TUI 会话里触发 `aka_list_repos` |

各目录详情：

- [claude-code/](./claude-code/) — 完整插件（MCP server + `aka-code-graph` skill），可 `claude plugin marketplace add` 本仓库后 `claude plugin install aka@aka`
- [codex/](./codex/) — TOML 片段 + `codex mcp add` 用法
- [opencode/](./opencode/) — JSON 片段 + OpenCode 本地 plugin + 使用策略（原生 skill 推荐，AGENTS-aka.md 备选），发布包 `aka-opencode-plugin-<ver>.zip` 即此目录

## 通用前置

1. 默认复用已经运行的 AKA 桌面端本地 MCP server：`http://127.0.0.1:4112/mcp`。
2. agent 会先调 `list_repos`：HTTP MCP 会尝试读取客户端 workspace roots 并自动排队索引；`status: "indexing"` 表示自动索引正在跑，稍后重试。
3. 如果客户端未暴露 roots 或要索引非当前工作区，显式调用 `analyze` 并传仓库绝对路径。
4. 桌面不常开或客户端不支持 HTTP MCP 时，Claude Code / Codex 可加 `--stdio --bin /path/to/AKA` 使用 `AKA mcp` fallback；fallback 与 GUI 共用同一份桌面数据。

源码开发时仍可 `cargo build --release -p aka-cli`，脚本也会自动探测 `target/{release,debug}/aka` 和 PATH 上的 `aka`，方便本地调试。

> License 提醒：解析引擎 `codebase-memory-mcp` 为 MIT，aka 客户端接入按 MIT 口径分发。
