# 客户端接入设计（Claude Code / Codex / OpenCode）

> 状态：v0（2026-06）。落地物在 [clients/](../clients/)；本文记录设计依据、能力矩阵与演进路线。

## 1. 设计目标与原则

- **一个接入面，N 种包装**：所有客户端统一走 MCP 协议接 `aka mcp`（rmcp stdio，八工具）。客户端差异只体现在「配置格式 + 分发方式」这一薄层，不为任何客户端做专属代码（不动 crates/、不绕过 `Backend` trait 接缝）。
- **零常驻**：stdio server 由客户端按需 spawn，进程随会话生灭；数据（`~/.aka/`）由 CLI 的 `analyze` 预先建好，MCP 进程只读。
- **路径不写死**：仓库内任何配置不含机器特定路径；`clients/install.sh` 安装时探测（`command -v aka` → `target/release` → `target/debug`），插件场景靠 PATH 解析。

## 2. 架构：本地 stdio 与远程 HTTP 的演进

```
现在（M2–M3，已落地）                      之后（M4 远程模式，待做）
─────────────────────────                ─────────────────────────────
agent 客户端                              agent 客户端
  │ spawn 子进程 (stdio)                    │ Streamable HTTP (+token)
  ▼                                        ▼
aka mcp ──读──▶ ~/.aka/                   aka serve :4111 (Docker @ Jensen)
  (tantivy + SQLite/CSR)                    ├ /mcp  Streamable HTTP 端点(新增)
                                            └ REST  (桌面端复用现有路由)
                                              │
                                              ~/.aka/（服务器侧索引）
```

- **阶段一（现状）**：本地 stdio。索引和查询都在用户机器上，`aka analyze` 建索引 → 客户端 spawn `aka mcp` 查询。优点：零网络、零认证、数据不出机。
- **阶段二（M4 远程模式）**：`aka serve` 增加 MCP Streamable HTTP 端点（rmcp 已支持该 transport，README 架构图中已预留）。三个客户端 2026 年中均已支持远程 MCP：
  - Claude Code：`claude mcp add --transport http aka https://host/mcp`；
  - Codex：`[mcp_servers.aka]` 里 `url = "..."` + `bearer_token_env_var`；
  - OpenCode：`"type": "remote"` + `url` + `headers`（自动处理 OAuth）。

  届时 `clients/` 各 README 增补远程片段即可，**客户端侧不需要新形态的交付物**——这是选 MCP 作唯一接缝的回报。
- **augment 工具**是为阶段三（编辑器钩子自动注入上下文）预留的低成本端点，hooks 方案成熟后接 Claude Code 的 `PreToolUse`/prompt hook。

## 3. 三客户端能力矩阵（2026-06 核实）

| 能力 | Claude Code | Codex CLI | OpenCode |
|---|---|---|---|
| 本地 stdio MCP | ✅ `claude mcp add` / 插件 `.mcp.json` | ✅ `[mcp_servers.x]` command/args | ✅ `mcp.x` type:"local"，command 为数组 |
| 远程 HTTP MCP | ✅ `--transport http` | ✅ `url` + `bearer_token_env_var` | ✅ type:"remote" + OAuth |
| 插件/捆绑分发 | ✅ 插件体系（marketplace + plugin.json，可捆绑 MCP+skills+hooks+agents） | ❌ 无插件体系，只有 config.toml | ✅ 本地/ npm plugin（JS/TS）；MCP 仍写 opencode.json |
| 用法指导（何时用哪个工具） | ✅ skill（`skills/aka-code-graph/`） | ⚠️ 仅靠工具 description / AGENTS.md | ✅ 原生 skill；AGENTS/instructions 备选 |
| 配置 CLI | `claude mcp add`、`claude plugin install` | `codex mcp add` | 无（手编 JSON / install.sh 用 jq 合并） |
| 工具级开关 | 权限系统 | `enabled_tools` / `disabled_tools` / 审批模式 | `enabled: false` 整 server 粒度 |
| 配置作用域 | user / project / local | `~/.codex/` + 受信任项目 `.codex/` | 全局 + 项目根（向上找 git 根） |

落差的补偿：Codex 没有 skill/plugin 机制，aka 的工具 description 已写成自带使用策略（"Call this first"、"Prefer this over separate lookups"、"Use 'impact' for the transitive blast radius"），让无 skill 客户端也能选对工具；需要更强引导时，用户可把 `clients/claude-code/skills/aka-code-graph/SKILL.md` 的正文贴进 AGENTS.md / 项目规则。

## 4. 各客户端格式要点（含来源）

### Claude Code 插件（[clients/claude-code/](../clients/claude-code/)）

