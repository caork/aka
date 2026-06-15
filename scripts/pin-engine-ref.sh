#!/usr/bin/env bash
# Sync Docker / release CBM pins from engine/ENGINE_SHA.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHA_FILE="${ROOT}/engine/ENGINE_SHA"

if [[ ! -f "${SHA_FILE}" ]]; then
  echo "error: missing ${SHA_FILE}; run scripts/sync-engine.sh first" >&2
  exit 1
fi

SHA="$(tr -d '[:space:]' < "${SHA_FILE}")"
if [[ ! "${SHA}" =~ ^[0-9a-f]{40}$ ]]; then
  echo "error: invalid engine sha in ${SHA_FILE}: ${SHA}" >&2
  exit 1
fi

replace_ref() {
  local file="$1"
  local pattern="$2"
  perl -0pi -e "s/${pattern}/${SHA}/g" "${file}"
}

replace_ref "${ROOT}/Dockerfile" 'ARG CBM_REF=\K[0-9a-f]{40}'
replace_ref "${ROOT}/.github/workflows/release.yml" 'CBM_REF: \K[0-9a-f]{40}'

echo "Pinned CBM_REF=${SHA}"
