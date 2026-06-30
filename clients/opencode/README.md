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

OpenCode 通过这个本地 endpoint 调用 aka 工具；不需要用户单独安装或启动额外命令行程序。AI 索引/查询结果会进入同一份 GUI 可见知识库。如果 OpenCode 提示 MCP 连接失败，先确认 AKA 桌面端正在运行。

默认 remote 方式会优先通过 MCP roots 读取当前 OpenCode workspace 并自动排队索引；如果客户端未暴露 roots，服务端会用进程工作目录兜底，本地路径参数也会自动提升到 git/project root 并排队索引；仍找不到时 agent 再用 `analyze` 显式传当前仓库绝对路径。把 OpenCode 配成 stdio 方式直接运行 `AKA mcp` 时，aka 也会自动发现当前工作区，并且和桌面 GUI 共用同一份数据。

## ② MCP 配置

OpenCode 的 MCP 配置写在 `opencode.json` 的 `mcp` 键下：全局 macOS/Linux `~/.config/opencode/opencode.json`，Windows `%USERPROFILE%\.config\opencode\opencode.json`，或项目根的 `opencode.json`（可入 git，schema 相同；OpenCode 从当前目录向上查找项目配置直到最近的 git worktree）。

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

macOS/Linux 也可直接跑安装脚本（幂等合并，需要 `jq`；会顺带安装下面的 plugin 与 skill）：

```bash
clients/install.sh --check
clients/install.sh --client opencode --reinstall
```

Windows PowerShell：

```powershell
.\clients\install.ps1 -Check
.\clients\install.ps1 -Client opencode -Reinstall
```

### Windows 安装

先安装并启动 Windows 版 AKA 桌面端，确认它在本机监听 `http://127.0.0.1:4112/mcp`。推荐使用 `aka-clients-<ver>.tar.gz` 里的 `clients\install.ps1`；如果只下载了 `aka-opencode-plugin-<ver>.zip`，可按下面手动步骤安装。

在 PowerShell 里进入解压目录，安装全局 OpenCode 配置、plugin 和 skill：

```powershell
$pkg = (Get-Location).Path
$cfgDir = Join-Path $HOME ".config\opencode"
$cfg = Join-Path $cfgDir "opencode.json"

New-Item -ItemType Directory -Force $cfgDir | Out-Null
New-Item -ItemType Directory -Force (Join-Path $cfgDir "plugins") | Out-Null
New-Item -ItemType Directory -Force (Join-Path $cfgDir "skills") | Out-Null

if (!(Test-Path $cfg)) {
  '{ "$schema": "https://opencode.ai/config.json" }' | Set-Content -Encoding UTF8 $cfg
}

$json = Get-Content $cfg -Raw | ConvertFrom-Json
if ($null -eq $json.mcp) {
  $json | Add-Member -NotePropertyName mcp -NotePropertyValue ([pscustomobject]@{})
}
if ($json.mcp.PSObject.Properties.Name -contains "aka") {
  $json.mcp.aka = [pscustomobject]@{
    type = "remote"
    url = "http://127.0.0.1:4112/mcp"
    enabled = $true
  }
} else {
  $json.mcp | Add-Member -NotePropertyName aka -NotePropertyValue ([pscustomobject]@{
    type = "remote"
    url = "http://127.0.0.1:4112/mcp"
    enabled = $true
  })
}
$json | ConvertTo-Json -Depth 20 | Set-Content -Encoding UTF8 $cfg

Copy-Item (Join-Path $pkg "plugins\aka.js") (Join-Path $cfgDir "plugins\aka.js") -Force
Copy-Item (Join-Path $pkg "skills\aka-code-graph") (Join-Path $cfgDir "skills") -Recurse -Force
```

如果你只想给当前项目安装，把配置片段合并进项目根 `opencode.json`，并把文件复制到项目里的 `.opencode` 目录：

```powershell
New-Item -ItemType Directory -Force ".opencode\plugins" | Out-Null
New-Item -ItemType Directory -Force ".opencode\skills" | Out-Null
Copy-Item "plugins\aka.js" ".opencode\plugins\aka.js" -Force
Copy-Item "skills\aka-code-graph" ".opencode\skills" -Recurse -Force
```

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

Windows 全局路径是 `%USERPROFILE%\.config\opencode\plugins\aka.js`；项目级路径仍是 `<project>\.opencode\plugins\aka.js`。

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

Windows 全局路径是 `%USERPROFILE%\.config\opencode\skills\aka-code-graph\SKILL.md`；项目级路径仍是 `<project>\.opencode\skills\aka-code-graph\SKILL.md`。

### 备选 A：`opencode.json` 的 `instructions` 数组（常驻，每会话都注入）

```json
{
  "$schema": "https://opencode.ai/config.json",
  "instructions": ["/absolute/path/to/AGENTS-aka.md"]
}
```

`instructions` 接受路径/glob/远程 URL 数组，内容与 AGENTS.md 合并。适合没有 skills 机制的旧版 OpenCode。

### 备选 B：追加进 AGENTS.md（常驻）

把 [`AGENTS-aka.md`](./AGENTS-aka.md) 的正文追加到项目根 `AGENTS.md`（仅该项目）或全局规则文件：macOS/Linux `~/.config/opencode/AGENTS.md`，Windows `%USERPROFILE%\.config\opencode\AGENTS.md`。

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
- https://opencode.ai/docs/troubleshooting/ — Windows 全局配置和插件目录使用 `%USERPROFILE%\.config\opencode`
