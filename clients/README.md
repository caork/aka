# aka 客户端接入

把 aka 接进编码 agent 客户端：本地 stdio MCP（`aka mcp`，九工具）是统一接入面，三个客户端只是配置格式不同。设计文档（架构、能力矩阵、远程模式演进）见 [docs/clients.md](../docs/clients.md)。

## 一键安装

```bash
cargo build --release -p aka-cli                 # 先有二进制
clients/install.sh --client claude-code          # 直连 MCP（最简）
clients/install.sh --client claude-code --plugin # 插件方式（含 skill）
clients/install.sh --client codex
clients/install.sh --client opencode             # 需要 jq
# 任意客户端都可加 --dry-run 预览、--bin /path/to/aka 指定二进制
```

脚本幂等：已配置过会提示并跳过。

## 三客户端速览（2026-06 核实的格式）

| 客户端 | 配置位置 | 格式要点 | 验证 |
|---|---|---|---|
| **Claude Code** | 插件（`.claude-plugin/plugin.json` + `.mcp.json`）或 `claude mcp add` | 插件可捆绑 MCP server + skills；marketplace 经仓库根 `.claude-plugin/marketplace.json` 分发 | `claude mcp list` / `claude plugin list` |
| **Codex CLI** | `~/.codex/config.toml` | `[mcp_servers.aka]` 表，`command` + `args` 字段 | `codex mcp list`，TUI 里 `/mcp` |
| **OpenCode** | `~/.config/opencode/opencode.json` 或项目根 `opencode.json`；插件在 `~/.config/opencode/plugins/` 或 `.opencode/plugins/` | `mcp.aka` 对象，`type: "local"`，**`command` 是数组**；本地 plugin 是 JS/TS module | TUI 会话里触发 `aka_list_repos` |

各目录详情：

- [claude-code/](./claude-code/) — 完整插件（MCP server + `aka-code-graph` skill），可 `claude plugin marketplace add` 本仓库后 `claude plugin install aka@aka`
- [codex/](./codex/) — TOML 片段 + `codex mcp add` 用法
- [opencode/](./opencode/) — JSON 片段 + OpenCode 本地 plugin + 使用策略（原生 skill 推荐，AGENTS-aka.md 备选），发布包 `aka-opencode-plugin-<ver>.zip` 即此目录

## 通用前置

1. 构建二进制：`cargo build --release -p aka-cli`（脚本会自动探测 `target/{release,debug}/aka`，也认 PATH）。
2. 至少索引一个仓库：`aka analyze /path/to/repo`（否则 agent 调 `list_repos` 得到空列表）。
3. MCP server 是 stdio 子进程，由客户端按需拉起，无需常驻；`aka serve`（HTTP :4111）只服务桌面端，与客户端接入无关（远程模式落地后才复用，见设计文档）。

> License 提醒：解析引擎 `codebase-memory-mcp` 为 MIT，aka 客户端接入按 MIT 口径分发。
