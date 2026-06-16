#!/usr/bin/env bash
# package-release.sh — 打包 v<ver> 发布资产到 dist/
#
# 用法:
#   scripts/package-release.sh [--version 0.1.0] [--skip-build] [--clients-only] [--desktop]
#   scripts/package-release.sh --desktop-only [--version 0.1.0] [--skip-build]
#   scripts/package-release.sh --desktop-windows [--version 0.1.0] [--skip-build]
#   scripts/package-release.sh --checksums-only [--version 0.1.0]
#
# 产物（主流程）:
#   dist/aka-claude-code-plugin-<ver>.zip   clients/claude-code/ 插件包
#   dist/aka-opencode-plugin-<ver>.zip      clients/opencode/ 插件包（MCP 片段 + skill + 本地 plugin）
#   dist/aka-clients-<ver>.tar.gz           整个 clients/ 目录
#   dist/aka-desktop-<ver>-<host-triple>.dmg
#                                             macOS Tauri GUI app DMG
#   dist/aka-desktop-<ver>-<host-triple>.app.zip
#                                             macOS Tauri GUI app（zip 内 AKA.app）
#   dist/aka-desktop-<ver>-<host-triple>.app.tar.gz[.sig]
#                                             macOS Tauri updater 包（存在签名密钥时生成）
#   dist/aka-desktop-<ver>-macos-open.sh      macOS 无公证包打开助手（去 quarantine）
#   dist/aka-desktop-<ver>-x86_64-pc-windows-msvc-setup.exe
#                                             Windows Tauri GUI NSIS installer
#   dist/aka-desktop-<ver>-x86_64-pc-windows-msvc-setup.exe.sig
#                                             Windows Tauri updater 签名（存在签名密钥时生成）
#   dist/aka-desktop-<ver>-x86_64-pc-windows-msvc-portable.zip
#                                             Windows Tauri GUI portable exe（engine 内嵌）
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
      echo "error: 独立 CLI/server 发布包已移除，不再支持 --target。" >&2
      echo "       请打包桌面端（AKA 可执行文件已支持 CLI/MCP 子命令）或使用 Docker 镜像。" >&2
      exit 1 ;;
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

