# aka × OpenAI Codex CLI

Codex 的 MCP 配置在 `~/.codex/config.toml`（CLI 与 IDE 扩展共用；受信任项目也可用项目级 `.codex/config.toml`），stdio server 用 `[mcp_servers.<name>]` 表声明。

## 配置

把 [`config.toml.snippet`](./config.toml.snippet) 追加到 `~/.codex/config.toml`：

```toml
[mcp_servers.aka]
command = "/absolute/path/to/AKA"
args = ["mcp"]
```

或者用 Codex 自带的命令行（免手编 TOML）：

```bash
codex mcp add aka -- /absolute/path/to/AKA mcp
```

也可直接跑仓库脚本：`clients/install.sh --client codex --bin /absolute/path/to/AKA`（自动探测常见路径、幂等写入）。

## 注意

- **路径**：Codex 启动 stdio server 时继承的 PATH 可能与你的交互 shell 不同；`command` 用桌面 AKA 可执行文件的绝对路径最稳。
- **超时**：`analyze` 大仓库耗时较长，必要时在该表里加 `tool_timeout_sec = 120`。
- **环境变量**：aka 默认读 `~/.aka/` 注册表，无需额外 env；如需自定义可加 `[mcp_servers.aka.env]` 表。
- **自动索引**：`AKA mcp` 启动和工具调用时都会自动发现当前 Codex 工作区；如果还没有索引，会在后台排队分析。会话里先调用 `list_repos`，看到 `status: "indexing"` 时稍后重试查询即可。

## 验证

```bash
codex mcp list          # 应列出 aka
codex                   # 进入 TUI 后输入 /mcp 查看激活的 server 与工具
# 会话里问："用 aka 列出已索引的仓库" → 应调用 list_repos
```

格式参考（2026-06 核实）：
- https://developers.openai.com/codex/mcp
- https://developers.openai.com/codex/config-reference
