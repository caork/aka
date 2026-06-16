# 客户端接入设计（Claude Code / Codex / OpenCode）

> 状态：v0（2026-06）。落地物在 [clients/](../clients/)；本文记录设计依据、能力矩阵与演进路线。

## 1. 设计目标与原则

- **一个协议，N 种包装**：所有客户端统一走 MCP 协议。默认连接 AKA 桌面端内置的本地 Streamable HTTP MCP（`http://127.0.0.1:4112/mcp`），让 AI indexing/query 和 GUI 图谱/code 预览天然复用同一份知识库。stdio `AKA mcp` 只作为桌面不常开或客户端不支持 HTTP MCP 时的 fallback，不绕过 `Backend` trait 接缝。
- **一个本地知识库**：桌面 GUI、桌面 HTTP MCP、stdio fallback / headless 调试入口都指向同一份 app data `AKA_HOME`。不再让 GUI 和 AI 插件各自生成孤岛索引。
- **路径不写死**：仓库内默认配置只写 localhost MCP URL；stdio fallback 才需要 `--bin` 探测（`command -v aka/AKA` → `target/release` → `target/debug`）。

## 2. 架构：本地 stdio 与远程 HTTP 的演进

```
现在（M3/M4 本机，已落地）                 之后（M4 远程模式，待做）
─────────────────────────                ─────────────────────────────
agent 客户端                              agent 客户端
  │ Streamable HTTP                         │ Streamable HTTP (+token)
  ▼                                        ▼
AKA desktop ─▶ http://127.0.0.1:4112/mcp   aka serve :4111 (Docker @ Jensen)
  │                                           ├ /mcp  Streamable HTTP 端点(远程模式)
  ▼                                           └ REST  (桌面端复用现有路由)
app data aka-home
  (tantivy + SQLite/CSR)

stdio fallback: agent ─spawn─▶ 桌面包 AKA mcp ──读写同一 app data aka-home

远程模式: agent ─HTTP+token─▶ aka serve :4111 ─▶ /data（服务器侧索引）
```

- **阶段一（现状）**：本机运行。桌面端负责索引/浏览，并内置 `127.0.0.1:4112/mcp` 给 Claude Code / Codex / OpenCode。`list_repos` 会尝试通过 MCP `roots/list` 获取客户端 workspace roots 并自动排队索引；客户端不暴露 roots 时，skill 指导 agent 用 `analyze` 显式传仓库绝对路径。stdio `AKA mcp` 保留为 fallback，且和 GUI 共用 app data。
- **阶段二（M4 远程模式）**：`aka serve` 增加 MCP Streamable HTTP 端点（rmcp 已支持该 transport，README 架构图中已预留）。三个客户端 2026 年中均已支持远程 MCP：
  - Claude Code：`claude mcp add --transport http aka https://host/mcp`；
  - Codex：`[mcp_servers.aka]` 里 `url = "..."` + `bearer_token_env_var`；
  - OpenCode：`"type": "remote"` + `url` + `headers`（自动处理 OAuth）。

  届时 `clients/` 各 README 增补远程片段即可，**客户端侧不需要新形态的交付物**——这是选 MCP 作唯一接缝的回报。
- **augment 工具**是为阶段三（编辑器钩子自动注入上下文）预留的低成本端点，hooks 方案成熟后接 Claude Code 的 `PreToolUse`/prompt hook。

## 3. 三客户端能力矩阵（2026-06 核实）

| 能力 | Claude Code | Codex CLI | OpenCode |
|---|---|---|---|
| 本地 stdio MCP | ✅ fallback：`claude mcp add --transport stdio` | ✅ fallback：`command`/`args` | 可用，但默认不推荐 |
| 桌面端本地 HTTP MCP | ✅ 默认：`--transport http` / 插件 `.mcp.json` | ✅ 默认：`url = "http://127.0.0.1:4112/mcp"` | ✅ 默认 `type:"remote"` + `http://127.0.0.1:4112/mcp` |
| 远程 HTTP MCP | ✅ `--transport http` | ✅ `url` + `bearer_token_env_var` | ✅ type:"remote" + OAuth |
| 插件/捆绑分发 | ✅ 插件体系（marketplace + plugin.json，可捆绑 MCP+skills+hooks+agents） | ❌ 无插件体系，只有 config.toml | ✅ 本地/ npm plugin（JS/TS）；MCP 仍写 opencode.json |
| 用法指导（何时用哪个工具） | ✅ skill（`skills/aka-code-graph/`） | ⚠️ 仅靠工具 description / AGENTS.md | ✅ 原生 skill；AGENTS/instructions 备选 |
| 配置 CLI | `claude mcp add`、`claude plugin install` | `codex mcp add` | 无（手编 JSON / install.sh 用 jq 合并） |
| 工具级开关 | 权限系统 | `enabled_tools` / `disabled_tools` / 审批模式 | `enabled: false` 整 server 粒度 |
| 配置作用域 | user / project / local | `~/.codex/` + 受信任项目 `.codex/` | 全局 + 项目根（向上找 git 根） |

落差的补偿：Codex 没有 skill/plugin 机制，aka 的工具 description 已写成自带使用策略（"Call this first"、"Prefer this over separate lookups"、"Use 'impact' for the transitive blast radius"），让无 skill 客户端也能选对工具；需要更强引导时，用户可把 `clients/claude-code/skills/aka-code-graph/SKILL.md` 的正文贴进 AGENTS.md / 项目规则。

