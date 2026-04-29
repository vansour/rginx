#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

ITERATIONS=1
RELEASE=0

usage() {
    cat <<'EOF'
Usage: run-cache-stress.sh [options]

Options:
  --iterations <n>
      重复执行整套 cache stress 矩阵的次数，默认 1
  --release
      使用 cargo test --release
  -h, --help
      显示帮助
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --iterations)
            [[ $# -ge 2 ]] || { echo "--iterations requires a value" >&2; exit 1; }
            ITERATIONS="$2"
            shift 2
            ;;
        --release)
            RELEASE=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            exit 1
            ;;
    esac
done

[[ "${ITERATIONS}" =~ ^[0-9]+$ ]] || { echo "--iterations must be a positive integer" >&2; exit 1; }
[[ "${ITERATIONS}" -ge 1 ]] || { echo "--iterations must be >= 1" >&2; exit 1; }

cd "${ROOT_DIR}"

if [[ "${RELEASE}" -eq 1 ]]; then
    cargo_args=(cargo test --release --locked)
else
    cargo_args=(cargo test --locked)
fi

matrix=(
    "rginx-http|cache_manager_handles_large_keysets_under_parallel_fill_and_hit_load|crate-local large-key and parallel hit/fill stress"
    "rginx-http|cache_manager_serves_large_cached_body_under_sustained_parallel_hits|crate-local large-object sustained hit stress"
    "rginx|reload_keeps_hot_cache_hits_available_under_concurrent_traffic|real-process reload plus hot-cache concurrency stress"
)

for iteration in $(seq 1 "${ITERATIONS}"); do
    printf '[cache-stress] iteration %s/%s\n' "${iteration}" "${ITERATIONS}"
    for entry in "${matrix[@]}"; do
        crate_name="${entry%%|*}"
        remainder="${entry#*|}"
        test_name="${remainder%%|*}"
        label="${remainder#*|}"
        printf '[cache-stress] running %-60s %s\n' "${test_name}" "${label}"
        if [[ "${crate_name}" == "rginx" ]]; then
            "${cargo_args[@]}" -p rginx --test reload "${test_name}" -- --ignored --nocapture --test-threads=1
        else
            "${cargo_args[@]}" -p "${crate_name}" --lib "${test_name}" -- --ignored --nocapture --test-threads=1
        fi
    done
done

printf '[cache-stress] completed %s iteration(s)\n' "${ITERATIONS}"
