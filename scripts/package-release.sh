#!/usr/bin/env bash
# package-release.sh — 打包 v<ver> 发布资产到 dist/
#
# 用法:
#   scripts/package-release.sh [--version 0.1.0] [--target <triple>] [--skip-build] [--clients-only] [--desktop]
#   scripts/package-release.sh --desktop-only [--version 0.1.0] [--skip-build]
#   scripts/package-release.sh --desktop-windows [--version 0.1.0] [--skip-build]
#   scripts/package-release.sh --checksums-only [--version 0.1.0]
#
# 产物（主流程）:
#   dist/aka-claude-code-plugin-<ver>.zip   clients/claude-code/ 插件包
#   dist/aka-opencode-plugin-<ver>.zip      clients/opencode/ 插件包（MCP 片段 + skill + 本地 plugin）
#   dist/aka-clients-<ver>.tar.gz           整个 clients/ 目录
#   dist/aka-<ver>-<host-triple>.tar.gz     strip 后的 release 二进制（tar 内单文件 aka）
#   dist/aka-<ver>-<host-triple>.zip        Windows release 二进制（zip 内单文件 aka.exe）
#   dist/aka-desktop-<ver>-<host-triple>.app.zip
#                                             macOS Tauri GUI app（zip 内 aka.app）
#   dist/aka-desktop-<ver>-x86_64-pc-windows-msvc-setup.exe
#                                             Windows Tauri GUI NSIS installer
#   dist/aka-desktop-<ver>-x86_64-pc-windows-msvc-portable.zip
#                                             Windows Tauri GUI portable exe
#
# 产物（--checksums-only 子命令，主流程不自动跑）:
#   dist/SHA256SUMS                         dist/ 下所有产物（含 dist/docker/*）的 sha256
#
# 注意: 不清空 dist/（dist/docker/ 是另一条流水线的产物位），只覆盖本脚本负责的文件。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${REPO_ROOT}/dist"

VERSION=""
TARGET=""
SKIP_BUILD=0
CHECKSUMS_ONLY=0
CLIENTS_ONLY=0
DESKTOP=0
DESKTOP_ONLY=0
DESKTOP_WINDOWS=0

usage() {
  sed -n '2,22p' "${BASH_SOURCE[0]}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      [[ $# -ge 2 ]] || { echo "error: --version 需要参数" >&2; exit 1; }
      VERSION="$2"; shift 2 ;;
    --target)
      [[ $# -ge 2 ]] || { echo "error: --target 需要参数" >&2; exit 1; }
      TARGET="$2"; shift 2 ;;
    --skip-build)
      SKIP_BUILD=1; shift ;;
    --clients-only)
      CLIENTS_ONLY=1; shift ;;
    --desktop)
      DESKTOP=1; shift ;;
    --desktop-only)
      DESKTOP=1; DESKTOP_ONLY=1; shift ;;
    --desktop-windows|--desktop-portable-windows)
      DESKTOP_WINDOWS=1; shift ;;
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

package_desktop() {
  local host_os host_arch desktop_triple app_path desktop_zip
  host_os="$(uname -s)"
  host_arch="$(uname -m)"

  case "${host_os}" in
    Darwin)
      case "${host_arch}" in
        arm64|aarch64) desktop_triple="aarch64-apple-darwin" ;;
        x86_64)        desktop_triple="x86_64-apple-darwin" ;;
        *) echo "error: 不支持的 macOS 架构 ${host_arch}" >&2; return 1 ;;
      esac

      if [[ "${SKIP_BUILD}" -eq 0 ]]; then
        echo "==> npm run tauri -- build --bundles app --ci --no-sign"
        (cd "${REPO_ROOT}/apps/desktop" && npm run tauri -- build --bundles app --ci --no-sign)
      fi

      app_path="${REPO_ROOT}/apps/desktop/src-tauri/target/release/bundle/macos/aka.app"
      [[ -d "${app_path}" ]] || { echo "error: 找不到 ${app_path}（先去掉 --skip-build 构建一次）" >&2; return 1; }

      desktop_zip="${DIST_DIR}/aka-desktop-${VERSION}-${desktop_triple}.app.zip"
      rm -f "${desktop_zip}"
      COPYFILE_DISABLE=1 ditto -c -k --norsrc --keepParent "${app_path}" "${desktop_zip}"
      echo "==> ${desktop_zip}"
      ;;
    *)
      echo "error: --desktop/--desktop-only 当前只打包本机 macOS GUI；Windows GUI 请用 --desktop-windows。" >&2
      return 1
      ;;
  esac
}

