# aka × OpenCode

OpenCode 的 MCP 配置写在 `opencode.json` 的 `mcp` 键下：全局 `~/.config/opencode/opencode.json`，或项目根的 `opencode.json`（可入 git，schema 相同；OpenCode 从当前目录向上找到最近的 git 根）。

## 配置

把 [`opencode.json.snippet`](./opencode.json.snippet) 合并进你的 `opencode.json`：

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "aka": {
      "type": "local",
      "command": ["aka", "mcp"],
      "enabled": true
    }
  }
}
```

要点（与 Claude Code / Codex 的差异）：

- `type` 必填，本地 stdio server 是 `"local"`（远程 HTTP 是 `"remote"` + `url`）。
- **`command` 是数组**（`["aka", "mcp"]`），不是 Codex 那种 `command` + `args` 两个字段。
- aka 不在 PATH 时把数组第一项换成绝对路径：`["/absolute/path/to/aka", "mcp"]`。
- 临时停用设 `"enabled": false`，不必删配置。
- 如需环境变量，加 `"environment": {"KEY": "value"}`。

也可直接跑仓库脚本：`clients/install.sh --client opencode`（自动探测二进制路径、幂等合并，需要 `jq`）。

## 验证

```bash
opencode                 # 启动 TUI
# 会话里问："用 aka 列出已索引的仓库" → 应调用 aka_list_repos 工具
# 或在 TUI 里查看 MCP/工具列表（不同版本入口略有差异）
```

启动时如果 server 起不来，OpenCode 会在日志/状态里报该 MCP 初始化失败——通常是 command 路径不对。

格式参考（2026-06 核实）：
- https://opencode.ai/docs/mcp-servers/
- https://opencode.ai/docs/config/
