#!/usr/bin/env bash
# Verify client/plugin release packages keep the agent-first indexing contract.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${REPO_ROOT}/dist"
WORKSPACE_VERSION="$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/,"",$3); print $3; exit}' "${REPO_ROOT}/Cargo.toml")"
VERSION="${WORKSPACE_VERSION}"

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: missing required command: $1" >&2
    exit 1
  }
}

require_file() {
  local path="$1"
  [[ -f "${path}" ]] || {
    echo "error: expected file missing: ${path}" >&2
    exit 1
  }
}

require_text() {
  local path="$1"
  local pattern="$2"
  local label="$3"
  if ! grep -Eq -- "${pattern}" "${path}"; then
    echo "error: ${label} missing in ${path}" >&2
    echo "       pattern: ${pattern}" >&2
    exit 1
  fi
}

reject_text() {
  local path="$1"
  local pattern="$2"
  local label="$3"
  if grep -Eq -- "${pattern}" "${path}"; then
    echo "error: ${label} found in ${path}" >&2
    echo "       pattern: ${pattern}" >&2
    exit 1
  fi
}

check_strategy_doc() {
  local path="$1"
  require_text "${path}" 'list_repos' "list_repos-first guidance"
  require_text "${path}" 'workspace roots|roots' "workspace roots auto-index guidance"
  require_text "${path}" '自动排队索引|queues? .*index|background indexing' "background indexing guidance"
  require_text "${path}" 'analyze' "analyze fallback guidance"
  require_text "${path}" 'import_repo' "remote git import guidance"
  require_text "${path}" 'status: ?"indexing"|status:"indexing"|indexing' "indexing status guidance"
  require_text "${path}" 'GitNexus-like' "GitNexus-like application semantics guidance"
  require_text "${path}" '不是完整 GitNexus|not .*complete GitNexus|not .*full GitNexus' "GitNexus non-equivalence guardrail"
  require_text "${path}" '连不上 aka|unreachable' "aka unavailable fallback guidance"
  require_text "${path}" 'dry_run:false' "write-boundary guidance"
  require_text "${path}" '不要把空结果当事实|Do not treat empty results as evidence' "indexing empty-result guardrail"
  reject_text "${path}" '完整等价于 GitNexus|complete GitNexus equivalent' "overstated GitNexus equivalence"
}

need tar
need unzip
need node

"${REPO_ROOT}/scripts/package-release.sh" --version "${VERSION}" --clients-only

CLAUDE_ZIP="${DIST_DIR}/aka-claude-code-plugin-${VERSION}.zip"
OPENCODE_ZIP="${DIST_DIR}/aka-opencode-plugin-${VERSION}.zip"
CLIENTS_TAR="${DIST_DIR}/aka-clients-${VERSION}.tar.gz"

require_file "${CLAUDE_ZIP}"
require_file "${OPENCODE_ZIP}"
require_file "${CLIENTS_TAR}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

mkdir -p "${TMP_DIR}/claude" "${TMP_DIR}/opencode" "${TMP_DIR}/clients"
unzip -q "${CLAUDE_ZIP}" -d "${TMP_DIR}/claude"
unzip -q "${OPENCODE_ZIP}" -d "${TMP_DIR}/opencode"
tar -xzf "${CLIENTS_TAR}" -C "${TMP_DIR}/clients"

require_file "${TMP_DIR}/claude/.claude-plugin/plugin.json"
require_file "${TMP_DIR}/claude/.mcp.json"
require_file "${TMP_DIR}/claude/skills/aka-code-graph/SKILL.md"
require_file "${TMP_DIR}/opencode/opencode.json.snippet"
require_file "${TMP_DIR}/opencode/plugins/aka.js"
require_file "${TMP_DIR}/opencode/skills/aka-code-graph/SKILL.md"
require_file "${TMP_DIR}/opencode/AGENTS-aka.md"
require_file "${TMP_DIR}/clients/clients/codex/AGENTS-aka.md"
require_file "${TMP_DIR}/clients/clients/codex/README.md"
require_file "${TMP_DIR}/clients/clients/install.sh"
require_file "${TMP_DIR}/clients/clients/install.ps1"
require_file "${TMP_DIR}/clients/clients/.claude-plugin/marketplace.json"

