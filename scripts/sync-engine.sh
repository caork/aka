#!/usr/bin/env bash
# Build or refresh the native AKA engine used by aka.
#
# Usage:
#   scripts/sync-engine.sh [--refresh-upstream] [source-or-repo-url]
#
# By default this builds the existing maintained checkout at
# engine/aka-engine-src without resetting it. Use --refresh-upstream only for a
# deliberate upstream review; it never resets or cleans the maintained checkout.
set -euo pipefail

REFRESH_UPSTREAM=0
if [[ "${1:-}" == "--refresh-upstream" ]]; then
  REFRESH_UPSTREAM=1
  shift
fi

FORK_URL="${1:-https://github.com/caork/codebase-memory-mcp.git}"
UPSTREAM_URL="${AKA_ENGINE_UPSTREAM_URL:-${CBM_UPSTREAM_URL:-https://github.com/DeusData/codebase-memory-mcp.git}}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DST="${ROOT}/engine"
CHECKOUT="${AKA_ENGINE_SRC:-${DST}/aka-engine-src}"
LEGACY_CHECKOUT="${DST}/codebase-memory-mcp-src"
BIN_NAME="aka-engine"
LEGACY_BIN_NAME="codebase-memory-mcp"
if [[ "$(uname -s)" =~ MINGW|MSYS|CYGWIN ]]; then
  BIN_NAME="aka-engine.exe"
  LEGACY_BIN_NAME="codebase-memory-mcp.exe"
fi

mkdir -p "${DST}"

SHA=""
if [[ ! -d "${CHECKOUT}/.git" && -d "${LEGACY_CHECKOUT}/.git" ]]; then
  CHECKOUT="${LEGACY_CHECKOUT}"
fi

if [[ ! -d "${CHECKOUT}/.git" ]]; then
  if [[ -d "${FORK_URL}/.git" ]]; then
    rm -rf "${CHECKOUT}"
    mkdir -p "${CHECKOUT}"
    rsync -a --delete \
      --exclude .git \
      --exclude build \
      "${FORK_URL}/" "${CHECKOUT}/"
    SHA="$(git -C "${FORK_URL}" rev-parse HEAD)"
  else
    rm -rf "${CHECKOUT}"
    git clone "${FORK_URL}" "${CHECKOUT}"
  fi
fi

ensure_remote() {
  local name="$1"
  local url="$2"
  if git -C "${CHECKOUT}" remote get-url "${name}" >/dev/null 2>&1; then
    git -C "${CHECKOUT}" remote set-url "${name}" "${url}"
  else
    git -C "${CHECKOUT}" remote add "${name}" "${url}"
  fi
}

if [[ ${REFRESH_UPSTREAM} -eq 1 ]]; then
  ensure_remote aka "${FORK_URL}"
  ensure_remote upstream "${UPSTREAM_URL}"
  git -C "${CHECKOUT}" fetch --tags --prune aka
  git -C "${CHECKOUT}" fetch --tags --prune upstream
  echo "Fetched aka (${FORK_URL}) and upstream (${UPSTREAM_URL})."
  echo "Review and merge/rebase manually inside ${CHECKOUT}; this script will build the current checkout as-is."
fi

if [[ -z "${SHA}" ]]; then
  SHA="$(git -C "${CHECKOUT}" rev-parse HEAD)"
fi

make -C "${CHECKOUT}" -f Makefile.cbm cbm

BUILT="${CHECKOUT}/build/c/${BIN_NAME}"
if [[ ! -x "${BUILT}" ]]; then
  BUILT="${CHECKOUT}/build/c/${LEGACY_BIN_NAME}"
fi
if [[ ! -x "${BUILT}" ]]; then
  echo "error: build did not produce ${BUILT}" >&2
  exit 1
fi

cp "${BUILT}" "${DST}/${BIN_NAME}"
chmod +x "${DST}/${BIN_NAME}"
cp "${BUILT}" "${DST}/${LEGACY_BIN_NAME}"
chmod +x "${DST}/${LEGACY_BIN_NAME}"
printf '%s\n' "${SHA}" > "${DST}/ENGINE_SHA"

echo "engine checkout: ${CHECKOUT}"
echo "engine binary: ${DST}/${BIN_NAME}"
echo "ENGINE_SHA=${SHA}"
