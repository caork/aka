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
TAURI_DIR="${REPO_ROOT}/apps/desktop/src-tauri"
TAURI_RESOURCES_DIR="${TAURI_DIR}/resources"
NODE_VERSION="${AKA_NODE_VERSION:-v24.16.0}"

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

platform_from_triple() {
  case "$1" in
    aarch64-apple-darwin) echo "darwin-arm64" ;;
    x86_64-apple-darwin) echo "darwin-x64" ;;
    x86_64-pc-windows-msvc) echo "win-x64" ;;
    *) echo "error: 不支持的桌面资源平台 $1" >&2; return 1 ;;
  esac
}

native_platform_from_resource_platform() {
  case "$1" in
    darwin-arm64) echo "darwin-arm64" ;;
    darwin-x64) echo "darwin-x64" ;;
    win-x64) echo "win32-x64" ;;
    *) echo "error: 不支持的 native 平台 $1" >&2; return 1 ;;
  esac
}

onnx_platform_from_resource_platform() {
  case "$1" in
    darwin-arm64) echo "darwin/arm64" ;;
    darwin-x64) echo "darwin/x64" ;;
    win-x64) echo "win32/x64" ;;
    *) echo "error: 不支持的 ONNX 平台 $1" >&2; return 1 ;;
  esac
}

slim_node_resource() {
  local platform dst
  platform="$1"
  dst="${TAURI_RESOURCES_DIR}/node"

  case "${platform}" in
    darwin-*)
      [[ -x "${dst}/bin/node" ]] || { echo "error: Node runtime 缺少 bin/node" >&2; return 1; }
      find "${dst}/bin" -mindepth 1 -maxdepth 1 ! -name node -exec rm -rf {} +
      rm -rf "${dst}/include" "${dst}/lib" "${dst}/share"
      ;;
    win-x64)
      [[ -f "${dst}/node.exe" ]] || { echo "error: Node runtime 缺少 node.exe" >&2; return 1; }
      find "${dst}" -mindepth 1 -maxdepth 1 \
        ! -name node.exe \
        ! -name LICENSE \
        -exec rm -rf {} +
      ;;
  esac
}

slim_tree_sitter_package() {
  local pkg_dir native_platform package_name
  pkg_dir="$1"
  native_platform="$2"
  [[ -d "${pkg_dir}" ]] || return 0
  package_name="$(basename "${pkg_dir}")"

  if [[ -d "${pkg_dir}/prebuilds" ]]; then
    find "${pkg_dir}/prebuilds" -mindepth 1 -maxdepth 1 -type d ! -name "${native_platform}" -exec rm -rf {} +
  fi

  if [[ "${package_name}" = "tree-sitter" ]]; then
    # The core runtime package resolves through package.json main=index.js.
    find "${pkg_dir}" -mindepth 1 -maxdepth 1 \
      ! -name package.json \
      ! -name index.js \
      ! -name prebuilds \
      -exec rm -rf {} +
    return 0
  fi

  # Language packages load bindings/node/index.js, package.json, the matching
  # prebuild, and optionally src/node-types.json. Source C/C++ files are bulk.
  find "${pkg_dir}" -mindepth 1 -maxdepth 1 \
    ! -name package.json \
    ! -name bindings \
    ! -name prebuilds \
    ! -name src \
    -exec rm -rf {} +

  if [[ -d "${pkg_dir}/src" ]]; then
    find "${pkg_dir}/src" -mindepth 1 \
      ! -name node-types.json \
      ! -path '*/tree_sitter' \
      ! -path '*/tree_sitter/*' \
      -exec rm -rf {} +
  fi
}

