#!/usr/bin/env bash
# Fetch/build the native codebase-memory-mcp engine used by aka.
#
# Usage:
#   scripts/sync-engine.sh [source-or-repo-url]
#
# Defaults to https://github.com/DeusData/codebase-memory-mcp.git. The source is
# cloned/copied into ignored engine/codebase-memory-mcp-src/ and built in place.
# The resulting binary is copied to engine/codebase-memory-mcp for desktop/Docker
# packaging, and engine/ENGINE_SHA records the upstream commit.
set -euo pipefail

SRC="${1:-https://github.com/DeusData/codebase-memory-mcp.git}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DST="${ROOT}/engine"
CHECKOUT="${DST}/codebase-memory-mcp-src"
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
    git -C "${CHECKOUT}" checkout origin/HEAD
  else
    rm -rf "${CHECKOUT}"
    git clone --depth 1 "${SRC}" "${CHECKOUT}"
  fi
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

echo "synced ${SRC} -> ${CHECKOUT}"
echo "engine binary: ${DST}/${BIN_NAME}"
echo "ENGINE_SHA=${SHA}"
