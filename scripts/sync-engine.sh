#!/usr/bin/env bash
# Fetch/build the native codebase-memory-mcp engine used by aka.
#
# Usage:
#   scripts/sync-engine.sh [source-or-repo-url]
#
# Defaults to https://github.com/DeusData/codebase-memory-mcp.git. The source is
# cloned/copied into ignored engine/codebase-memory-mcp-src/ and built in place.
# The resulting binary is copied to engine/codebase-memory-mcp for desktop/Docker
# packaging, engine/ENGINE_SHA records the upstream commit, and tracked patches
# under engine/patches/codebase-memory-mcp/*.patch are applied before building.
set -euo pipefail

SRC="${1:-https://github.com/DeusData/codebase-memory-mcp.git}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DST="${ROOT}/engine"
CHECKOUT="${DST}/codebase-memory-mcp-src"
PATCH_DIR="${DST}/patches/codebase-memory-mcp"
BIN_NAME="codebase-memory-mcp"
if [[ "$(uname -s)" =~ MINGW|MSYS|CYGWIN ]]; then
  BIN_NAME="codebase-memory-mcp.exe"
fi

mkdir -p "${DST}"

if [[ -d "${SRC}/.git" ]]; then
  rm -rf "${CHECKOUT}"
  mkdir -p "${CHECKOUT}"
  rsync -a --delete \
    --exclude .git \
    --exclude build \
    "${SRC}/" "${CHECKOUT}/"
  SHA="$(git -C "${SRC}" rev-parse HEAD)"
else
  if [[ -d "${CHECKOUT}/.git" ]]; then
    git -C "${CHECKOUT}" fetch --tags --prune origin
    git -C "${CHECKOUT}" checkout --detach origin/HEAD
    git -C "${CHECKOUT}" reset --hard origin/HEAD
    git -C "${CHECKOUT}" clean -fdx
  else
    rm -rf "${CHECKOUT}"
    git clone --depth 1 "${SRC}" "${CHECKOUT}"
  fi
  SHA="$(git -C "${CHECKOUT}" rev-parse HEAD)"
fi

if [[ -d "${PATCH_DIR}" ]]; then
  while IFS= read -r patch_file; do
    echo "applying engine patch: ${patch_file#${ROOT}/}"
    patch -d "${CHECKOUT}" -p1 < "${patch_file}"
  done < <(find "${PATCH_DIR}" -type f -name '*.patch' | sort)
fi

make -C "${CHECKOUT}" -f Makefile.cbm cbm

BUILT="${CHECKOUT}/build/c/${BIN_NAME}"
if [[ ! -x "${BUILT}" ]]; then
  echo "error: build did not produce ${BUILT}" >&2
  exit 1
fi

cp "${BUILT}" "${DST}/${BIN_NAME}"
chmod +x "${DST}/${BIN_NAME}"
printf '%s\n' "${SHA}" > "${DST}/ENGINE_SHA"

echo "synced ${SRC} -> ${CHECKOUT}"
echo "engine binary: ${DST}/${BIN_NAME}"
echo "ENGINE_SHA=${SHA}"