## 4. 各客户端格式要点（含来源）

### Claude Code 插件（[clients/claude-code/](../clients/claude-code/)）

- 清单 `.claude-plugin/plugin.json`，**`name` 是唯一必填字段**；`mcpServers` 字段指向 `.mcp.json`（或内联）。默认 `.mcp.json` 使用 HTTP MCP：`{ "aka": { "type": "http", "url": "http://127.0.0.1:4112/mcp" } }`，要求 AKA 桌面端运行；stdio fallback 另走 `claude mcp add --transport stdio aka -- /path/to/AKA mcp`。
- skills 放 `skills/<name>/SKILL.md`（YAML frontmatter：name + description）。
- 分发：仓库根 `.claude-plugin/marketplace.json` 声明 `plugins[].source: "./clients/claude-code"`，用户 `claude plugin marketplace add <repo>` → `claude plugin install aka@aka`。安装时插件目录被**拷贝**进缓存，不能引用目录外文件；本地调试用 `claude --plugin-dir`。
- 校验：`claude plugin validate <path>`（`--strict` 把未知字段警告升级为错误）。
- 来源：[plugins-reference](https://code.claude.com/docs/en/plugins-reference)、[plugin-marketplaces](https://code.claude.com/docs/en/plugin-marketplaces)、[anthropics/claude-code 官方 marketplace.json](https://github.com/anthropics/claude-code/blob/main/.claude-plugin/marketplace.json)。

### Codex CLI（[clients/codex/](../clients/codex/)）

- `~/.codex/config.toml`，默认用 `[mcp_servers.aka] url = "http://127.0.0.1:4112/mcp"`；stdio fallback 用 `command`/`args`/`env`。远程服务可用 `url` + `bearer_token_env_var`。可调 `startup_timeout_sec`/`tool_timeout_sec`/审批模式。`codex mcp add` 可代写。
- 来源：[developers.openai.com/codex/mcp](https://developers.openai.com/codex/mcp)、[config-reference](https://developers.openai.com/codex/config-reference)。

### OpenCode（[clients/opencode/](../clients/opencode/)）

- `opencode.json`（全局 `~/.config/opencode/` 或项目根）`mcp` 键；默认 `type:"remote"` + `url:"http://127.0.0.1:4112/mcp"`，连接正在运行的 AKA 桌面端。
- 本地 plugin 放 `~/.config/opencode/plugins/` 或 `.opencode/plugins/`，是导出 plugin 函数的 JS/TS module。aka 的 `plugins/aka.js` 只做集成加载自检；真正工具面由 `mcp.aka` 连接桌面端 MCP endpoint。
- 使用策略优先走 `skills/aka-code-graph/SKILL.md`；旧版/常驻场景用 `AGENTS-aka.md` 或 `instructions` 数组。
- 来源：[opencode.ai/docs/mcp-servers](https://opencode.ai/docs/mcp-servers/)、[opencode.ai/docs/config](https://opencode.ai/docs/config/)、[opencode.ai/docs/plugins](https://opencode.ai/docs/plugins/)、[opencode.ai/docs/skills](https://opencode.ai/docs/skills/)。

## 5. 版本兼容策略

- **MCP 工具面即合同**：工具的名称、参数、输出字段视同 `docs/contracts/artifacts.md` 的同级合同——**只增不改不删**。新能力 = 新工具或新可选参数；废弃工具先在 description 标注 deprecated 一个版本周期再移除。三个客户端都直接消费工具 schema，没有中间适配层可以吸收破坏性变更。
- **插件版本**：`plugin.json` 的 `version` 跟随 aka 二进制的 minor 版本手动 bump（不 bump 用户就不会收到更新）；插件只含 HTTP MCP 配置和 markdown，与二进制弱耦合，硬依赖是「AKA 桌面端提供 `127.0.0.1:4112/mcp`」。
- **客户端版本下限**：Claude Code 用到的特性（plugin.json + .mcp.json + skills + marketplace）为 2025 年底已稳定的核心集，刻意不用新版才有的字段（`displayName` 需 ≥2.1.143、`defaultEnabled` 需 ≥2.1.154、`userConfig` 等），保证老版本可装。Codex/OpenCode 片段同样只用各自文档标注的稳定字段。
- **格式漂移的防线**：三家配置格式仍在演进，`clients/` 各 README 标注「2026-06 核实」与来源 URL；install.sh 优先走官方 CLI（`claude mcp add`、`codex mcp add`），格式变更由官方 CLI 吸收，手写文件仅作 fallback（OpenCode MCP 配置用 jq 合并而非整文件覆盖，本地 plugin 用发现目录安装）。

## 6. 已知限制 / 待办

- 默认 HTTP MCP 要求 AKA 桌面端已启动。桌面不常开时，Claude Code / Codex 用 `--stdio --bin /path/to/AKA` 配置 fallback；该 fallback 复用 GUI 的 app data `AKA_HOME`，不是独立产品形态。
- OpenCode 的「装好验证」主要依赖 TUI 交互，没有 `claude mcp list` 这样的一行命令式探针；plugin 会写一条加载日志，但 MCP 是否可用仍以会话里触发 `aka_list_repos` 为准。默认配置要求 AKA 桌面端已启动。
- 远程模式（M4）落地时：aka-server 挂 rmcp Streamable HTTP、加 token 认证、Docker 镜像；届时在 clients/ 各 README 补远程配置片段。
