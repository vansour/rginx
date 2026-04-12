#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

log() {
    printf '[test-slow] %s\n' "$*"
}

cd "${ROOT_DIR}"

log "running slow integration tests"

required_http3_targets=(
    "http3"
    "upstream_http3"
    "grpc_http3"
    "reload"
    "admin"
    "check"
)

for test_name in "${required_http3_targets[@]}"; do
    if [[ ! -f "${ROOT_DIR}/crates/rginx-app/tests/${test_name}.rs" ]]; then
        printf '[test-slow] missing required HTTP/3 gate target: %s\n' "${test_name}" >&2
        exit 1
    fi
done

mapfile -t tests < <(
    find "${ROOT_DIR}/crates/rginx-app/tests" -maxdepth 1 -type f -name '*.rs' -printf '%f\n' |
        sed 's/\.rs$//' |
        sort
)

for test_name in "${tests[@]}"; do
    log "running ${test_name}"
    cargo test -p rginx --test "${test_name}" --locked --quiet -- --test-threads=1
done