- 清单 `.claude-plugin/plugin.json`，**`name` 是唯一必填字段**；`mcpServers` 字段指向 `.mcp.json`（或内联）。MCP 配置是标准 `mcpServers` 格式，支持 `${CLAUDE_PLUGIN_ROOT}`（插件安装目录）等变量——但 aka 二进制不随插件分发，所以 command 用 PATH 名 `"aka"`，安装说明见该目录 README。
- skills 放 `skills/<name>/SKILL.md`（YAML frontmatter：name + description）。
- 分发：仓库根 `.claude-plugin/marketplace.json` 声明 `plugins[].source: "./clients/claude-code"`，用户 `claude plugin marketplace add <repo>` → `claude plugin install aka@aka`。安装时插件目录被**拷贝**进缓存，不能引用目录外文件；本地调试用 `claude --plugin-dir`。
- 校验：`claude plugin validate <path>`（`--strict` 把未知字段警告升级为错误）。
- 来源：[plugins-reference](https://code.claude.com/docs/en/plugins-reference)、[plugin-marketplaces](https://code.claude.com/docs/en/plugin-marketplaces)、[anthropics/claude-code 官方 marketplace.json](https://github.com/anthropics/claude-code/blob/main/.claude-plugin/marketplace.json)。

### Codex CLI（[clients/codex/](../clients/codex/)）

- `~/.codex/config.toml`，stdio 用 `[mcp_servers.aka]` + `command`/`args`/`env`；远程用 `url` + `bearer_token_env_var`。可调 `startup_timeout_sec`/`tool_timeout_sec`/审批模式。`codex mcp add` 可代写。
- 来源：[developers.openai.com/codex/mcp](https://developers.openai.com/codex/mcp)、[config-reference](https://developers.openai.com/codex/config-reference)。

### OpenCode（[clients/opencode/](../clients/opencode/)）

- `opencode.json`（全局 `~/.config/opencode/` 或项目根）`mcp` 键；本地 `type:"local"` 且 **`command` 是数组**，`environment` 传 env，`enabled` 开关；远程 `type:"remote"` + `url`。
- 本地 plugin 放 `~/.config/opencode/plugins/` 或 `.opencode/plugins/`，是导出 plugin 函数的 JS/TS module。aka 的 `plugins/aka.js` 只做集成加载自检；真正工具面仍由 `mcp.aka` 启动 `aka mcp`。
- 使用策略优先走 `skills/aka-code-graph/SKILL.md`；旧版/常驻场景用 `AGENTS-aka.md` 或 `instructions` 数组。
- 来源：[opencode.ai/docs/mcp-servers](https://opencode.ai/docs/mcp-servers/)、[opencode.ai/docs/config](https://opencode.ai/docs/config/)、[opencode.ai/docs/plugins](https://opencode.ai/docs/plugins/)、[opencode.ai/docs/skills](https://opencode.ai/docs/skills/)。

## 5. 版本兼容策略

- **MCP 工具面即合同**：八工具的名称、参数、输出字段视同 `docs/contracts/artifacts.md` 的同级合同——**只增不改不删**。新能力 = 新工具或新可选参数；废弃工具先在 description 标注 deprecated 一个版本周期再移除。三个客户端都直接消费工具 schema，没有中间适配层可以吸收破坏性变更。
- **插件版本**：`plugin.json` 的 `version` 跟随 aka 二进制的 minor 版本手动 bump（不 bump 用户就不会收到更新）；插件只含配置和 markdown，与二进制弱耦合，唯一硬依赖是「二进制支持 `aka mcp` 子命令」（M2 起恒真）。
- **客户端版本下限**：Claude Code 用到的特性（plugin.json + .mcp.json + skills + marketplace）为 2025 年底已稳定的核心集，刻意不用新版才有的字段（`displayName` 需 ≥2.1.143、`defaultEnabled` 需 ≥2.1.154、`userConfig` 等），保证老版本可装。Codex/OpenCode 片段同样只用各自文档标注的稳定字段。
- **格式漂移的防线**：三家配置格式仍在演进，`clients/` 各 README 标注「2026-06 核实」与来源 URL；install.sh 优先走官方 CLI（`claude mcp add`、`codex mcp add`），格式变更由官方 CLI 吸收，手写文件仅作 fallback（OpenCode MCP 配置用 jq 合并而非整文件覆盖，本地 plugin 用发现目录安装）。

## 6. 已知限制 / 待办

- 插件方式要求 aka 在 PATH（插件清单无法在安装时探测路径）；install.sh 已给出 symlink/改 `.mcp.json` 两条出路。`userConfig`（enable 时弹窗让用户填二进制路径，`${user_config.aka_bin}` 替换进 command）是更优解，等用户群 Claude Code 版本普遍支持后再启用。
- OpenCode 的「装好验证」主要依赖 TUI 交互，没有 `claude mcp list` 这样的一行命令式探针；plugin 会写一条加载日志，但 MCP 是否可用仍以会话里触发 `aka_list_repos` 为准。
- 远程模式（M4）落地时：aka-server 挂 rmcp Streamable HTTP、加 token 认证、Docker 镜像；届时在 clients/ 各 README 补远程配置片段。
