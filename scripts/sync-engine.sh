#!/usr/bin/env bash
# Build or refresh the native codebase-memory-mcp engine used by aka.
#
# Usage:
#   scripts/sync-engine.sh [--refresh-upstream] [source-or-repo-url]
#
# By default this builds the existing maintained checkout at
# engine/codebase-memory-mcp-src without resetting it. Use --refresh-upstream
# only for a deliberate upstream sync.
set -euo pipefail

REFRESH_UPSTREAM=0
if [[ "${1:-}" == "--refresh-upstream" ]]; then
  REFRESH_UPSTREAM=1
  shift
fi

SRC="${1:-https://github.com/caork/codebase-memory-mcp.git}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DST="${ROOT}/engine"
CHECKOUT="${DST}/codebase-memory-mcp-src"
BIN_NAME="codebase-memory-mcp"
if [[ "$(uname -s)" =~ MINGW|MSYS|CYGWIN ]]; then
  BIN_NAME="codebase-memory-mcp.exe"
fi

mkdir -p "${DST}"

SHA=""
if [[ ${REFRESH_UPSTREAM} -eq 1 || ! -d "${CHECKOUT}/.git" ]]; then
  if [[ -d "${SRC}/.git" ]]; then
    rm -rf "${CHECKOUT}"
    mkdir -p "${CHECKOUT}"
    rsync -a --delete \
      --exclude .git \
      --exclude build \
      "${SRC}/" "${CHECKOUT}/"
    SHA="$(git -C "${SRC}" rev-parse HEAD)"
  elif [[ -d "${CHECKOUT}/.git" ]]; then
    git -C "${CHECKOUT}" remote set-url origin "${SRC}"
    git -C "${CHECKOUT}" fetch --tags --prune origin
    git -C "${CHECKOUT}" checkout --detach origin/HEAD
    git -C "${CHECKOUT}" reset --hard origin/HEAD
    git -C "${CHECKOUT}" clean -fdx
  else
    rm -rf "${CHECKOUT}"
    git clone --depth 1 "${SRC}" "${CHECKOUT}"
  fi
fi

if [[ -z "${SHA}" ]]; then
  SHA="$(git -C "${CHECKOUT}" rev-parse HEAD)"
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

echo "engine checkout: ${CHECKOUT}"
echo "engine binary: ${DST}/${BIN_NAME}"
echo "ENGINE_SHA=${SHA}"