CLAUDE_TMP_DIR="${TMP_DIR}/claude" WORKSPACE_VERSION="${WORKSPACE_VERSION}" node <<'NODE'
const fs = require("fs");
const path = require("path");
const root = process.env.CLAUDE_TMP_DIR;
const expectedVersion = process.env.WORKSPACE_VERSION;
const plugin = JSON.parse(fs.readFileSync(path.join(root, ".claude-plugin/plugin.json"), "utf8"));
const mcp = JSON.parse(fs.readFileSync(path.join(root, ".mcp.json"), "utf8"));
if (plugin.version !== expectedVersion) {
  throw new Error(`plugin version ${plugin.version} does not match workspace ${expectedVersion}`);
}
if (plugin.license !== "MIT") {
  throw new Error(`plugin license must be MIT, got ${plugin.license}`);
}
if (!mcp.mcpServers || !mcp.mcpServers.aka) {
  throw new Error(".mcp.json must contain mcpServers.aka");
}
if (mcp.aka) {
  throw new Error(".mcp.json must not use legacy top-level aka");
}
if (mcp.mcpServers.aka.type !== "http" || mcp.mcpServers.aka.url !== "http://127.0.0.1:4112/mcp") {
  throw new Error("mcpServers.aka must point at the desktop HTTP MCP endpoint");
}
NODE

CLIENTS_TMP_DIR="${TMP_DIR}/clients/clients" node <<'NODE'
const fs = require("fs");
const path = require("path");
const root = process.env.CLIENTS_TMP_DIR;
const marketplace = JSON.parse(fs.readFileSync(path.join(root, ".claude-plugin/marketplace.json"), "utf8"));
const plugin = marketplace.plugins && marketplace.plugins.find((item) => item.name === "aka");
if (!plugin) {
  throw new Error("clients marketplace must list the aka plugin");
}
if (plugin.source !== "./claude-code") {
  throw new Error(`clients marketplace source must be ./claude-code, got ${plugin.source}`);
}
NODE

check_strategy_doc "${TMP_DIR}/claude/skills/aka-code-graph/SKILL.md"
check_strategy_doc "${TMP_DIR}/opencode/skills/aka-code-graph/SKILL.md"
check_strategy_doc "${TMP_DIR}/opencode/AGENTS-aka.md"
check_strategy_doc "${TMP_DIR}/clients/clients/codex/AGENTS-aka.md"

require_text "${TMP_DIR}/clients/clients/README.md" 'AKA 桌面端' "desktop-first client wording"
require_text "${TMP_DIR}/clients/clients/README.md" 'install\.ps1' "Windows installer guidance"
require_text "${TMP_DIR}/clients/clients/README.md" '--check|-Check' "script check guidance"
require_text "${TMP_DIR}/clients/clients/README.md" '--reinstall|-Reinstall' "script reinstall guidance"
require_text "${TMP_DIR}/clients/clients/README.md" 'list_repos' "client package list_repos guidance"
require_text "${TMP_DIR}/clients/clients/README.md" '自动排队索引' "client package auto-index guidance"
require_text "${TMP_DIR}/clients/clients/install.sh" '--check' "shell installer check mode"
require_text "${TMP_DIR}/clients/clients/install.sh" '--reinstall' "shell installer reinstall mode"
require_text "${TMP_DIR}/clients/clients/install.ps1" '-Check' "PowerShell installer check mode"
require_text "${TMP_DIR}/clients/clients/install.ps1" '-Reinstall' "PowerShell installer reinstall mode"
require_text "${TMP_DIR}/clients/clients/codex/README.md" 'workspace roots 自动索引' "Codex roots auto-index guidance"
require_text "${TMP_DIR}/clients/clients/opencode/README.md" 'MCP roots .*自动排队索引' "OpenCode roots auto-index guidance"
require_text "${TMP_DIR}/clients/clients/codex/config.toml.snippet" 'default_tools_approval_mode = "prompt"' "safe Codex approval guidance"

reject_text "${TMP_DIR}/clients/clients/README.md" '独立 CLI|裸 CLI|用户可见 CLI|CLI 版' "user-facing CLI product wording"
reject_text "${TMP_DIR}/clients/clients/install.sh" '独立 CLI|裸 CLI|用户可见 CLI|CLI 版' "user-facing CLI product wording"
reject_text "${TMP_DIR}/clients/clients/codex/config.toml.snippet" '全部只读|可放心 auto|default_tools_approval_mode = "auto"' "unsafe Codex auto approval guidance"
reject_text "${TMP_DIR}/clients/clients/claude-code/.mcp.json" '^  "aka"[[:space:]]*:' "legacy top-level Claude MCP config"
reject_text "${REPO_ROOT}/.claude-plugin/marketplace.json" '八工具|MCP 八' "stale MCP tool count wording"

echo "==> client package smoke passed (${VERSION})"
