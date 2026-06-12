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
#   dist/aka-desktop-<ver>-<host-triple>.dmg
#                                             macOS Tauri GUI app DMG
#   dist/aka-desktop-<ver>-<host-triple>.app.zip
#                                             macOS Tauri GUI app（zip 内 AKA.app）
#   dist/aka-desktop-<ver>-x86_64-pc-windows-msvc-setup.exe
#                                             Windows Tauri GUI NSIS installer
#   dist/aka-desktop-<ver>-x86_64-pc-windows-msvc-portable.zip
#                                             Windows Tauri GUI portable exe + CBM engine
#
# 产物（--checksums-only 子命令，主流程不自动跑）:
#   dist/SHA256SUMS                         dist/ 下所有产物（含 dist/docker/*）的 sha256
#
# 注意: 不清空 dist/（dist/docker/ 是另一条流水线的产物位），只覆盖本脚本负责的文件。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${REPO_ROOT}/dist"
TAURI_DIR="${REPO_ROOT}/apps/desktop/src-tauri"
TAURI_RESOURCES_DIR="${TAURI_DIR}/resources"

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

if [[ -z "${VERSION}" ]]; then
  VERSION="$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/,"",$3); print $3; exit}' "${REPO_ROOT}/Cargo.toml")"
  [[ -n "${VERSION}" ]] || { echo "error: 无法从 Cargo.toml 读取 workspace 版本" >&2; exit 1; }
fi

mkdir -p "${DIST_DIR}"

platform_from_triple() {
  case "$1" in
    aarch64-apple-darwin) echo "darwin-arm64" ;;
    x86_64-apple-darwin) echo "darwin-x64" ;;
    x86_64-pc-windows-msvc) echo "win-x64" ;;
    *) echo "error: 不支持的桌面资源平台 $1" >&2; return 1 ;;
  esac
}

host_resource_platform() {
  case "$(uname -s):$(uname -m)" in
    Darwin:arm64|Darwin:aarch64) echo "darwin-arm64" ;;
    Darwin:x86_64) echo "darwin-x64" ;;
    MINGW*:x86_64|MSYS*:x86_64|CYGWIN*:x86_64) echo "win-x64" ;;
    *) echo "unknown" ;;
  esac
}

cbm_exe_for_platform() {
  case "$1" in
    darwin-arm64|darwin-x64) echo "codebase-memory-mcp" ;;
    win-x64) echo "codebase-memory-mcp.exe" ;;
    *) echo "error: 不支持的 CBM 平台 $1" >&2; return 1 ;;
  esac
}

env_var_for_platform() {
  tr '[:lower:]-' '[:upper:]_' <<< "$1"
}

first_existing_file() {
  local candidate
  for candidate in "$@"; do
    [[ -n "${candidate}" && -f "${candidate}" ]] || continue
    printf '%s\n' "${candidate}"
    return 0
  done
  return 1
}

find_cbm_binary() {
  local platform exe platform_env
  platform="$1"
  exe="$(cbm_exe_for_platform "${platform}")"
  platform_env="AKA_CBM_BIN_$(env_var_for_platform "${platform}")"

  first_existing_file \
    "${!platform_env:-}" \
    "${AKA_CBM_BIN:-}" \
    "${REPO_ROOT}/engine/${exe}" \
    "${REPO_ROOT}/engine/codebase-memory-mcp-src/build/c/${exe}" \
    "/tmp/codebase-memory-mcp-src/build/c/${exe}" \
    "$(command -v "${exe}" 2>/dev/null || true)"
}