slim_native_bindings_for_platform() {
  local platform dst native_platform onnx_platform onnx_os onnx_arch scoped_pkg
  platform="$1"
  dst="${TAURI_RESOURCES_DIR}/engine"
  native_platform="$(native_platform_from_resource_platform "${platform}")"
  onnx_platform="$(onnx_platform_from_resource_platform "${platform}")"
  onnx_os="${onnx_platform%%/*}"
  onnx_arch="${onnx_platform##*/}"

  find "${dst}/gitnexus/node_modules" -maxdepth 1 -type d -name 'tree-sitter*' -print0 |
    while IFS= read -r -d '' pkg; do
      slim_tree_sitter_package "${pkg}" "${native_platform}"
    done

  for scoped_pkg in "${dst}/gitnexus/node_modules/@ladybugdb"/core-*; do
    [[ -e "${scoped_pkg}" ]] || continue
    case "$(basename "${scoped_pkg}")" in
      core-darwin-arm64|core-darwin-x64|core-win32-x64)
        [[ "$(basename "${scoped_pkg}")" = "core-${native_platform}" ]] || rm -rf "${scoped_pkg}"
        ;;
    esac
  done

  if [[ -d "${dst}/gitnexus/node_modules/onnxruntime-node/bin/napi-v6" ]]; then
    find "${dst}/gitnexus/node_modules/onnxruntime-node/bin/napi-v6" -mindepth 1 -maxdepth 1 -type d ! -name "${onnx_os}" -exec rm -rf {} +
    find "${dst}/gitnexus/node_modules/onnxruntime-node/bin/napi-v6/${onnx_os}" -mindepth 1 -maxdepth 1 -type d ! -name "${onnx_arch}" -exec rm -rf {} + 2>/dev/null || true
  fi

  # The desktop runtime is Node-only; onnxruntime-web's WASM/browser payload is
  # large and not used by the bundled emitter.
  rm -rf "${dst}/gitnexus/node_modules/onnxruntime-web"
}

slim_engine_resource() {
  local platform dst npm_os npm_cpu native_platform ladybug_pkg
  platform="$1"
  dst="${TAURI_RESOURCES_DIR}/engine"
  native_platform="$(native_platform_from_resource_platform "${platform}")"

  case "${platform}" in
    darwin-arm64) npm_os="darwin"; npm_cpu="arm64"; ladybug_pkg="core-darwin-arm64" ;;
    darwin-x64) npm_os="darwin"; npm_cpu="x64"; ladybug_pkg="core-darwin-x64" ;;
    win-x64) npm_os="win32"; npm_cpu="x64"; ladybug_pkg="core-win32-x64" ;;
  esac

  echo "==> 瘦身 engine 运行时 (${platform})"
  npm --prefix "${dst}/gitnexus" prune \
    --omit=dev \
    --ignore-scripts \
    --include=optional \
    --os="${npm_os}" \
    --cpu="${npm_cpu}"

  # npm prune removes the materialized vendored grammars because they are not
  # package-lock dependencies. Re-materialize them after pruning, then activate
  # the matching prebuilds without requiring a compiler.
  (
    cd "${dst}/gitnexus"
    node scripts/materialize-vendor-grammars.cjs
    node scripts/build-tree-sitter-grammars.cjs
  )

  rm -rf \
    "${dst}/gitnexus-shared" \
    "${dst}/gitnexus/src" \
    "${dst}/gitnexus/scripts" \
    "${dst}/gitnexus/test" \
    "${dst}/gitnexus/tests" \
    "${dst}/gitnexus/web" \
    "${dst}/gitnexus/hooks" \
    "${dst}/gitnexus/skills" \
    "${dst}/gitnexus/.vitest" \
    "${dst}/gitnexus/coverage"
  find "${dst}/gitnexus/vendor" -mindepth 1 -maxdepth 1 -type d ! -name leiden -exec rm -rf {} + 2>/dev/null || true

  find "${dst}" -type d \( \
      -name .cache -o \
      -name .github -o \
      -name .vscode -o \
      -name __tests__ -o \
      -name test -o \
      -name tests -o \
      -name docs -o \
      -name doc -o \
      -name example -o \
      -name examples -o \
      -name benchmark -o \
      -name benchmarks \
    \) -prune -exec rm -rf {} +
  find "${dst}" -type f \( \
      -name '*.tsbuildinfo' -o \
      -name '*.map' -o \
      -name '.DS_Store' -o \
      -name 'CHANGELOG*' -o \
      -name 'HISTORY*' \
    \) -delete

  slim_native_bindings_for_platform "${platform}"

  if [[ -f "${dst}/gitnexus/node_modules/@ladybugdb/${ladybug_pkg}/lbugjs.node" ]]; then
    cp "${dst}/gitnexus/node_modules/@ladybugdb/${ladybug_pkg}/lbugjs.node" \
      "${dst}/gitnexus/node_modules/@ladybugdb/core/lbugjs.node"
  fi

  [[ -f "${dst}/gitnexus/dist/export/emit-cli.js" ]] || { echo "error: engine emit-cli build 缺失" >&2; return 1; }
  [[ -f "${dst}/gitnexus/node_modules/@ladybugdb/core/lbugjs.node" ]] || { echo "error: engine 缺少 Ladybug native binding" >&2; return 1; }
  [[ -f "${dst}/gitnexus/node_modules/tree-sitter/prebuilds/${native_platform}/tree-sitter.node" ]] || { echo "error: engine 缺少 tree-sitter ${native_platform} runtime" >&2; return 1; }
  [[ -f "${dst}/gitnexus/node_modules/tree-sitter-javascript/prebuilds/${native_platform}/tree-sitter-javascript.node" ]] || { echo "error: engine 缺少 tree-sitter-javascript ${native_platform} runtime" >&2; return 1; }
  [[ -f "${dst}/gitnexus/node_modules/tree-sitter-c/prebuilds/${native_platform}/tree-sitter-c.node" ]] || { echo "error: engine 缺少 tree-sitter-c ${native_platform} runtime" >&2; return 1; }
}