create_zip_archive() {
  local archive exclude_patterns=()
  archive="$1"
  shift
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --exclude)
        [[ $# -ge 2 ]] || { echo "error: --exclude 需要参数" >&2; return 1; }
        exclude_patterns+=("$2")
        shift 2
        ;;
      --)
        shift
        break
        ;;
      *)
        break
        ;;
    esac
  done
  if command -v zip >/dev/null 2>&1; then
    local zip_args=()
    if [[ ${#exclude_patterns[@]} -gt 0 ]]; then
      local pattern
      for pattern in "${exclude_patterns[@]}"; do
        zip_args+=(-x "${pattern}")
      done
    fi
    if [[ ${#zip_args[@]} -gt 0 ]]; then
      zip -q -X -r "${archive}" "$@" "${zip_args[@]}"
    else
      zip -q -X -r "${archive}" "$@"
    fi
    return
  fi
  if command -v 7z >/dev/null 2>&1; then
    local seven_zip_args=()
    if [[ ${#exclude_patterns[@]} -gt 0 ]]; then
      local pattern
      for pattern in "${exclude_patterns[@]}"; do
        seven_zip_args+=("-xr!${pattern}")
      done
    fi
    if [[ ${#seven_zip_args[@]} -gt 0 ]]; then
      7z a -tzip "${archive}" "$@" "${seven_zip_args[@]}" >/dev/null
    else
      7z a -tzip "${archive}" "$@" >/dev/null
    fi
    return
  fi
  if command -v powershell.exe >/dev/null 2>&1 || command -v powershell >/dev/null 2>&1 || command -v pwsh >/dev/null 2>&1; then
    local ps zip_path item args=()
    ps="$(command -v powershell.exe 2>/dev/null || command -v powershell 2>/dev/null || command -v pwsh)"
    zip_path="${archive}"
    if command -v cygpath >/dev/null 2>&1; then
      zip_path="$(cygpath -w "${archive}")"
      for item in "$@"; do
        args+=("$(cygpath -w "${item}")")
      done
    else
      args=("$@")
    fi
    AKA_ZIP_DEST="${zip_path}" "${ps}" -NoProfile -Command \
      "Compress-Archive -Path @(\$args) -DestinationPath \$env:AKA_ZIP_DEST -Force" \
      -- "${args[@]}"
    return
  fi
  echo "error: 找不到可用的 zip 工具（需要 zip、7z 或 PowerShell Compress-Archive）" >&2
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
  assert_engine_resource_dir "${platform}" "${dst}"
}

assert_engine_file_nonempty() {
  local engine_bin="$1"
  [[ -f "${engine_bin}" ]] || { echo "error: engine 文件不存在: ${engine_bin}" >&2; return 1; }
  [[ -s "${engine_bin}" ]] || { echo "error: engine 文件为空: ${engine_bin}" >&2; return 1; }
}

find_engine_resource_bin() {
  local platform dir exe
  platform="$1"
  dir="$2"
  exe="$(cbm_exe_for_platform "${platform}")"
  first_existing_file \
    "${dir}/${exe}" \
    "${dir}/bin/${exe}" \
    "${dir}/build/c/${exe}"
}

assert_engine_resource_dir() {
  local platform dir engine_bin
  platform="$1"
  dir="$2"
  engine_bin="$(find_engine_resource_bin "${platform}" "${dir}" || true)"
  if [[ -z "${engine_bin}" ]]; then
    echo "error: 桌面 engine 资源缺少 native CBM 二进制: ${dir}" >&2
    echo "       需要 $(cbm_exe_for_platform "${platform}")，不要打入旧的 JS/node engine 目录。" >&2
    return 1
  fi
  if [[ "${platform}" != "win-x64" && ! -x "${engine_bin}" ]]; then
    echo "error: 桌面 engine 资源不可执行: ${engine_bin}" >&2
    return 1
  fi
  assert_engine_file_nonempty "${engine_bin}"
  echo "==> 校验桌面 engine 资源: ${engine_bin}"
}

assert_app_bundle_engine() {
  local app_path platform resources dir engine_bin
  app_path="$1"
  platform="$2"
  resources="${app_path}/Contents/Resources"
  [[ -d "${resources}" ]] || { echo "error: app 缺少 Resources 目录: ${resources}" >&2; return 1; }
  for dir in "${resources}/engine" "${resources}/resources/engine"; do
    engine_bin="$(find_engine_resource_bin "${platform}" "${dir}" || true)"
    if [[ -n "${engine_bin}" ]]; then
      if [[ "${platform}" != "win-x64" && ! -x "${engine_bin}" ]]; then
        echo "error: app 包内 engine 不可执行: ${engine_bin}" >&2
        return 1
      fi
      echo "==> 校验 app 包内 engine: ${engine_bin}"
      return 0
    fi
  done
  echo "error: app 包内缺少 native CBM engine: ${resources}/{engine,resources/engine}" >&2
  return 1
}

assert_zip_has_engine() {
  local zip_path platform prefix exe listing base entry
  zip_path="$1"
  platform="$2"
  prefix="$3"
  exe="$(cbm_exe_for_platform "${platform}")"
  [[ -f "${zip_path}" ]] || { echo "error: 找不到 zip: ${zip_path}" >&2; return 1; }
  listing="$(unzip -Z1 "${zip_path}")"
  for base in "engine" "resources/engine" "engine/bin" "resources/engine/bin" "engine/build/c" "resources/engine/build/c"; do
    entry="${prefix:+${prefix}/}${base}/${exe}"
    if grep -qxF "${entry}" <<< "${listing}"; then
      echo "==> 校验 zip 包内 engine: ${entry}"
      return 0
    fi
  done
  echo "error: zip 包内缺少 native CBM engine: ${zip_path}" >&2
  return 1
}

prepare_desktop_resources() {
  local triple platform
  triple="$1"
  platform="$(platform_from_triple "${triple}")"
  echo "==> 准备桌面内置资源 (${platform})"
  copy_engine_resource "${platform}"
}

macos_notarization_credentials_present() {
  if [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    return 0
  fi
  if [[ -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; then
    return 0
  fi
  return 1
}

macos_signing_credentials_present() {
  [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]] || [[ -n "${APPLE_CERTIFICATE:-}" && -n "${APPLE_CERTIFICATE_PASSWORD:-}" ]]
}

macos_signed_release_requested() {
  case "${AKA_MACOS_SIGN:-auto}" in
    1|true|TRUE|yes|YES|required) return 0 ;;
    0|false|FALSE|no|NO|never) return 1 ;;
    auto)
      macos_signing_credentials_present && macos_notarization_credentials_present
      return
      ;;
    *)
      echo "error: AKA_MACOS_SIGN 只能是 auto/1/0/true/false/required/never，当前为 ${AKA_MACOS_SIGN}" >&2
      return 1
      ;;
  esac
}

require_macos_signing_env() {
  if ! macos_signing_credentials_present; then
    echo "error: macOS release 要求签名，但缺少签名证书环境变量。" >&2
    echo "       需要 APPLE_CERTIFICATE + APPLE_CERTIFICATE_PASSWORD，或预装证书并设置 APPLE_SIGNING_IDENTITY。" >&2
    return 1
  fi
  if ! macos_notarization_credentials_present; then
    echo "error: macOS release 要求公证，但缺少 Apple notarization 环境变量。" >&2
    echo "       需要 APPLE_ID + APPLE_PASSWORD + APPLE_TEAM_ID，或 APPLE_API_ISSUER + APPLE_API_KEY + APPLE_API_KEY_PATH。" >&2
    return 1
  fi
}

codesign_ad_hoc_app() {
  local app_path
  app_path="$1"
  echo "==> 本地 ad-hoc codesign: ${app_path}"
  codesign --force --deep --sign - "${app_path}"
}

verify_macos_bundle() {
  local app_path dmg_path signed_release
  app_path="$1"
  dmg_path="$2"
  signed_release="$3"

  codesign --verify --deep --strict --verbose=4 "${app_path}"
  hdiutil verify "${dmg_path}"

  if [[ "${signed_release}" -eq 1 ]]; then
    spctl --assess --type execute --verbose=4 "${app_path}"
    spctl --assess --type open --context context:primary-signature --verbose=4 "${dmg_path}"
  else
    echo "==> 本地包为 ad-hoc/未公证构建；hdiutil 与 codesign 校验通过，但 Gatekeeper 仍会拒绝公网下载的产物。"
  fi
}

create_macos_dmg() {
  local app_path dmg_path attempt
  app_path="$1"
  dmg_path="$2"

  for attempt in 1 2 3; do
    if hdiutil create -volname "AKA" -srcfolder "${app_path}" -ov -format UDZO "${dmg_path}"; then
      return 0
    fi
    echo "warning: hdiutil create failed (attempt ${attempt}/3); retrying after cleanup..." >&2
    hdiutil detach -quiet "/Volumes/AKA" 2>/dev/null || true
    sleep $((attempt * 3))
  done

  echo "error: hdiutil create failed after 3 attempts" >&2
  return 1
}

find_tauri_dmg() {
  local dmg_dir
  dmg_dir="${REPO_ROOT}/apps/desktop/src-tauri/target/release/bundle/dmg"
  find "${dmg_dir}" -maxdepth 1 -type f -name '*.dmg' | sort | tail -n 1
}

find_tauri_updater_archive() {
  local archive_dir
  archive_dir="$1"
  [[ -d "${archive_dir}" ]] || return 0
  find "${archive_dir}" -maxdepth 1 -type f -name '*.app.tar.gz' | sort | tail -n 1
}

clear_tauri_updater_archives() {
  local archive_dir
  archive_dir="$1"
  [[ -d "${archive_dir}" ]] || return 0
  find "${archive_dir}" -maxdepth 1 -type f \
    \( -name '*.app.tar.gz' -o -name '*.app.tar.gz.sig' -o -name '*.app.tar.gz.signature' \) \
    -delete
}

has_signature_sidecar() {
  local src
  src="$1"
  [[ -f "${src}.sig" ]] || [[ -f "${src}.signature" ]]
}

copy_signature_sidecars() {
  local src dst sidecar
  src="$1"
  dst="$2"
  for suffix in sig signature; do
    sidecar="${src}.${suffix}"
    if [[ -f "${sidecar}" ]]; then
      cp "${sidecar}" "${dst}.${suffix}"
      echo "==> ${dst}.${suffix}"
    fi
  done
}

tauri_updater_signing_configured() {
  case "${AKA_TAURI_UPDATER:-auto}" in
    0|false|FALSE|no|NO|never) return 1 ;;
  esac
  [[ "${AKA_TAURI_UPDATER:-auto}" != "never" ]] &&
    [[ -n "${TAURI_UPDATER_PUBKEY:-}" ]] &&
    [[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" || -n "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]]
}

tauri_updater_config_args() {
  local mode endpoint install_mode config
  mode="${AKA_TAURI_UPDATER:-auto}"
  case "${mode}" in
    1|true|TRUE|yes|YES|required) mode="required" ;;
    0|false|FALSE|no|NO|never) mode="never" ;;
    auto|"") mode="auto" ;;
    *)
      echo "error: AKA_TAURI_UPDATER 只能是 auto/required/never/true/false，当前为 ${AKA_TAURI_UPDATER}" >&2
      return 1
      ;;
  esac

  if [[ "${mode}" = "never" ]]; then
    return 0
  fi

  if [[ -z "${TAURI_UPDATER_PUBKEY:-}" || ( -z "${TAURI_SIGNING_PRIVATE_KEY:-}" && -z "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ) ]]; then
    if [[ "${mode}" = "required" ]]; then
      echo "error: 自动更新 release 需要 TAURI_UPDATER_PUBKEY 和 TAURI_SIGNING_PRIVATE_KEY/TAURI_SIGNING_PRIVATE_KEY_PATH。" >&2
      return 1
    fi
    return 0
  fi

  endpoint="${AKA_UPDATER_ENDPOINT:-https://github.com/caork/aka/releases/latest/download/latest.json}"
  install_mode="${AKA_UPDATER_WINDOWS_INSTALL_MODE:-passive}"
  config="$(TAURI_UPDATER_PUBKEY="${TAURI_UPDATER_PUBKEY}" \
    AKA_UPDATER_ENDPOINT="${endpoint}" \
    AKA_UPDATER_WINDOWS_INSTALL_MODE="${install_mode}" \
    node <<'NODE'
const pubkey = process.env.TAURI_UPDATER_PUBKEY;
const endpoint = process.env.AKA_UPDATER_ENDPOINT;
const installMode = process.env.AKA_UPDATER_WINDOWS_INSTALL_MODE;
process.stdout.write(JSON.stringify({
  bundle: {
    createUpdaterArtifacts: true
  },
  plugins: {
    updater: {
      pubkey,
      endpoints: [endpoint],
      windows: {
        installMode
      }
    }
  }
}));
NODE
  )"
  echo "==> Tauri updater artifacts enabled: ${endpoint}" >&2
  printf '%s\n' --config "${config}"
}

package_clients() {
  PLUGIN_ZIP="${DIST_DIR}/aka-claude-code-plugin-${VERSION}.zip"
  rm -f "${PLUGIN_ZIP}"
  (
    cd "${REPO_ROOT}/clients/claude-code"
    create_zip_archive "${PLUGIN_ZIP}" \
      --exclude "*.DS_Store" \
      --exclude "*/.DS_Store" \
      -- .
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
    create_zip_archive "${OPENCODE_ZIP}" \
      --exclude "*.DS_Store" \
      --exclude "*/.DS_Store" \
      --exclude "._*" \
      --exclude "*/._*" \
      -- .
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

package_desktop() {
  local host_os host_arch desktop_triple desktop_platform app_path desktop_dmg desktop_zip desktop_updater helper_script signed_release tauri_dmg tauri_updater tauri_macos_bundle_dir
  host_os="$(uname -s)"
  host_arch="$(uname -m)"

  case "${host_os}" in
    Darwin)
      case "${host_arch}" in
        arm64|aarch64) desktop_triple="aarch64-apple-darwin" ;;
        x86_64)        desktop_triple="x86_64-apple-darwin" ;;
        *) echo "error: 不支持的 macOS 架构 ${host_arch}" >&2; return 1 ;;
      esac
      desktop_platform="$(platform_from_triple "${desktop_triple}")"
      signed_release=0
      if macos_signed_release_requested; then
        require_macos_signing_env
        signed_release=1
      fi

      if [[ "${SKIP_BUILD}" -eq 0 ]]; then
        prepare_desktop_resources "${desktop_triple}"
        tauri_macos_bundle_dir="${REPO_ROOT}/apps/desktop/src-tauri/target/release/bundle/macos"
        clear_tauri_updater_archives "${tauri_macos_bundle_dir}"
        if [[ "${signed_release}" -eq 1 ]]; then
          local tauri_args=(build --bundles app,dmg --ci)
          local updater_config_args
          local native_updater_env=()
          updater_config_args="$(tauri_updater_config_args)" || return 1
          if [[ -n "${updater_config_args}" ]]; then
            native_updater_env=(AKA_ENABLE_NATIVE_UPDATER=1)
            while IFS= read -r arg; do
              tauri_args+=("${arg}")
            done <<< "${updater_config_args}"
          fi
          echo "==> npm run tauri -- ${tauri_args[*]}"
          if [[ "${#native_updater_env[@]}" -gt 0 ]]; then
            (cd "${REPO_ROOT}/apps/desktop" && env "${native_updater_env[@]}" npm run tauri -- "${tauri_args[@]}")
          else
            (cd "${REPO_ROOT}/apps/desktop" && npm run tauri -- "${tauri_args[@]}")
          fi
        else
          local tauri_args=(build --bundles app --ci --no-sign)
          local updater_config_args
          local native_updater_env=()
          updater_config_args="$(tauri_updater_config_args)" || return 1
          if [[ -n "${updater_config_args}" ]]; then
            native_updater_env=(AKA_ENABLE_NATIVE_UPDATER=1)
            while IFS= read -r arg; do
              tauri_args+=("${arg}")
            done <<< "${updater_config_args}"
          fi
          echo "==> npm run tauri -- ${tauri_args[*]}"
          if [[ "${#native_updater_env[@]}" -gt 0 ]]; then
            (cd "${REPO_ROOT}/apps/desktop" && env "${native_updater_env[@]}" npm run tauri -- "${tauri_args[@]}")
          else
            (cd "${REPO_ROOT}/apps/desktop" && npm run tauri -- "${tauri_args[@]}")
          fi
        fi
      fi

      app_path="${REPO_ROOT}/apps/desktop/src-tauri/target/release/bundle/macos/AKA.app"
      [[ -d "${app_path}" ]] || { echo "error: 找不到 ${app_path}（先去掉 --skip-build 构建一次）" >&2; return 1; }
      assert_app_bundle_engine "${app_path}" "${desktop_platform}"
      if [[ "${signed_release}" -eq 0 ]]; then
        codesign_ad_hoc_app "${app_path}"
      fi

      desktop_dmg="${DIST_DIR}/aka-desktop-${VERSION}-${desktop_triple}.dmg"
      rm -f "${desktop_dmg}"
      if [[ "${signed_release}" -eq 1 ]]; then
        tauri_dmg="$(find_tauri_dmg)"
        [[ -f "${tauri_dmg}" ]] || { echo "error: Tauri 未产出 dmg" >&2; return 1; }
        cp "${tauri_dmg}" "${desktop_dmg}"
      else
        create_macos_dmg "${app_path}" "${desktop_dmg}"
      fi
      verify_macos_bundle "${app_path}" "${desktop_dmg}" "${signed_release}"
      echo "==> ${desktop_dmg}"

      desktop_zip="${DIST_DIR}/aka-desktop-${VERSION}-${desktop_triple}.app.zip"
      rm -f "${desktop_zip}"
      COPYFILE_DISABLE=1 ditto -c -k --norsrc --keepParent "${app_path}" "${desktop_zip}"
      assert_zip_has_engine "${desktop_zip}" "${desktop_platform}" "AKA.app/Contents/Resources"
      echo "==> ${desktop_zip}"

      tauri_macos_bundle_dir="${REPO_ROOT}/apps/desktop/src-tauri/target/release/bundle/macos"
      tauri_updater="$(find_tauri_updater_archive "${tauri_macos_bundle_dir}" || true)"
      if [[ -n "${tauri_updater}" ]]; then
        if has_signature_sidecar "${tauri_updater}"; then
          desktop_updater="${DIST_DIR}/aka-desktop-${VERSION}-${desktop_triple}.app.tar.gz"
          rm -f "${desktop_updater}" "${desktop_updater}.sig" "${desktop_updater}.signature"
          cp "${tauri_updater}" "${desktop_updater}"
          copy_signature_sidecars "${tauri_updater}" "${desktop_updater}"
          echo "==> ${desktop_updater}"
        elif tauri_updater_signing_configured; then
          echo "error: Tauri updater archive 缺少签名: ${tauri_updater}.sig" >&2
          return 1
        else
          echo "warning: 忽略未签名的 Tauri updater 包: ${tauri_updater}" >&2
        fi
      elif tauri_updater_signing_configured; then
        echo "error: Tauri 未产出 macOS updater 包: ${tauri_macos_bundle_dir}/*.app.tar.gz" >&2
        return 1
      fi

      helper_script="${DIST_DIR}/aka-desktop-${VERSION}-macos-open.sh"
      cp "${REPO_ROOT}/scripts/open-macos-dmg.sh" "${helper_script}"
      chmod +x "${helper_script}"
      echo "==> ${helper_script}"
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
    local embed_dir
    prepare_desktop_resources "${win_triple}"
    embed_dir="${TAURI_DIR}/embedded-engine"
    rm -rf "${embed_dir}"
    mkdir -p "${embed_dir}"
    assert_engine_file_nonempty "${TAURI_RESOURCES_DIR}/engine/codebase-memory-mcp.exe"
    cp "${TAURI_RESOURCES_DIR}/engine/codebase-memory-mcp.exe" "${embed_dir}/codebase-memory-mcp.exe"
    assert_engine_file_nonempty "${embed_dir}/codebase-memory-mcp.exe"
    echo "==> 内嵌 Windows CBM engine: ${TAURI_RESOURCES_DIR}/engine/codebase-memory-mcp.exe -> ${embed_dir}/codebase-memory-mcp.exe"
    rm -rf "${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/engine"
    local tauri_args=(build --target "${win_triple}" --bundles nsis --ci)
    if command -v cargo-xwin >/dev/null 2>&1 && [[ "$(uname -s)" = "Darwin" ]]; then
      tauri_args=(build --runner cargo-xwin --target "${win_triple}" --bundles nsis --ci)
    fi
    local updater_config_args
    local native_updater_env=()
    updater_config_args="$(tauri_updater_config_args)" || return 1
    if [[ -n "${updater_config_args}" ]]; then
      native_updater_env=(AKA_ENABLE_NATIVE_UPDATER=1)
      while IFS= read -r arg; do
        tauri_args+=("${arg}")
      done <<< "${updater_config_args}"
    fi
    echo "==> npm run tauri -- ${tauri_args[*]}"
    if [[ "${#native_updater_env[@]}" -gt 0 ]]; then
      (cd "${REPO_ROOT}/apps/desktop" && env "${native_updater_env[@]}" npm run tauri -- "${tauri_args[@]}")
    else
      (cd "${REPO_ROOT}/apps/desktop" && npm run tauri -- "${tauri_args[@]}")
    fi
  fi

  exe_path="${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/AKA.exe"
  if [[ ! -f "${exe_path}" ]]; then
    exe_path="${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/aka-desktop.exe"
  fi
  [[ -f "${exe_path}" ]] || { echo "error: 找不到 Windows GUI exe（先去掉 --skip-build 构建一次）" >&2; return 1; }

  setup_src="$(find "${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/bundle/nsis" -maxdepth 1 -type f -name "*${VERSION}*setup.exe" | sort | tail -n 1 || true)"
  if [[ -z "${setup_src}" ]]; then
    setup_src="$(find "${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/bundle/nsis" -maxdepth 1 -type f -name '*setup.exe' | sort | tail -n 1 || true)"
  fi
  [[ -f "${setup_src}" ]] || { echo "error: 找不到 Windows NSIS 安装器（先去掉 --skip-build 构建一次）" >&2; return 1; }

  setup_exe="${DIST_DIR}/aka-desktop-${VERSION}-${win_triple}-setup.exe"
  rm -f "${setup_exe}"
  cp "${setup_src}" "${setup_exe}"
  copy_signature_sidecars "${setup_src}" "${setup_exe}"
  if tauri_updater_signing_configured; then
    [[ -f "${setup_exe}.sig" || -f "${setup_exe}.signature" ]] || {
      echo "error: Tauri Windows updater installer 缺少签名: ${setup_src}.sig" >&2
      return 1
    }
  fi
  echo "==> ${setup_exe}"

  portable_zip="${DIST_DIR}/aka-desktop-${VERSION}-${win_triple}-portable.zip"
  rm -f "${portable_zip}"
  stage="$(mktemp -d)"
  cp "${exe_path}" "${stage}/AKA.exe"
  mkdir -p "${stage}/engine"
  assert_engine_file_nonempty "${TAURI_RESOURCES_DIR}/engine/codebase-memory-mcp.exe"
  cp "${TAURI_RESOURCES_DIR}/engine/codebase-memory-mcp.exe" "${stage}/engine/codebase-memory-mcp.exe"
  assert_engine_file_nonempty "${stage}/engine/codebase-memory-mcp.exe"
  (cd "${stage}" && create_zip_archive "${portable_zip}" AKA.exe engine/codebase-memory-mcp.exe)
  rm -rf "${stage}"
  assert_zip_has_engine "${portable_zip}" "win-x64" ""
  echo "==> ${portable_zip}"
}

if [[ "${CHECKSUMS_ONLY}" -eq 1 ]]; then
  cd "${DIST_DIR}"
  files=()
  while IFS= read -r f; do
    files+=("${f#./}")
  done < <(find . -maxdepth 1 -type f ! -name SHA256SUMS ! -name latest.json ! -name .DS_Store | sort)
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

if [[ "${DESKTOP}" -eq 1 ]]; then
  package_desktop
fi

echo
echo "==> 完成。校验和请最后单独跑: scripts/package-release.sh --checksums-only"
ls_args=("${PLUGIN_ZIP}" "${OPENCODE_ZIP}" "${CLIENTS_TAR}")
if [[ "${DESKTOP}" -eq 1 ]]; then
  ls_args+=("${DIST_DIR}/aka-desktop-${VERSION}-"*.dmg "${DIST_DIR}/aka-desktop-${VERSION}-"*.app.zip)
fi
ls -lh "${ls_args[@]}"
