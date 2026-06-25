#!/usr/bin/env bash
# Smoke optional SCIP enrichment on a real repository using an existing
# open-source analyzer result (`index.scip`). The AKA runtime only imports and
# validates the SCIP result; this script does not make SCIP a runtime fallback.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/smoke-oss-analyzer-scip.sh --repo /path/to/repo [options]

Required:
  --repo PATH              Repository to index.

Options:
  --scip-index PATH        Existing index.scip. Defaults to REPO/index.scip.
  --make-scip CMD          Optional shell command that creates index.scip.
                           Runs in REPO with AKA_SCIP_INDEX set.
  --query TEXT             Search query after indexing. Default: Service.
  --context SYMBOL         Optional symbol context query after indexing.
  --min-lines N            Warn below this source line count. Default: 500000.
  --aka-home PATH          Isolated AKA_HOME. Defaults to a temp dir.
  --index-max-secs N       Global indexing budget. Default: 1800.
  --enrichment-max-secs N  Optional SCIP import budget. Default: 600.
  --keep-aka-home          Do not delete the temporary AKA_HOME.
  --dry-run                Validate inputs and generated settings, then exit.

Examples:
  scripts/smoke-oss-analyzer-scip.sh --repo ~/src/dubbo --scip-index ~/src/dubbo/index.scip

  scripts/smoke-oss-analyzer-scip.sh --repo ~/src/dubbo \
    --make-scip 'scip-java index --build-tool maven --output "$AKA_SCIP_INDEX"'
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

repo=""
scip_index=""
make_scip=""
query="Service"
context_symbol=""
min_lines=500000
aka_home=""
index_max_secs=1800
enrichment_max_secs=600
keep_aka_home=0
dry_run=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)
      repo="${2:-}"
      shift 2
      ;;
    --scip-index)
      scip_index="${2:-}"
      shift 2
      ;;
    --make-scip)
      make_scip="${2:-}"
      shift 2
      ;;
    --query)
      query="${2:-}"
      shift 2
      ;;
    --context)
      context_symbol="${2:-}"
      shift 2
      ;;
    --min-lines)
      min_lines="${2:-}"
      shift 2
      ;;
    --aka-home)
      aka_home="${2:-}"
      shift 2
      ;;
    --index-max-secs)
      index_max_secs="${2:-}"
      shift 2
      ;;
    --enrichment-max-secs)
      enrichment_max_secs="${2:-}"
      shift 2
      ;;
    --keep-aka-home)
      keep_aka_home=1
      shift
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

[[ -n "${repo}" ]] || {
  usage >&2
  exit 2
}

[[ -d "${repo}" ]] || die "repo does not exist: ${repo}"
repo="$(cd "${repo}" && pwd -P)"
scip_index="${scip_index:-${repo}/index.scip}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

if [[ -n "${make_scip}" && ! -f "${scip_index}" ]]; then
  echo "==> generating SCIP index with external command"
  mkdir -p "$(dirname "${scip_index}")"
  (
    cd "${repo}"
    AKA_SCIP_INDEX="${scip_index}" bash -lc "${make_scip}"
  )
fi

[[ -f "${scip_index}" ]] || die "SCIP index missing: ${scip_index} (pass --scip-index or --make-scip)"

if [[ -z "${aka_home}" ]]; then
  aka_home="$(mktemp -d "${TMPDIR:-/tmp}/aka-scip-smoke.XXXXXX")"
  if [[ "${keep_aka_home}" -eq 0 ]]; then
    trap 'rm -rf "${aka_home}"' EXIT
  fi
else
  mkdir -p "${aka_home}"
fi

line_count="$(
  {
    while IFS= read -r -d '' file; do
      wc -l <"${file}"
    done < <(find "${repo}" \
      \( -path '*/.git' -o -path '*/target' -o -path '*/node_modules' -o -path '*/build' -o -path '*/dist' \) -prune \
      -o -type f \
      \( -name '*.java' -o -name '*.kt' -o -name '*.scala' -o -name '*.py' -o -name '*.ts' -o -name '*.tsx' -o -name '*.js' -o -name '*.jsx' -o -name '*.rs' -o -name '*.go' \) \
      -print0)
    echo 0
  } \
  | awk '{ total += $1 } END { print total + 0 }'
)"

echo "==> repo: ${repo}"
echo "==> source lines: ${line_count}"
echo "==> scip index: ${scip_index}"
echo "==> AKA_HOME: ${aka_home}"

if [[ "${line_count}" -lt "${min_lines}" ]]; then
  echo "warning: source line count ${line_count} is below requested ${min_lines}" >&2
fi

mkdir -p "${aka_home}"
json_scip_index="${scip_index//\\/\\\\}"
json_scip_index="${json_scip_index//\"/\\\"}"
cat >"${aka_home}/settings.json" <<JSON
{
  "indexMaxSecs": ${index_max_secs},
  "ossAnalyzerEnrichmentEnabled": true,
  "ossAnalyzerEnrichmentMaxSecs": ${enrichment_max_secs},
  "scipIndexPath": "${json_scip_index}"
}
JSON

if [[ "${dry_run}" -eq 1 ]]; then
  echo "==> dry run passed"
  cat "${aka_home}/settings.json"
  exit 0
fi

log_path="${aka_home}/scip-smoke.log"
echo "==> running baseline indexing plus optional SCIP enrichment"
(
  cd "${repo_root}"
  AKA_HOME="${aka_home}" AKA_INDEX_MAX_SECS="${index_max_secs}" \
    cargo run -p aka-cli --features embedded-engine,scip-import -- analyze "${repo}"
) 2>&1 | tee "${log_path}"

grep -Eq '就绪|ready|Reusing unchanged index' "${log_path}" \
  || die "analyze did not report a ready baseline; see ${log_path}"

grep -Eq 'provider=scip (merged|skipped|failed)|skipped remaining providers reason=timeout|optional enrichment outcome' "${log_path}" \
  || die "SCIP enrichment outcome was not visible; see ${log_path}"

if grep -Eq 'provider=scip failed|merge_failed|invalid_provenance' "${log_path}"; then
  die "SCIP enrichment failed; see ${log_path}"
fi

echo "==> list repos"
AKA_HOME="${aka_home}" cargo run -p aka-cli --features embedded-engine,scip-import -- repos

echo "==> search query: ${query}"
search_out="$(
  AKA_HOME="${aka_home}" cargo run -p aka-cli --features embedded-engine,scip-import -- search "${query}" --limit 5 2>&1
)"
echo "${search_out}"
grep -qv 'no results' <<<"${search_out}" || die "search returned no results for ${query}"

if [[ -n "${context_symbol}" ]]; then
  echo "==> context symbol: ${context_symbol}"
  context_out="$(
    AKA_HOME="${aka_home}" cargo run -p aka-cli --features embedded-engine,scip-import -- context "${context_symbol}" 2>&1
  )"
  echo "${context_out}"
  grep -Eq '^-- definitions \([1-9]' <<<"${context_out}" \
    || die "context returned no definitions for ${context_symbol}"
fi

echo "==> SCIP OSS analyzer smoke passed"
