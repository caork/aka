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

FORK_URL="${1:-https://github.com/caork/aka-engine.git}"
UPSTREAM_URL="${AKA_ENGINE_UPSTREAM_URL:-https://github.com/DeusData/codebase-memory-mcp.git}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DST="${ROOT}/engine"
CHECKOUT="${AKA_ENGINE_SRC:-${DST}/aka-engine-src}"

mkdir -p "${DST}"

SHA=""
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

make -C "${CHECKOUT}" -f Makefile.cbm libaka-engine
if [[ "$(uname -s)" =~ MINGW|MSYS|CYGWIN ]]; then
  make -C "${CHECKOUT}" -f Makefile.cbm aka-engine-dll
fi

LIB_BUILT="${CHECKOUT}/build/c/libaka_engine.a"
if [[ ! -f "${LIB_BUILT}" ]]; then
  echo "error: build did not produce ${LIB_BUILT}" >&2
  exit 1
fi

rm -f \
  "${DST}/aka-engine" \
  "${DST}/aka-engine.exe" \
  "${DST}/codebase-memory-mcp" \
  "${DST}/codebase-memory-mcp.exe"
if [[ "$(uname -s)" =~ MINGW|MSYS|CYGWIN ]]; then
  DLL_BUILT="${CHECKOUT}/build/c/aka_engine.dll"
  if [[ ! -f "${DLL_BUILT}" ]]; then
    echo "error: build did not produce ${DLL_BUILT}" >&2
    exit 1
  fi
  cp "${DLL_BUILT}" "${DST}/aka_engine.dll"
fi
printf '%s\n' "${SHA}" > "${DST}/ENGINE_SHA"

echo "engine checkout: ${CHECKOUT}"
echo "engine embedded lib: ${LIB_BUILT}"
if [[ "$(uname -s)" =~ MINGW|MSYS|CYGWIN ]]; then
  echo "engine dll: ${DST}/aka_engine.dll"
fi
echo "ENGINE_SHA=${SHA}"