copy_engine_resource() {
  local platform exe bin dst host_platform
  platform="$1"
  exe="$(cbm_exe_for_platform "${platform}")"
  dst="${TAURI_RESOURCES_DIR}/engine"

  bin="$(find_cbm_binary "${platform}" || true)"
  host_platform="$(host_resource_platform)"
  if [[ -z "${bin}" && "${platform}" = "${host_platform}" ]]; then
    echo "==> 本地未找到 CBM engine，运行 scripts/sync-engine.sh 构建 (${platform})"
    "${REPO_ROOT}/scripts/sync-engine.sh"
    bin="$(find_cbm_binary "${platform}" || true)"
  fi

  if [[ -z "${bin}" ]]; then
    echo "error: 找不到 ${platform} 的 codebase-memory-mcp 二进制。" >&2
    echo "       请先运行 scripts/sync-engine.sh，或设置 AKA_CBM_BIN / AKA_CBM_BIN_$(env_var_for_platform "${platform}")。" >&2
    return 1
  fi

  rm -rf "${dst}"
  mkdir -p "${dst}"
  cp "${bin}" "${dst}/${exe}"
  chmod +x "${dst}/${exe}" 2>/dev/null || true
  if [[ -f "${REPO_ROOT}/engine/ENGINE_SHA" ]]; then
    cp "${REPO_ROOT}/engine/ENGINE_SHA" "${dst}/ENGINE_SHA"
  fi
  echo "==> 内置 CBM engine: ${bin} -> ${dst}/${exe}"
}

prepare_desktop_resources() {
  local triple platform
  triple="$1"
  platform="$(platform_from_triple "${triple}")"
  echo "==> 准备桌面内置资源 (${platform})"
  copy_engine_resource "${platform}"
}

package_clients() {
  PLUGIN_ZIP="${DIST_DIR}/aka-claude-code-plugin-${VERSION}.zip"
  rm -f "${PLUGIN_ZIP}"
  (
    cd "${REPO_ROOT}/clients/claude-code"
    zip -r -X -q "${PLUGIN_ZIP}" . -x "*.DS_Store" -x "*/.DS_Store"
  )
  echo "==> ${PLUGIN_ZIP}"

  plugin_listing="$(unzip -l "${PLUGIN_ZIP}")"
  for entry in ".claude-plugin/plugin.json" ".mcp.json" "skills/aka-code-graph/SKILL.md"; do
    if ! grep -qF " ${entry}" <<< "${plugin_listing}"; then
      echo "error: 插件包缺少 ${entry}" >&2; exit 1
    fi
  done

  OPENCODE_ZIP="${DIST_DIR}/aka-opencode-plugin-${VERSION}.zip"
  rm -f "${OPENCODE_ZIP}"
  (
    cd "${REPO_ROOT}/clients/opencode"
    zip -r -X -q "${OPENCODE_ZIP}" . -x "*.DS_Store" -x "*/.DS_Store" -x "._*" -x "*/._*"
  )
  echo "==> ${OPENCODE_ZIP}"

  opencode_listing="$(unzip -l "${OPENCODE_ZIP}")"
  for entry in "README.md" "opencode.json.snippet" "AGENTS-aka.md" "skills/aka-code-graph/SKILL.md" "plugins/aka.js"; do
    if ! grep -qF " ${entry}" <<< "${opencode_listing}"; then
      echo "error: opencode 包缺少 ${entry}" >&2; exit 1
    fi
  done

  CLIENTS_TAR="${DIST_DIR}/aka-clients-${VERSION}.tar.gz"
  rm -f "${CLIENTS_TAR}"
  COPYFILE_DISABLE=1 tar -czf "${CLIENTS_TAR}" \
    --exclude '.DS_Store' \
    -C "${REPO_ROOT}" clients
  echo "==> ${CLIENTS_TAR}"
}