package_windows_desktop() {
  local win_triple exe_path setup_src setup_exe portable_zip stage
  win_triple="x86_64-pc-windows-msvc"

  if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    local tauri_args=(build --target "${win_triple}" --bundles nsis --ci)
    if command -v cargo-xwin >/dev/null 2>&1 && [[ "$(uname -s)" = "Darwin" ]]; then
      tauri_args=(build --runner cargo-xwin --target "${win_triple}" --bundles nsis --ci)
    fi
    echo "==> npm run tauri -- ${tauri_args[*]}"
    (cd "${REPO_ROOT}/apps/desktop" && npm run tauri -- "${tauri_args[@]}")
  fi

  exe_path="${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/aka-desktop.exe"
  [[ -f "${exe_path}" ]] || { echo "error: 找不到 ${exe_path}（先去掉 --skip-build 构建一次）" >&2; return 1; }

  setup_src="$(find "${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/bundle/nsis" -maxdepth 1 -type f -name '*setup.exe' | sort | tail -n 1 || true)"
  [[ -f "${setup_src}" ]] || { echo "error: 找不到 Windows NSIS 安装器（先去掉 --skip-build 构建一次）" >&2; return 1; }

  setup_exe="${DIST_DIR}/aka-desktop-${VERSION}-${win_triple}-setup.exe"
  rm -f "${setup_exe}"
  cp "${setup_src}" "${setup_exe}"
  echo "==> ${setup_exe}"

  portable_zip="${DIST_DIR}/aka-desktop-${VERSION}-${win_triple}-portable.zip"
  rm -f "${portable_zip}"
  stage="$(mktemp -d)"
  cp "${exe_path}" "${stage}/aka-desktop.exe"
  (cd "${stage}" && zip -q -X -r "${portable_zip}" aka-desktop.exe)
  rm -rf "${stage}"
  echo "==> ${portable_zip}"
}

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

if [[ "${DESKTOP_ONLY}" -eq 1 ]]; then
  package_desktop
  echo
  echo "==> 完成桌面 GUI 包。校验和请最后单独跑: scripts/package-release.sh --checksums-only"
  ls -lh "${DIST_DIR}/aka-desktop-${VERSION}-"*.app.zip
  exit 0
fi

if [[ "${DESKTOP_WINDOWS}" -eq 1 ]]; then
  package_windows_desktop
  echo
  echo "==> 完成 Windows GUI 包。校验和请最后单独跑: scripts/package-release.sh --checksums-only"
  ls -lh "${DIST_DIR}/aka-desktop-${VERSION}-x86_64-pc-windows-msvc-"*
  exit 0
fi

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

# ---------- 2. opencode 插件 zip ----------
OPENCODE_ZIP="${DIST_DIR}/aka-opencode-plugin-${VERSION}.zip"
rm -f "${OPENCODE_ZIP}"
(
  cd "${REPO_ROOT}/clients/opencode"
  zip -r -X -q "${OPENCODE_ZIP}" . -x "*.DS_Store" -x "*/.DS_Store" -x "._*" -x "*/._*"
)
echo "==> ${OPENCODE_ZIP}"

# 自检: opencode 包关键文件必须在包里
opencode_listing="$(unzip -l "${OPENCODE_ZIP}")"
for entry in "README.md" "opencode.json.snippet" "AGENTS-aka.md" "skills/aka-code-graph/SKILL.md" "plugins/aka.js"; do
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

if [[ "${CLIENTS_ONLY}" -eq 1 ]]; then
  echo
  echo "==> 完成客户端/插件包。校验和请最后单独跑: scripts/package-release.sh --checksums-only"
  ls -lh "${PLUGIN_ZIP}" "${OPENCODE_ZIP}" "${CLIENTS_TAR}"
  exit 0
fi

# ---------- 4. release 二进制 ----------
if [[ -n "${TARGET}" ]]; then
  TRIPLE="${TARGET}"
else
  case "$(uname -m)" in
    arm64|aarch64) ARCH="aarch64" ;;
    x86_64)        ARCH="x86_64" ;;
    *) echo "error: 不支持的架构 $(uname -m)" >&2; exit 1 ;;
  esac
  case "$(uname -s)" in
    Darwin) TRIPLE="${ARCH}-apple-darwin" ;;
    Linux)  TRIPLE="${ARCH}-unknown-linux-gnu" ;;
    MINGW*|MSYS*|CYGWIN*) TRIPLE="${ARCH}-pc-windows-msvc" ;;
    *) echo "error: 不支持的系统 $(uname -s)" >&2; exit 1 ;;
  esac
