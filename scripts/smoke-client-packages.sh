#!/usr/bin/env bash
# Verify client/plugin release packages keep the agent-first indexing contract.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${AKA_CLIENT_SMOKE_VERSION:-client-smoke}"
DIST_DIR="${REPO_ROOT}/dist"

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
  if ! grep -Eq "${pattern}" "${path}"; then
    echo "error: ${label} missing in ${path}" >&2
    echo "       pattern: ${pattern}" >&2
    exit 1
  fi
}

reject_text() {
  local path="$1"
  local pattern="$2"
  local label="$3"
  if grep -Eq "${pattern}" "${path}"; then
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
  reject_text "${path}" '完整等价于 GitNexus|complete GitNexus equivalent' "overstated GitNexus equivalence"
}

need tar
need unzip

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

check_strategy_doc "${TMP_DIR}/claude/skills/aka-code-graph/SKILL.md"
check_strategy_doc "${TMP_DIR}/opencode/skills/aka-code-graph/SKILL.md"
check_strategy_doc "${TMP_DIR}/opencode/AGENTS-aka.md"
check_strategy_doc "${TMP_DIR}/clients/clients/codex/AGENTS-aka.md"

require_text "${TMP_DIR}/clients/clients/README.md" 'AKA 桌面端' "desktop-first client wording"
require_text "${TMP_DIR}/clients/clients/README.md" 'list_repos' "client package list_repos guidance"
require_text "${TMP_DIR}/clients/clients/README.md" '自动排队索引' "client package auto-index guidance"
require_text "${TMP_DIR}/clients/clients/codex/README.md" 'workspace roots 自动索引' "Codex roots auto-index guidance"
require_text "${TMP_DIR}/clients/clients/opencode/README.md" 'MCP roots .*自动排队索引' "OpenCode roots auto-index guidance"

reject_text "${TMP_DIR}/clients/clients/README.md" '独立 CLI|裸 CLI|用户可见 CLI|CLI 版' "user-facing CLI product wording"
reject_text "${TMP_DIR}/clients/clients/install.sh" '独立 CLI|裸 CLI|用户可见 CLI|CLI 版' "user-facing CLI product wording"

echo "==> client package smoke passed (${VERSION})"
