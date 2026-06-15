#!/usr/bin/env bash
# aka 客户端接入安装脚本
#
# 用法:
#   clients/install.sh --client claude-code [--plugin] [--bin /path/to/AKA] [--dry-run]
#   clients/install.sh --client codex       [--bin /path/to/AKA] [--dry-run]
#   clients/install.sh --client opencode    [--dry-run]
#
# 行为:
#   - Claude Code / Codex 自动探测 MCP 命令: --bin 参数 > PATH 上的 aka/AKA > 仓库 target/release > target/debug
#   - OpenCode 默认连接已运行的 AKA 桌面端本地 MCP endpoint，不需要单独的 CLI 二进制
#   - 幂等: 目标配置里已有 aka 条目时跳过并提示, 不会重复写入
#   - --dry-run: 只打印将要执行的动作, 不写任何文件
#
# 各客户端写入目标:
#   claude-code : 默认 `claude mcp add aka`(user scope); 加 --plugin 则走插件方式
#                 (marketplace add 本仓库 + plugin install aka@aka, 含 skill)
#   codex       : 追加 [mcp_servers.aka] 到 ~/.codex/config.toml
#   opencode    : 合并远程 mcp.aka 进 ~/.config/opencode/opencode.json (需要 jq) + 装 OpenCode plugin + 使用策略 skill

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

CLIENT=""
BIN=""
DRY_RUN=0
USE_PLUGIN=0

info()  { printf '\033[1;34m[aka]\033[0m %s\n' "$*"; }
warn()  { printf '\033[1;33m[aka]\033[0m %s\n' "$*" >&2; }
die()   { printf '\033[1;31m[aka]\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
  sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

# ---------- 参数解析 ----------
while [ $# -gt 0 ]; do
  case "$1" in
    --client)  CLIENT="${2:-}"; shift 2 ;;
    --bin)     BIN="${2:-}"; shift 2 ;;
    --plugin)  USE_PLUGIN=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage 0 ;;
    *) die "未知参数: $1（--help 查看用法）" ;;
  esac
done

case "$CLIENT" in
  claude-code|codex|opencode) ;;
  "") usage 1 ;;
  *) die "不支持的 --client: ${CLIENT}（可选 claude-code|codex|opencode）" ;;
esac

# ---------- 探测 aka 二进制 ----------
detect_bin() {
  if [ -n "$BIN" ]; then
    [ -x "$BIN" ] || die "--bin 指定的文件不可执行: $BIN"
    return
  fi
  for name in aka AKA; do
    if command -v "$name" >/dev/null 2>&1; then
      BIN="$(command -v "$name")"
      return
    fi
  done
  for cand in \
    "${REPO_ROOT}/target/release/aka" \
    "${REPO_ROOT}/target/debug/aka" \
    "${REPO_ROOT}/apps/desktop/src-tauri/target/release/AKA" \
    "${REPO_ROOT}/apps/desktop/src-tauri/target/release/AKA.exe"
  do
    if [ -x "$cand" ]; then
      BIN="$cand"
      return
    fi
  done
  die "找不到可执行的 aka/AKA。请用 --bin 指向桌面包里的 AKA 可执行文件，或源码开发时先 cargo build -p aka-cli。"
}

if [ "$CLIENT" != "opencode" ]; then
  detect_bin
  info "aka MCP 命令: ${BIN}"
elif [ -n "$BIN" ]; then
  warn "OpenCode 默认连接 http://127.0.0.1:4112/mcp，不会使用 --bin 指定的路径: ${BIN}"
fi

run() {
  if [ "$DRY_RUN" -eq 1 ]; then
    info "[dry-run] $*"
  else
    "$@"
  fi
}

# ---------- claude-code ----------
install_claude_code() {
  command -v claude >/dev/null 2>&1 || die "未找到 claude CLI，请先安装 Claude Code。"

  if [ "$USE_PLUGIN" -eq 1 ]; then
    # 插件方式: marketplace add 本仓库 + 安装插件(捆绑 MCP server + skill)。
    # 注意: 插件内 .mcp.json 的 command 是 "aka"(按 PATH 解析)。
    if ! command -v aka >/dev/null 2>&1; then
      warn "aka 不在 PATH 上。插件清单无法动态指定路径，两个选择:"
      warn "  1) 给桌面 AKA 建一个 aka 软链: ln -s ${BIN} ~/.local/bin/aka"
      warn "  2) 编辑 ${SCRIPT_DIR}/claude-code/.mcp.json，把 \"aka\" 换成绝对路径 ${BIN} 后重跑"
      die  "处理后重新运行本脚本。"
    fi
    if claude plugin list 2>/dev/null | grep -q '^aka@aka\|aka@aka'; then
      info "插件 aka@aka 已安装，跳过。(更新: claude plugin update aka@aka)"
      return
    fi
    run claude plugin marketplace add "${REPO_ROOT}"
    run claude plugin install aka@aka
    info "完成。验证: claude plugin list && claude mcp list"
  else
    # 直连 MCP 方式(无 skill, 最简单), user scope 全项目可用。
    if claude mcp list 2>/dev/null | grep -q '^aka[: ]\|^aka$'; then
      info "MCP server 'aka' 已存在，跳过。(查看: claude mcp list; 移除: claude mcp remove aka)"
      return
    fi
    run claude mcp add --scope user aka -- "${BIN}" mcp
    info "完成。验证: claude mcp list（应显示 aka ✓ connected）"
    info "提示: 想要捆绑 skill 的插件方式，用 --plugin 重跑。"
  fi
}

