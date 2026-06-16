# aka × Claude Code 插件

把 aka 作为 Claude Code 插件安装：捆绑 **AKA 桌面端本地 HTTP MCP 配置**（19 个工具）+ **aka-code-graph skill**（指导 agent 何时用哪个工具）。AI 的索引/查询结果会进入同一份 GUI 可见知识库。

## 目录结构

```
clients/claude-code/            # 插件根
├── .claude-plugin/plugin.json  # 插件清单（name 是唯一必填字段）
├── .mcp.json                   # 捆绑的 MCP server 配置（默认连接 http://127.0.0.1:4112/mcp）
└── skills/aka-code-graph/SKILL.md
```

仓库根另有 `.claude-plugin/marketplace.json`，把本仓库变成一个可 `marketplace add` 的 marketplace，插件源指向 `./clients/claude-code`。

## 前置条件：启动 AKA 桌面端

安装并启动 AKA 桌面端后，它会自动在本机启动 MCP endpoint：

```text
http://127.0.0.1:4112/mcp
```

Claude Code 通过这个本地 HTTP MCP 调用 aka 工具；不需要单独启动额外命令行程序。

## 安装

```bash
# 1. 把本仓库注册为 marketplace（本地路径或 git 均可）
claude plugin marketplace add /absolute/path/to/aka     # 本地
#   或：claude plugin marketplace add caork/aka          # GitHub（私库需 gh 登录）

# 2. 安装插件
claude plugin install aka@aka

# 本地开发免安装试跑：
claude --plugin-dir /absolute/path/to/aka/clients/claude-code
```

也可以不走插件，直接注册 MCP server（没有 skill，但最简单）：

```bash
claude mcp add --transport http aka http://127.0.0.1:4112/mcp
```

或直接跑仓库里的安装脚本：`clients/install.sh --client claude-code`。

桌面不常开或当前 Claude Code 版本不支持 HTTP MCP 时，可用 stdio fallback：

```bash
claude mcp add --transport stdio aka -- /absolute/path/to/AKA mcp
clients/install.sh --client claude-code --stdio --bin /absolute/path/to/AKA
```

stdio fallback 使用桌面包里的 `AKA` 时也会和 GUI 共用同一份桌面数据。

## 验证

```bash
claude plugin list                 # 应看到 aka@aka
claude mcp list                    # 启动 AKA 桌面端后应看到 aka connected
# 进入 claude 会话，问一句"列出 aka 已索引的仓库"→ 应触发 list_repos 工具
```

校验清单格式：

```bash
claude plugin validate clients/claude-code
claude plugin validate .                     # 校验 marketplace.json
```

> 注意：解析引擎 AKA engine 为 MIT，本插件随 aka 客户端接入按 MIT 口径分发。