package_binary() {
  local triple bin_name archive_kind build_args bin bin_archive stage arch
  if [[ -n "${TARGET}" ]]; then
    triple="${TARGET}"
  else
    case "$(uname -m)" in
      arm64|aarch64) arch="aarch64" ;;
      x86_64)        arch="x86_64" ;;
      *) echo "error: 不支持的架构 $(uname -m)" >&2; exit 1 ;;
    esac
    case "$(uname -s)" in
      Darwin) TRIPLE="${arch}-apple-darwin" ;;
      Linux)  TRIPLE="${arch}-unknown-linux-gnu" ;;
      MINGW*|MSYS*|CYGWIN*) TRIPLE="${arch}-pc-windows-msvc" ;;
      *) echo "error: 不支持的系统 $(uname -s)" >&2; exit 1 ;;
    esac
    triple="${TRIPLE}"
  fi

  case "${triple}" in
    *-pc-windows-*) bin_name="aka.exe"; archive_kind="zip" ;;
    *) bin_name="aka"; archive_kind="tar.gz" ;;
  esac

  if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    build_args=(build --release -p aka-cli)
    if [[ -n "${TARGET}" ]]; then
      build_args+=(--target "${triple}")
    fi
    if [[ "${triple}" = "x86_64-pc-windows-msvc" ]] && [[ "$(uname -s)" = "Darwin" ]] && command -v cargo-xwin >/dev/null 2>&1; then
      echo "==> cargo xwin ${build_args[*]}"
      (cd "${REPO_ROOT}" && cargo xwin "${build_args[@]}")
    else
      echo "==> cargo ${build_args[*]}"
      (cd "${REPO_ROOT}" && cargo "${build_args[@]}")
    fi
  fi

  if [[ -n "${TARGET}" ]]; then
    bin="${REPO_ROOT}/target/${triple}/release/${bin_name}"
  else
    bin="${REPO_ROOT}/target/release/${bin_name}"
  fi
  [[ -x "${bin}" ]] || { echo "error: 找不到 ${bin}（先去掉 --skip-build 构建一次）" >&2; exit 1; }

  bin_archive="${DIST_DIR}/aka-${VERSION}-${triple}.${archive_kind}"
  rm -f "${bin_archive}"
  stage="$(mktemp -d)"
  trap 'rm -rf "${stage}"' EXIT
  cp "${bin}" "${stage}/${bin_name}"
  if [[ "${bin_name}" = "aka" ]] && command -v strip >/dev/null 2>&1; then
    strip "${stage}/${bin_name}"
  fi
  if [[ "${archive_kind}" = "zip" ]]; then
    (cd "${stage}" && zip -q -X -r "${bin_archive}" "${bin_name}")
  else
    COPYFILE_DISABLE=1 tar -czf "${bin_archive}" -C "${stage}" "${bin_name}"
  fi
  echo "==> ${bin_archive}"
  BIN_ARCHIVE="${bin_archive}"
}

