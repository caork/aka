# aka × OpenCode

接入分两步：**① 启动 AKA 桌面端**（内置本地 MCP server）+ **② 安装 OpenCode 配置/插件/使用策略**。本目录就是发布包 `aka-opencode-plugin-<ver>.zip` 的内容，解压即用。

```
opencode.json.snippet              ② MCP 配置片段（连接 AKA 桌面端）
plugins/aka.js                     ② OpenCode 原生本地 plugin（加载自检/日志）
skills/aka-code-graph/SKILL.md     ② 使用策略（推荐载体：原生 skill，按需加载）
AGENTS-aka.md                      ② 备选载体：常驻指令（instructions 数组 / AGENTS.md）
```

## ① 启动 AKA 桌面端

安装并启动 AKA 桌面端后，它会自动在本机启动 MCP endpoint：

```text
http://127.0.0.1:4112/mcp
```

OpenCode 通过这个本地 endpoint 调用 aka 工具；不需要用户单独安装或启动 CLI 版 `aka.exe`。如果 OpenCode 提示 MCP 连接失败，先确认 AKA 桌面端正在运行。

## ② MCP 配置

OpenCode 的 MCP 配置写在 `opencode.json` 的 `mcp` 键下：全局 `~/.config/opencode/opencode.json`，或项目根的 `opencode.json`（可入 git，schema 相同；OpenCode 从当前目录向上找到最近的 git 根）。

把 [`opencode.json.snippet`](./opencode.json.snippet) 合并进你的 `opencode.json`：

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "aka": {
      "type": "remote",
      "url": "http://127.0.0.1:4112/mcp",
      "enabled": true
    }
  }
}
```

要点（与 Claude Code / Codex 的差异）：

- 默认使用 `"type": "remote"` + `url` 连接桌面 AKA 内置的本地 MCP server。
- 这个 URL 只绑定 `127.0.0.1`，不会暴露到局域网或公网。
- 临时停用设 `"enabled": false`，不必删配置。

也可直接跑仓库脚本：`clients/install.sh --client opencode`（幂等合并，需要 `jq`；会顺带安装下面的 plugin 与 skill）。

## ② 插件与使用策略

### OpenCode 原生 plugin

把 `plugins/aka.js` 拷到任一 OpenCode plugin 发现路径：

```bash
# 全局（所有项目可用）
mkdir -p ~/.config/opencode/plugins
cp plugins/aka.js ~/.config/opencode/plugins/

# 或项目级（仅该仓库）
mkdir -p <project>/.opencode/plugins
cp plugins/aka.js <project>/.opencode/plugins/
```

这个 plugin 不替代 MCP 配置；它只是 OpenCode 原生插件入口，用于确认 aka 集成包已加载。真正的工具调用由上面的 `mcp.aka` 配置连接正在运行的 AKA 桌面端。

只配 MCP 时 agent 看得到工具但不懂用法（决策表/输出解读/反模式）。三种载体选一种，**别同时启用 skill 和 AGENTS 版**（内容相同，重复浪费上下文）：

### 推荐：原生 skill（OpenCode 2026-06 起支持，按需加载最省 token）

把 `skills/aka-code-graph/` 整个目录拷到任一发现路径：

```bash
# 全局（所有项目可用）
mkdir -p ~/.config/opencode/skills
cp -R skills/aka-code-graph ~/.config/opencode/skills/

# 或项目级（仅该仓库）
mkdir -p <project>/.opencode/skills
cp -R skills/aka-code-graph <project>/.opencode/skills/
```

OpenCode 也会扫 `~/.claude/skills/` 与项目 `.claude/skills/`——如果你已经装过 aka 的 Claude Code 插件 skill，OpenCode 可能已自动识别，无需重复安装。agent 通过内置 `skill` 工具按 name/description 按需加载。

### 备选 A：`opencode.json` 的 `instructions` 数组（常驻，每会话都注入）

```json
{
  "$schema": "https://opencode.ai/config.json",
  "instructions": ["/absolute/path/to/AGENTS-aka.md"]
}
```

`instructions` 接受路径/glob/远程 URL 数组，内容与 AGENTS.md 合并。适合没有 skills 机制的旧版 OpenCode。

### 备选 B：追加进 AGENTS.md（常驻）

把 [`AGENTS-aka.md`](./AGENTS-aka.md) 的正文追加到项目根 `AGENTS.md`（仅该项目）或 `~/.config/opencode/AGENTS.md`（全局）。

## 验证

```bash
opencode                 # 启动 TUI
# 会话里问："用 aka 列出已索引的仓库" → 应调用 aka_list_repos 工具
# 装了 skill 的话问："你有 aka-code-graph skill 吗" → 应能通过 skill 工具加载
```

启动时如果 server 起不来，OpenCode 会在日志/状态里报该 MCP 初始化失败——通常是 AKA 桌面端没有运行。

格式参考（2026-06 核实）：
- https://opencode.ai/docs/mcp-servers/
- https://opencode.ai/docs/config/
- https://opencode.ai/docs/plugins/ — 本地插件放 `.opencode/plugins/` 或 `~/.config/opencode/plugins/`
- https://opencode.ai/docs/skills/ — 原生 Agent Skills，frontmatter 必需 `name`（小写字母数字+连字符，须与目录名一致）+ `description`（1–1024 字符）
- https://opencode.ai/docs/rules/ — AGENTS.md 与 `instructions` 数组
