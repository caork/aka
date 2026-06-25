#!/usr/bin/env bash
# Smoke optional tree-sitter stack-graphs Python enrichment on a real Python repository.
# AKA runtime imports the generated aka-facts bundle; it does not start stack-graphs.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/smoke-oss-analyzer-stack-graphs-python.sh --repo /path/to/repo [options]

Required:
  --repo PATH                 Repository to index.

Options:
  --facts PATH                Existing/generated aka-facts bundle path.
                              Defaults to REPO/.aka/stack-graphs-python-oss-analyzer-facts.json.
  --tool CMD                  stack-graphs Python CLI command.
                              Default: tree-sitter-stack-graphs-python
  --database PATH             stack-graphs database path.
  --tool-version VERSION      Override analyzer toolVersion in the generated bundle.
  --query TEXT                Search query after indexing. Default: importlib.
  --context SYMBOL            Optional symbol context query after indexing.
  --min-lines N               Required Python source line count. Default: 500000.
  --aka-home PATH             Isolated AKA_HOME. Defaults to a temp dir.
  --index-max-secs N          Global indexing budget. Default: 1800.
  --enrichment-max-secs N     Optional stack-graphs import budget. Default: 600.
  --adapter-timeout-secs N    stack-graphs adapter generation budget. Default: 1800.
  --max-files N               Optional cap for adapter smoke/debug runs.
  --max-query-positions N     Max stack-graphs definition queries. Default: 10000.
  --index-batch-size N        Files per stack-graphs index invocation. Default: 200.
  --index-batch-timeout-secs N  Per stack-graphs index invocation deadline. Default: 300.
  --query-batch-size N        Positions per definition query invocation. Default: 32.
  --query-timeout-secs N      Per stack-graphs definition query deadline. Default: 5.
  --max-query-timeouts-per-file N  Skip a file after this many query timeouts. Default: 2.
  --max-file-secs N           stack-graphs per-file indexing budget. Default: 20.
  --exclude-dir PATH          Additional repo-relative directory to skip. Repeatable.
  --allow-small-repo          Allow Python source lines below --min-lines for adapter debugging.
  --keep-aka-home             Do not delete the temporary AKA_HOME.
  --skip-generate             Reuse --facts without running stack-graphs.
  --dry-run                   Validate inputs and generated settings, then exit.

Example:
  scripts/smoke-oss-analyzer-stack-graphs-python.sh --repo ~/src/cpython \
    --tool /tmp/aka-large-smoke/bin/tree-sitter-stack-graphs-python \
    --tool-version tree-sitter-stack-graphs-python-0.3.0 \
    --query importlib --context main
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

repo=""
facts_path=""
tool="tree-sitter-stack-graphs-python"
database_path=""
tool_version=""
query="importlib"
context_symbol=""
min_lines=500000
aka_home=""
index_max_secs=1800
enrichment_max_secs=600
adapter_timeout_secs=1800
max_files=""
max_query_positions=10000
index_batch_size=200
index_batch_timeout_secs=300
query_batch_size=32
query_timeout_secs=5
max_query_timeouts_per_file=2
max_file_secs=20
exclude_dirs=()
allow_small_repo=0
keep_aka_home=0
skip_generate=0
dry_run=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)
      repo="${2:-}"
      shift 2
      ;;
    --facts)
      facts_path="${2:-}"
      shift 2
      ;;
    --tool)
      tool="${2:-}"
      shift 2
      ;;
    --database)
      database_path="${2:-}"
      shift 2
      ;;
    --tool-version)
      tool_version="${2:-}"
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
    --adapter-timeout-secs)
      adapter_timeout_secs="${2:-}"
      shift 2
      ;;
    --max-files)
      max_files="${2:-}"
      shift 2
      ;;
    --max-query-positions)
      max_query_positions="${2:-}"
      shift 2
      ;;
    --index-batch-size)
      index_batch_size="${2:-}"
      shift 2
      ;;
    --index-batch-timeout-secs)
      index_batch_timeout_secs="${2:-}"
      shift 2
      ;;
    --query-batch-size)
      query_batch_size="${2:-}"
      shift 2
      ;;
    --query-timeout-secs)
      query_timeout_secs="${2:-}"
      shift 2
      ;;
    --max-query-timeouts-per-file)
      max_query_timeouts_per_file="${2:-}"
      shift 2
      ;;
    --max-file-secs)
      max_file_secs="${2:-}"
      shift 2
      ;;
    --exclude-dir)
      exclude_dirs+=("${2:-}")
      shift 2
      ;;
    --allow-small-repo)
      allow_small_repo=1
      shift
      ;;
    --keep-aka-home)
      keep_aka_home=1
      shift
      ;;
    --skip-generate)
      skip_generate=1
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
facts_path="${facts_path:-${repo}/.aka/stack-graphs-python-oss-analyzer-facts.json}"
database_path="${database_path:-${repo}/.aka/stack-graphs-python.db}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"