copy_engine_resource() {
  local src dst platform npm_os npm_cpu ladybug_pkg
  platform="$1"
  src="${REPO_ROOT}/engine"
  dst="${TAURI_RESOURCES_DIR}/engine"

  case "${platform}" in
    darwin-arm64) npm_os="darwin"; npm_cpu="arm64"; ladybug_pkg="core-darwin-arm64" ;;
    darwin-x64) npm_os="darwin"; npm_cpu="x64"; ladybug_pkg="core-darwin-x64" ;;
    win-x64) npm_os="win32"; npm_cpu="x64"; ladybug_pkg="core-win32-x64" ;;
    *) echo "error: 不支持的 engine npm 平台 ${platform}" >&2; return 1 ;;
  esac

  [[ -d "${src}/gitnexus" ]] || { echo "error: 找不到 engine/gitnexus，请先初始化 engine 依赖" >&2; return 1; }
  [[ -d "${src}/gitnexus-shared" ]] || { echo "error: 找不到 engine/gitnexus-shared，请先初始化 engine 依赖" >&2; return 1; }

  rm -rf "${dst}"
  mkdir -p "${TAURI_RESOURCES_DIR}"
  rsync -a --delete \
    --exclude '.git' \
    --exclude '.DS_Store' \
    --exclude 'node_modules' \
    "${src}/" "${dst}/"

  echo "==> 准备 engine npm 依赖 (${npm_os}/${npm_cpu})"
  npm --prefix "${dst}/gitnexus-shared" ci \
    --ignore-scripts \
    --include=optional \
    --os="${npm_os}" \
    --cpu="${npm_cpu}"
  npm --prefix "${dst}/gitnexus-shared" run build
  GITNEXUS_SKIP_OPTIONAL_GRAMMARS=0 npm --prefix "${dst}/gitnexus" ci \
    --ignore-scripts \
    --include=optional \
    --os="${npm_os}" \
    --cpu="${npm_cpu}"
  (
    cd "${dst}/gitnexus"
    node scripts/materialize-vendor-grammars.cjs
    node scripts/build-tree-sitter-grammars.cjs
  )
  npm --prefix "${dst}/gitnexus" run build

  if [[ -f "${dst}/gitnexus/node_modules/@ladybugdb/${ladybug_pkg}/lbugjs.node" ]]; then
    cp "${dst}/gitnexus/node_modules/@ladybugdb/${ladybug_pkg}/lbugjs.node" \
      "${dst}/gitnexus/node_modules/@ladybugdb/core/lbugjs.node"
  fi

  slim_engine_resource "${platform}"

  [[ -f "${dst}/gitnexus/dist/export/emit-cli.js" ]] || { echo "error: engine emit-cli build 缺失" >&2; return 1; }
  [[ -f "${dst}/gitnexus/node_modules/@ladybugdb/core/lbugjs.node" ]] || { echo "error: engine 缺少 Ladybug native binding" >&2; return 1; }
  case "${platform}" in
    win-x64)
      file "${dst}/gitnexus/node_modules/@ladybugdb/core/lbugjs.node" | grep -q "PE32+" || { echo "error: Windows engine Ladybug binding 不是 PE32+ DLL" >&2; return 1; }
      [[ -f "${dst}/gitnexus/node_modules/tree-sitter/prebuilds/win32-x64/tree-sitter.node" ]] || { echo "error: Windows engine 缺少 tree-sitter win32-x64 runtime" >&2; return 1; }
      [[ -f "${dst}/gitnexus/node_modules/tree-sitter-javascript/prebuilds/win32-x64/tree-sitter-javascript.node" ]] || { echo "error: Windows engine 缺少 tree-sitter-javascript win32-x64 runtime" >&2; return 1; }
      ;;
    darwin-arm64)
      file "${dst}/gitnexus/node_modules/@ladybugdb/core/lbugjs.node" | grep -q "Mach-O.*arm64" || { echo "error: macOS arm64 engine Ladybug binding 不是 arm64 Mach-O" >&2; return 1; }
      [[ -f "${dst}/gitnexus/node_modules/tree-sitter/prebuilds/darwin-arm64/tree-sitter.node" ]] || { echo "error: macOS arm64 engine 缺少 tree-sitter runtime" >&2; return 1; }
      [[ -f "${dst}/gitnexus/node_modules/tree-sitter-javascript/prebuilds/darwin-arm64/tree-sitter-javascript.node" ]] || { echo "error: macOS arm64 engine 缺少 tree-sitter-javascript runtime" >&2; return 1; }
      ;;
    darwin-x64)
      file "${dst}/gitnexus/node_modules/@ladybugdb/core/lbugjs.node" | grep -q "Mach-O.*x86_64" || { echo "error: macOS x64 engine Ladybug binding 不是 x86_64 Mach-O" >&2; return 1; }
      [[ -f "${dst}/gitnexus/node_modules/tree-sitter/prebuilds/darwin-x64/tree-sitter.node" ]] || { echo "error: macOS x64 engine 缺少 tree-sitter runtime" >&2; return 1; }
      [[ -f "${dst}/gitnexus/node_modules/tree-sitter-javascript/prebuilds/darwin-x64/tree-sitter-javascript.node" ]] || { echo "error: macOS x64 engine 缺少 tree-sitter-javascript runtime" >&2; return 1; }
      ;;
  esac
}

