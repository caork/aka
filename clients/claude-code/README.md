# aka × Claude Code 插件

把 aka 作为 Claude Code 插件安装：捆绑 **aka MCP server**（stdio，八工具）+ **aka-code-graph skill**（指导 agent 何时用哪个工具）。

## 目录结构

```
clients/claude-code/            # 插件根
├── .claude-plugin/plugin.json  # 插件清单（name 是唯一必填字段）
├── .mcp.json                   # 捆绑的 MCP server 配置
└── skills/aka-code-graph/SKILL.md
```

仓库根另有 `.claude-plugin/marketplace.json`，把本仓库变成一个可 `marketplace add` 的 marketplace，插件源指向 `./clients/claude-code`。

## 前置条件：aka 二进制在 PATH 上

`.mcp.json` 里 `command` 写的是 `"aka"`（按 PATH 解析）。插件清单不支持安装时动态探测路径，所以二选一：

```bash
# 方式 A：装到 PATH（推荐）
cargo install --path apps/cli            # 或
ln -s "$(pwd)/target/release/aka" ~/.local/bin/aka

# 方式 B：不想动 PATH —— 改本目录 .mcp.json，把 "command": "aka"
#         换成你的绝对路径，如 "/Users/you/github/aka/target/release/aka"
#         （改完再安装插件；插件安装时会把目录拷进缓存，装后再改源文件不生效）
```

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
claude mcp add aka -- /absolute/path/to/aka mcp
```

或直接跑仓库里的安装脚本：`clients/install.sh --client claude-code`。

## 验证

```bash
claude plugin list                 # 应看到 aka@aka
claude mcp list                    # 应看到 aka（plugin 提供或手动 add 的）
# 进入 claude 会话，问一句"列出 aka 已索引的仓库"→ 应触发 list_repos 工具
```

校验清单格式：

```bash
claude plugin validate clients/claude-code
claude plugin validate .                     # 校验 marketplace.json
```

> 注意：上游 License 为 PolyForm Noncommercial 1.0，本插件随整个项目仅限非商用。