find_prunes=(
  -path '*/.git' -o
  -path '*/__pycache__' -o
  -path '*/.mypy_cache' -o
  -path '*/.pytest_cache' -o
  -path '*/.ruff_cache' -o
  -path '*/.tox' -o
  -path '*/.venv' -o
  -path '*/venv' -o
  -path '*/node_modules' -o
  -path '*/target' -o
  -path '*/build' -o
  -path '*/dist'
)
for exclude_dir in ${exclude_dirs[@]+"${exclude_dirs[@]}"}; do
  [[ -n "${exclude_dir}" ]] || continue
  find_prunes+=(-o -path "*/${exclude_dir%/}")
done

line_count="$(
  {
    while IFS= read -r -d '' file; do
      wc -l <"${file}"
    done < <(find "${repo}" \
      \( "${find_prunes[@]}" \) -prune \
      -o -type f \( -name '*.py' -o -name '*.pyi' \) -print0)
    echo 0
  } \
  | awk '{ total += $1 } END { print total + 0 }'
)"

echo "==> repo: ${repo}"
echo "==> Python source lines: ${line_count}"
echo "==> facts bundle: ${facts_path}"
echo "==> stack-graphs tool: ${tool}"
echo "==> stack-graphs database: ${database_path}"

if [[ "${line_count}" -lt "${min_lines}" ]]; then
  if [[ "${allow_small_repo}" -eq 0 ]]; then
    die "Python source line count ${line_count} is below requested ${min_lines}; pass --allow-small-repo only for adapter debugging"
  fi
  echo "warning: Python source line count ${line_count} is below requested ${min_lines}" >&2
fi

if [[ -z "${aka_home}" ]]; then
  aka_home="$(mktemp -d "${TMPDIR:-/tmp}/aka-stack-graphs-python-smoke.XXXXXX")"
  if [[ "${keep_aka_home}" -eq 0 ]]; then
    trap 'rm -rf "${aka_home}"' EXIT
  fi
else
  mkdir -p "${aka_home}"
fi
echo "==> AKA_HOME: ${aka_home}"

json_facts_path="${facts_path//\\/\\\\}"
json_facts_path="${json_facts_path//\"/\\\"}"
mkdir -p "${aka_home}"
cat >"${aka_home}/settings.json" <<JSON
{
  "indexMaxSecs": ${index_max_secs},
  "ossAnalyzerEnrichmentEnabled": true,
  "ossAnalyzerEnrichmentMaxSecs": ${enrichment_max_secs},
  "ossAnalyzerFactsPath": "${json_facts_path}"
}
JSON

if [[ "${dry_run}" -eq 1 ]]; then
  echo "==> dry run passed"
  cat "${aka_home}/settings.json"
  exit 0
fi

if [[ "${skip_generate}" -eq 0 ]]; then
  echo "==> generating aka-facts with external tree-sitter stack-graphs"
  adapter_args=(
    --repo "${repo}"
    --out "${facts_path}"
    --tool "${tool}"
    --database "${database_path}"
    --timeout-secs "${adapter_timeout_secs}"
    --max-query-positions "${max_query_positions}"
    --index-batch-size "${index_batch_size}"
    --index-batch-timeout-secs "${index_batch_timeout_secs}"
    --query-batch-size "${query_batch_size}"
    --query-timeout-secs "${query_timeout_secs}"
    --max-query-timeouts-per-file "${max_query_timeouts_per_file}"
    --max-file-secs "${max_file_secs}"
  )
  for exclude_dir in ${exclude_dirs[@]+"${exclude_dirs[@]}"}; do
    [[ -n "${exclude_dir}" ]] || continue
    adapter_args+=(--exclude-dir "${exclude_dir}")
  done
  if [[ -n "${tool_version}" ]]; then
    adapter_args+=(--tool-version "${tool_version}")
  fi
  if [[ -n "${max_files}" ]]; then
    adapter_args+=(--max-files "${max_files}")
  fi
  node "${repo_root}/scripts/oss-analyzer-stack-graphs-python.mjs" "${adapter_args[@]}"
fi

[[ -f "${facts_path}" ]] || die "facts bundle missing: ${facts_path}"

echo "==> validating facts bundle"
(
  cd "${repo_root}"
  cargo run -p aka-cli --features embedded-engine,scip-import -- validate-facts "${facts_path}"
)

log_path="${aka_home}/stack-graphs-python-smoke.log"
echo "==> running baseline indexing plus optional stack-graphs enrichment"
(
  cd "${repo_root}"
  AKA_HOME="${aka_home}" AKA_INDEX_MAX_SECS="${index_max_secs}" \
    cargo run -p aka-cli --features embedded-engine,scip-import -- analyze "${repo}"
) 2>&1 | tee "${log_path}"

grep -Eq '就绪|ready|Reusing unchanged index' "${log_path}" \
  || die "analyze did not report a ready baseline; see ${log_path}"

grep -Eq 'provider=aka-facts-file merged|optional enrichment outcome' "${log_path}" \
  || die "stack-graphs facts enrichment outcome was not visible; see ${log_path}"

if grep -Eq 'provider=aka-facts-file failed|merge_failed|invalid_provenance' "${log_path}"; then
  die "stack-graphs facts enrichment failed; see ${log_path}"
fi

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

echo "==> stack-graphs Python OSS analyzer smoke passed"
