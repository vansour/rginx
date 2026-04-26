#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

log() {
    printf '[test-fast] %s\n' "$*"
}

SKIP_MODULARIZATION_GATE="${SKIP_MODULARIZATION_GATE:-0}"

cd "${ROOT_DIR}"

if [[ "${SKIP_MODULARIZATION_GATE}" != "1" ]]; then
    log "running modularization gate"
    python3 ./scripts/run-modularization-gate.py
else
    log "skipping modularization gate because SKIP_MODULARIZATION_GATE=1"
fi

log "running fast unit and crate-local tests"

matrix=(
    "rginx-core|crate-local core model and config invariants"
    "rginx-config|config compile and validate paths, including HTTP/3 listener metadata"
    "rginx-http|transport, proxy, TLS, and HTTP/3 runtime unit coverage"
    "rginx-runtime|reload/restart orchestration and listener bootstrap coverage"
    "rginx-observability|logging and tracing setup"
)

for entry in "${matrix[@]}"; do
    crate_name="${entry%%|*}"
    label="${entry#*|}"
    log "running ${crate_name}: ${label}"
    cargo test -p "${crate_name}" --lib --locked --quiet -- --test-threads=1
done

log "running rginx binary unit tests"
cargo test -p rginx --bin rginx --locked --quiet -- --test-threads=1