fi

case "${TRIPLE}" in
  *-pc-windows-*)
    BIN_NAME="aka.exe"
    ARCHIVE_KIND="zip"
    ;;
  *)
    BIN_NAME="aka"
    ARCHIVE_KIND="tar.gz"
    ;;
esac

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
  build_args=(build --release -p aka-cli)
  if [[ -n "${TARGET}" ]]; then
    build_args+=(--target "${TRIPLE}")
  fi
  echo "==> cargo ${build_args[*]}"
  (cd "${REPO_ROOT}" && cargo "${build_args[@]}")
fi

if [[ -n "${TARGET}" ]]; then
  BIN="${REPO_ROOT}/target/${TRIPLE}/release/${BIN_NAME}"
else
  BIN="${REPO_ROOT}/target/release/${BIN_NAME}"
fi
[[ -x "${BIN}" ]] || { echo "error: 找不到 ${BIN}（先去掉 --skip-build 构建一次）" >&2; exit 1; }

BIN_ARCHIVE="${DIST_DIR}/aka-${VERSION}-${TRIPLE}.${ARCHIVE_KIND}"
rm -f "${BIN_ARCHIVE}"
STAGE="$(mktemp -d)"
trap 'rm -rf "${STAGE}"' EXIT
cp "${BIN}" "${STAGE}/${BIN_NAME}"
if [[ "${BIN_NAME}" = "aka" ]] && command -v strip >/dev/null 2>&1; then
  strip "${STAGE}/${BIN_NAME}"
fi
if [[ "${ARCHIVE_KIND}" = "zip" ]]; then
  (cd "${STAGE}" && zip -q -X -r "${BIN_ARCHIVE}" "${BIN_NAME}")
else
  COPYFILE_DISABLE=1 tar -czf "${BIN_ARCHIVE}" -C "${STAGE}" "${BIN_NAME}"
fi
echo "==> ${BIN_ARCHIVE}"

if [[ "${DESKTOP}" -eq 1 ]]; then
  package_desktop
fi

echo
echo "==> 完成。校验和请最后单独跑: scripts/package-release.sh --checksums-only"
ls_args=("${PLUGIN_ZIP}" "${OPENCODE_ZIP}" "${CLIENTS_TAR}" "${BIN_ARCHIVE}")
if [[ "${DESKTOP}" -eq 1 ]]; then
  ls_args+=("${DIST_DIR}/aka-desktop-${VERSION}-"*.app.zip)
fi
ls -lh "${ls_args[@]}"
