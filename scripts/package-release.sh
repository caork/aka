#!/usr/bin/env bash
# package-release.sh — 打包 v<ver> 发布资产到 dist/
#
# 用法:
#   scripts/package-release.sh [--version 0.1.0] [--skip-build]
#   scripts/package-release.sh --checksums-only [--version 0.1.0]
#
# 产物（主流程）:
#   dist/aka-claude-code-plugin-<ver>.zip   clients/claude-code/ 插件包
#   dist/aka-opencode-<ver>.zip             clients/opencode/ 客户端包（MCP 片段 + skill + AGENTS 版指令）
#   dist/aka-clients-<ver>.tar.gz           整个 clients/ 目录
#   dist/aka-<ver>-<host-triple>.tar.gz     strip 后的 release 二进制（tar 内单文件 aka）
#
# 产物（--checksums-only 子命令，主流程不自动跑）:
#   dist/SHA256SUMS                         dist/ 下所有产物（含 dist/docker/*）的 sha256
#
# 注意: 不清空 dist/（dist/docker/ 是另一条流水线的产物位），只覆盖本脚本负责的文件。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${REPO_ROOT}/dist"

VERSION=""
SKIP_BUILD=0
CHECKSUMS_ONLY=0

usage() {
  sed -n '2,17p' "${BASH_SOURCE[0]}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      [[ $# -ge 2 ]] || { echo "error: --version 需要参数" >&2; exit 1; }
      VERSION="$2"; shift 2 ;;
    --skip-build)
      SKIP_BUILD=1; shift ;;
    --checksums-only)
      CHECKSUMS_ONLY=1; shift ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "error: 未知参数 $1" >&2; usage >&2; exit 1 ;;
  esac
done

# 默认版本号取根 Cargo.toml 的 [workspace.package] version
if [[ -z "${VERSION}" ]]; then
  VERSION="$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/,"",$3); print $3; exit}' "${REPO_ROOT}/Cargo.toml")"
  [[ -n "${VERSION}" ]] || { echo "error: 无法从 Cargo.toml 读取 workspace 版本" >&2; exit 1; }
fi

mkdir -p "${DIST_DIR}"

# ---------- 子命令: 只生成校验和（放在最后由主线程统一跑） ----------
if [[ "${CHECKSUMS_ONLY}" -eq 1 ]]; then
  cd "${DIST_DIR}"
  files=()
  while IFS= read -r f; do
    files+=("${f#./}")
  done < <(find . -type f ! -name SHA256SUMS ! -name .DS_Store | sort)
  [[ ${#files[@]} -gt 0 ]] || { echo "error: dist/ 下没有可校验的产物" >&2; exit 1; }
  shasum -a 256 "${files[@]}" > SHA256SUMS
  echo "==> dist/SHA256SUMS"
  cat SHA256SUMS
  exit 0
fi

echo "==> 版本: ${VERSION}"

# ---------- 1. claude-code 插件 zip ----------
PLUGIN_ZIP="${DIST_DIR}/aka-claude-code-plugin-${VERSION}.zip"
rm -f "${PLUGIN_ZIP}"
(
  cd "${REPO_ROOT}/clients/claude-code"
  zip -r -X -q "${PLUGIN_ZIP}" . -x "*.DS_Store" -x "*/.DS_Store"
)
echo "==> ${PLUGIN_ZIP}"

# 自检: 插件关键文件必须在包里（先取列表再 grep，避免 pipefail+SIGPIPE 误报）
plugin_listing="$(unzip -l "${PLUGIN_ZIP}")"
for entry in ".claude-plugin/plugin.json" ".mcp.json" "skills/aka-code-graph/SKILL.md"; do
  if ! grep -qF " ${entry}" <<< "${plugin_listing}"; then
    echo "error: 插件包缺少 ${entry}" >&2; exit 1
  fi
done

# ---------- 2. opencode 客户端 zip ----------
OPENCODE_ZIP="${DIST_DIR}/aka-opencode-${VERSION}.zip"
rm -f "${OPENCODE_ZIP}"
(
  cd "${REPO_ROOT}/clients/opencode"
  zip -r -X -q "${OPENCODE_ZIP}" . -x "*.DS_Store" -x "*/.DS_Store" -x "._*" -x "*/._*"
)
echo "==> ${OPENCODE_ZIP}"

# 自检: opencode 包关键文件必须在包里
opencode_listing="$(unzip -l "${OPENCODE_ZIP}")"
for entry in "README.md" "opencode.json.snippet" "AGENTS-aka.md" "skills/aka-code-graph/SKILL.md"; do
  if ! grep -qF " ${entry}" <<< "${opencode_listing}"; then
    echo "error: opencode 包缺少 ${entry}" >&2; exit 1
  fi
done

# ---------- 3. 整个 clients/ tar.gz ----------
CLIENTS_TAR="${DIST_DIR}/aka-clients-${VERSION}.tar.gz"
rm -f "${CLIENTS_TAR}"
COPYFILE_DISABLE=1 tar -czf "${CLIENTS_TAR}" \
  --exclude '.DS_Store' \
  -C "${REPO_ROOT}" clients
echo "==> ${CLIENTS_TAR}"

# ---------- 4. 本机 release 二进制 tar.gz ----------
case "$(uname -m)" in
  arm64|aarch64) ARCH="aarch64" ;;
  x86_64)        ARCH="x86_64" ;;
  *) echo "error: 不支持的架构 $(uname -m)" >&2; exit 1 ;;
esac
case "$(uname -s)" in
  Darwin) TRIPLE="${ARCH}-apple-darwin" ;;
  Linux)  TRIPLE="${ARCH}-unknown-linux-gnu" ;;
  *) echo "error: 不支持的系统 $(uname -s)" >&2; exit 1 ;;
esac

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
  echo "==> cargo build --release -p aka-cli"
  (cd "${REPO_ROOT}" && cargo build --release -p aka-cli)
fi

BIN="${REPO_ROOT}/target/release/aka"
[[ -x "${BIN}" ]] || { echo "error: 找不到 ${BIN}（先去掉 --skip-build 构建一次）" >&2; exit 1; }

BIN_TAR="${DIST_DIR}/aka-${VERSION}-${TRIPLE}.tar.gz"
rm -f "${BIN_TAR}"
STAGE="$(mktemp -d)"
trap 'rm -rf "${STAGE}"' EXIT
cp "${BIN}" "${STAGE}/aka"
strip "${STAGE}/aka"
COPYFILE_DISABLE=1 tar -czf "${BIN_TAR}" -C "${STAGE}" aka
echo "==> ${BIN_TAR}"

echo
echo "==> 完成。校验和请最后单独跑: scripts/package-release.sh --checksums-only"
ls -lh "${PLUGIN_ZIP}" "${OPENCODE_ZIP}" "${CLIENTS_TAR}" "${BIN_TAR}"
