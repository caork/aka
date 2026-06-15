# aka × OpenAI Codex CLI

Codex 的 MCP 配置在 `~/.codex/config.toml`（CLI 与 IDE 扩展共用；受信任项目也可用项目级 `.codex/config.toml`）。默认连接 AKA 桌面端内置的本地 Streamable HTTP MCP，让 AI 索引/查询和 GUI 图谱复用同一份知识库。

Codex 没有内置 skill/plugin 机制。想让 Codex 更稳定地选择 aka 工具，可把 [`AGENTS-aka.md`](./AGENTS-aka.md) 的内容追加到项目 `AGENTS.md` 或全局 Codex 指令里；它强调 `list_repos`、workspace roots 自动索引、`analyze` 兜底和各工具选择规则。

## 配置

把 [`config.toml.snippet`](./config.toml.snippet) 追加到 `~/.codex/config.toml`：

```toml
[mcp_servers.aka]
url = "http://127.0.0.1:4112/mcp"
```

或者用 Codex 自带的命令行（免手编 TOML）：

```bash
codex mcp add aka --url http://127.0.0.1:4112/mcp
```

也可直接跑仓库脚本：`clients/install.sh --client codex`（幂等写入）。

桌面不常开或当前 Codex 版本不支持 HTTP MCP 时，可改用 stdio fallback：

```bash
codex mcp add aka -- /absolute/path/to/AKA mcp
clients/install.sh --client codex --stdio --bin /absolute/path/to/AKA
```

## 注意

- **桌面端**：HTTP MCP 需要 AKA 桌面端正在运行；端口固定为本机 `127.0.0.1:4112`，不暴露到局域网。
- **stdio fallback 路径**：Codex 启动 stdio server 时继承的 PATH 可能与你的交互 shell 不同；`command` 用桌面 AKA 可执行文件的绝对路径最稳。
- **超时**：`analyze` 大仓库耗时较长，必要时在该表里加 `tool_timeout_sec = 120`。
- **环境变量**：默认不需要额外 env。stdio fallback 使用桌面包里的 `AKA` 时也会和 GUI 共用同一份桌面数据。
- **自动索引**：会话里先调用 `list_repos`。HTTP MCP 会尝试读取 Codex workspace roots 并自动排队索引；如果客户端未暴露 roots 或本地目标仓库不在列表里，调用 `analyze` 并传仓库绝对路径。远程 GitHub/Git 仓库用 `import_repo`，传 `kind:"git"` 和 clone URL；已索引仓库要刷新用 `update_repo`。返回的 `repo` 是后续查询要带的仓库名。看到 `status: "indexing"` 时稍后重试 `list_repos` / 查询即可。
- **使用策略**：建议安装/引用 [`AGENTS-aka.md`](./AGENTS-aka.md)。不要让 agent 先逐文件 grep；让它先 `list_repos`，必要时自动/显式索引，再按任务选择 `query`、`search_code`、`context`、`impact`、`route_map` 等工具。

## 验证

```bash
codex mcp list          # 应列出 aka
codex                   # 进入 TUI 后输入 /mcp 查看激活的 server 与工具
# 会话里问："用 aka 列出已索引的仓库" → 应调用 list_repos
```

格式参考（2026-06 核实）：
- https://developers.openai.com/codex/mcp
- https://developers.openai.com/codex/config-reference