copy_node_resource() {
  local platform archive url cache unpacked src dst
  platform="$1"
  dst="${TAURI_RESOURCES_DIR}/node"
  mkdir -p "${TAURI_RESOURCES_DIR}" "${REPO_ROOT}/target/release-node"
  rm -rf "${dst}"

  case "${platform}" in
    darwin-*)
      archive="${REPO_ROOT}/target/release-node/node-${NODE_VERSION}-${platform}.tar.gz"
      url="https://nodejs.org/dist/${NODE_VERSION}/node-${NODE_VERSION}-${platform}.tar.gz"
      if [[ ! -f "${archive}" ]]; then
        echo "==> 下载 Node ${NODE_VERSION} (${platform})"
        curl -fL "${url}" -o "${archive}"
      fi
      cache="$(mktemp -d)"
      tar -xzf "${archive}" -C "${cache}"
      unpacked="${cache}/node-${NODE_VERSION}-${platform}"
      mkdir -p "${dst}"
      cp -R "${unpacked}/bin" "${dst}/bin"
      cp "${unpacked}/README.md" "${unpacked}/LICENSE" "${dst}/" 2>/dev/null || true
      rm -rf "${cache}"
      ;;
    win-x64)
      archive="${REPO_ROOT}/target/release-node/node-${NODE_VERSION}-${platform}.zip"
      url="https://nodejs.org/dist/${NODE_VERSION}/node-${NODE_VERSION}-${platform}.zip"
      if [[ ! -f "${archive}" ]]; then
        echo "==> 下载 Node ${NODE_VERSION} (${platform})"
        curl -fL "${url}" -o "${archive}"
      fi
      cache="$(mktemp -d)"
      unzip -q "${archive}" -d "${cache}"
      unpacked="${cache}/node-${NODE_VERSION}-${platform}"
      mkdir -p "${dst}"
      cp "${unpacked}/node.exe" "${dst}/node.exe"
      cp "${unpacked}/README.md" "${unpacked}/LICENSE" "${dst}/" 2>/dev/null || true
      rm -rf "${cache}"
      ;;
    *)
      echo "error: 不支持的 Node 平台 ${platform}" >&2
      return 1
      ;;
  esac

  if [[ "${platform}" = win-* ]]; then
    [[ -f "${dst}/node.exe" ]] || { echo "error: Node runtime 缺少 node.exe" >&2; return 1; }
  else
    [[ -x "${dst}/bin/node" ]] || { echo "error: Node runtime 缺少 bin/node" >&2; return 1; }
  fi
  slim_node_resource "${platform}"
}