# ---------- codex ----------
install_codex() {
  local cfg="${HOME}/.codex/config.toml"
  if [ -f "$cfg" ] && grep -q '^\[mcp_servers\.aka\]' "$cfg"; then
    info "${cfg} 已有 [mcp_servers.aka]，跳过。"
    return
  fi
  if command -v codex >/dev/null 2>&1; then
    # 官方 CLI 写入，最稳
    run codex mcp add aka -- "${BIN}" mcp
  else
    info "未找到 codex CLI，直接追加 TOML 到 ${cfg}"
    if [ "$DRY_RUN" -eq 1 ]; then
      info "[dry-run] 将追加: [mcp_servers.aka] command=\"${BIN}\" args=[\"mcp\"]"
    else
      mkdir -p "${HOME}/.codex"
      {
        printf '\n[mcp_servers.aka]\n'
        printf 'command = "%s"\n' "${BIN}"
        printf 'args = ["mcp"]\n'
      } >> "$cfg"
    fi
  fi
  info "完成。验证: codex mcp list（或 codex TUI 里 /mcp）"
}

# ---------- opencode ----------
# OpenCode 原生本地 plugin: 用于确认 aka 集成包已加载；工具能力仍来自 mcp.aka。
install_opencode_plugin() {
  local src="${SCRIPT_DIR}/opencode/plugins/aka.js"
  local dst="${HOME}/.config/opencode/plugins/aka.js"
  [ -f "$src" ] || { warn "未找到 ${src}，跳过 OpenCode plugin 安装。"; return; }
  if [ -f "$dst" ]; then
    info "OpenCode plugin 已存在: ${dst}，跳过。"
    return
  fi
  if [ "$DRY_RUN" -eq 1 ]; then
    info "[dry-run] 将拷贝 ${src} -> ${dst}"
    return
  fi
  mkdir -p "$(dirname "$dst")"
  cp "$src" "$dst"
  info "已安装 OpenCode plugin: ${dst}"
}

# 使用策略 skill: OpenCode 原生支持 SKILL.md(2026-06 核实, 也兼容 ~/.claude/skills/)。
# 全局装到 ~/.config/opencode/skills/aka-code-graph/; 幂等, 已存在则跳过。
install_opencode_skill() {
  local src="${SCRIPT_DIR}/opencode/skills/aka-code-graph"
  local dst="${HOME}/.config/opencode/skills/aka-code-graph"
  [ -f "${src}/SKILL.md" ] || { warn "未找到 ${src}/SKILL.md，跳过 skill 安装。"; return; }
  if [ -f "${dst}/SKILL.md" ]; then
    info "skill 已存在: ${dst}，跳过。(也可改用 AGENTS-aka.md, 见 ${SCRIPT_DIR}/opencode/README.md)"
    return
  fi
  if [ -f "${HOME}/.claude/skills/aka-code-graph/SKILL.md" ]; then
    info "检测到 ~/.claude/skills/aka-code-graph(OpenCode 会自动识别), 跳过重复安装。"
    return
  fi
  if [ "$DRY_RUN" -eq 1 ]; then
    info "[dry-run] 将拷贝 ${src} -> ${dst}"
    return
  fi
  mkdir -p "$(dirname "$dst")"
  cp -R "$src" "$dst"
  info "已安装使用策略 skill: ${dst}"
}

install_opencode() {
  local cfg="${HOME}/.config/opencode/opencode.json"
  if [ -f "$cfg" ] && command -v jq >/dev/null 2>&1 && [ "$(jq -r '.mcp.aka // empty | type' "$cfg" 2>/dev/null)" = "object" ]; then
    info "${cfg} 已有 mcp.aka，跳过。"
    install_opencode_plugin
    install_opencode_skill
    return
  fi
  if ! command -v jq >/dev/null 2>&1; then
    warn "未安装 jq，无法安全合并 JSON。请手动把下面片段合并进 ${cfg}:"
    printf '{\n  "mcp": {\n    "aka": { "type": "remote", "url": "http://127.0.0.1:4112/mcp", "enabled": true }\n  }\n}\n'
    install_opencode_plugin
    install_opencode_skill
    return
  fi
  if [ "$DRY_RUN" -eq 1 ]; then
    info "[dry-run] 将向 ${cfg} 合并 mcp.aka = {type:remote, url:http://127.0.0.1:4112/mcp, enabled:true}"
    install_opencode_plugin
    install_opencode_skill
    return
  fi
  mkdir -p "$(dirname "$cfg")"
  # shellcheck disable=SC2016  # $schema 是字面量, 不是 shell 变量
  [ -f "$cfg" ] || printf '{ "$schema": "https://opencode.ai/config.json" }\n' > "$cfg"
  local tmp
  tmp="$(mktemp)"
  jq '.mcp.aka = {type: "remote", url: "http://127.0.0.1:4112/mcp", enabled: true}' "$cfg" > "$tmp"
  mv "$tmp" "$cfg"
  info "完成。验证: 先启动 AKA 桌面端，再启动 opencode，让它调用 aka 的 list_repos。"
  install_opencode_plugin
  install_opencode_skill
}

case "$CLIENT" in
  claude-code) install_claude_code ;;
  codex)       install_codex ;;
  opencode)    install_opencode ;;
esac