package_desktop() {
  local host_os host_arch desktop_triple app_path desktop_dmg desktop_zip
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
        prepare_desktop_resources "${desktop_triple}"
        echo "==> npm run tauri -- build --bundles app --ci --no-sign"
        (cd "${REPO_ROOT}/apps/desktop" && npm run tauri -- build --bundles app --ci --no-sign)
      fi

      app_path="${REPO_ROOT}/apps/desktop/src-tauri/target/release/bundle/macos/AKA.app"
      [[ -d "${app_path}" ]] || { echo "error: 找不到 ${app_path}（先去掉 --skip-build 构建一次）" >&2; return 1; }

      desktop_dmg="${DIST_DIR}/aka-desktop-${VERSION}-${desktop_triple}.dmg"
      rm -f "${desktop_dmg}"
      hdiutil create -volname "AKA" -srcfolder "${app_path}" -ov -format UDZO "${desktop_dmg}"
      echo "==> ${desktop_dmg}"

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
  local win_triple exe_path setup_src setup_exe portable_zip stage engine_src
  win_triple="x86_64-pc-windows-msvc"

  if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    prepare_desktop_resources "${win_triple}"
    rm -rf "${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/engine"
    local tauri_args=(build --target "${win_triple}" --bundles nsis --ci)
    if command -v cargo-xwin >/dev/null 2>&1 && [[ "$(uname -s)" = "Darwin" ]]; then
      tauri_args=(build --runner cargo-xwin --target "${win_triple}" --bundles nsis --ci)
    fi
    echo "==> npm run tauri -- ${tauri_args[*]}"
    (cd "${REPO_ROOT}/apps/desktop" && npm run tauri -- "${tauri_args[@]}")
  fi

  exe_path="${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/AKA.exe"
  if [[ ! -f "${exe_path}" ]]; then
    exe_path="${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/aka-desktop.exe"
  fi
  [[ -f "${exe_path}" ]] || { echo "error: 找不到 Windows GUI exe（先去掉 --skip-build 构建一次）" >&2; return 1; }

  setup_src="$(find "${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/bundle/nsis" -maxdepth 1 -type f -name '*setup.exe' | sort | tail -n 1 || true)"
  [[ -f "${setup_src}" ]] || { echo "error: 找不到 Windows NSIS 安装器（先去掉 --skip-build 构建一次）" >&2; return 1; }

  setup_exe="${DIST_DIR}/aka-desktop-${VERSION}-${win_triple}-setup.exe"
  rm -f "${setup_exe}"
  cp "${setup_src}" "${setup_exe}"
  echo "==> ${setup_exe}"

  portable_zip="${DIST_DIR}/aka-desktop-${VERSION}-${win_triple}-portable.zip"
  rm -f "${portable_zip}"
  stage="$(mktemp -d)"
  cp "${exe_path}" "${stage}/AKA.exe"
  engine_src="${TAURI_RESOURCES_DIR}/engine"
  [[ -f "${engine_src}/codebase-memory-mcp.exe" ]] || { echo "error: 找不到 Windows portable 所需 CBM engine: ${engine_src}/codebase-memory-mcp.exe" >&2; return 1; }
  cp -R "${engine_src}" "${stage}/engine"
  (cd "${stage}" && zip -q -X -r "${portable_zip}" AKA.exe engine)
  rm -rf "${stage}"
  echo "==> ${portable_zip}"
}

if [[ "${CHECKSUMS_ONLY}" -eq 1 ]]; then
  cd "${DIST_DIR}"
  files=()
  while IFS= read -r f; do
    files+=("${f#./}")
  done < <(find . -maxdepth 1 -type f ! -name SHA256SUMS ! -name .DS_Store | sort)
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
  ls -lh "${DIST_DIR}/aka-desktop-${VERSION}-"*.dmg "${DIST_DIR}/aka-desktop-${VERSION}-"*.app.zip
  exit 0
fi

if [[ "${DESKTOP_WINDOWS}" -eq 1 ]]; then
  package_windows_desktop
  echo
  echo "==> 完成 Windows GUI 包。校验和请最后单独跑: scripts/package-release.sh --checksums-only"
  ls -lh "${DIST_DIR}/aka-desktop-${VERSION}-x86_64-pc-windows-msvc-"*
  exit 0
fi

package_clients
if [[ "${CLIENTS_ONLY}" -eq 1 ]]; then
  echo
  echo "==> 完成客户端/插件包。校验和请最后单独跑: scripts/package-release.sh --checksums-only"
  ls -lh "${PLUGIN_ZIP}" "${OPENCODE_ZIP}" "${CLIENTS_TAR}"
  exit 0
fi

package_binary

if [[ "${DESKTOP}" -eq 1 ]]; then
  package_desktop
fi

echo
echo "==> 完成。校验和请最后单独跑: scripts/package-release.sh --checksums-only"
ls_args=("${PLUGIN_ZIP}" "${OPENCODE_ZIP}" "${CLIENTS_TAR}" "${BIN_ARCHIVE}")
if [[ "${DESKTOP}" -eq 1 ]]; then
  ls_args+=("${DIST_DIR}/aka-desktop-${VERSION}-"*.dmg "${DIST_DIR}/aka-desktop-${VERSION}-"*.app.zip)
fi
ls -lh "${ls_args[@]}"