prepare_desktop_resources() {
  local triple platform
  triple="$1"
  platform="$(platform_from_triple "${triple}")"
  echo "==> 准备桌面内置资源 (${platform})"
  copy_engine_resource "${platform}"
  copy_node_resource "${platform}"
}

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
        prepare_desktop_resources "${desktop_triple}"
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
  local win_triple exe_path setup_src setup_exe portable_zip stage engine_src node_src
  win_triple="x86_64-pc-windows-msvc"

  if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    prepare_desktop_resources "${win_triple}"
    rm -rf \
      "${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/engine" \
      "${REPO_ROOT}/apps/desktop/src-tauri/target/${win_triple}/release/node"
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
  engine_src="${TAURI_RESOURCES_DIR}/engine"
  node_src="${TAURI_RESOURCES_DIR}/node"
  [[ -d "${engine_src}/gitnexus" ]] || { echo "error: 找不到 Windows portable 所需 engine 资源: ${engine_src}" >&2; return 1; }
  [[ -f "${node_src}/node.exe" ]] || { echo "error: 找不到 Windows portable 所需 Node runtime: ${node_src}" >&2; return 1; }
  cp -R "${engine_src}" "${stage}/engine"
  cp -R "${node_src}" "${stage}/node"
  (cd "${stage}" && zip -q -X -r "${portable_zip}" aka-desktop.exe engine node)
  rm -rf "${stage}"
  echo "==> ${portable_zip}"
}

# ---------- 子命令: 只生成校验和（放在最后由主线程统一跑） ----------
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
  if [[ "${TRIPLE}" = "x86_64-pc-windows-msvc" ]] && [[ "$(uname -s)" = "Darwin" ]] && command -v cargo-xwin >/dev/null 2>&1; then
    echo "==> cargo xwin ${build_args[*]}"
    (cd "${REPO_ROOT}" && cargo xwin "${build_args[@]}")
  else
    echo "==> cargo ${build_args[*]}"
    (cd "${REPO_ROOT}" && cargo "${build_args[@]}")
  fi
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
