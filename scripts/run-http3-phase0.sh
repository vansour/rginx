#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

RUN_GATE=1
RUN_SOAK=1
SOAK_ITERATIONS=1

usage() {
    cat <<'EOF'
Usage: run-http3-phase0.sh [options]

Phase 0 HTTP/3 baseline runner. By default it runs:
  1. the dedicated HTTP/3 regression gate
  2. a focused HTTP/3 soak subset

Options:
  --skip-gate
      Skip the dedicated HTTP/3 regression gate
  --skip-soak
      Skip the focused HTTP/3 soak subset
  --soak-iterations <n>
      Repeat the focused HTTP/3 soak subset n times, default: 1
  -h, --help
      Show help
EOF
}

log() {
    printf '[http3-phase0] %s\n' "$*"
}

die() {
    printf '[http3-phase0] error: %s\n' "$*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-gate)
            RUN_GATE=0
            shift
            ;;
        --skip-soak)
            RUN_SOAK=0
            shift
            ;;
        --soak-iterations)
            [[ $# -ge 2 ]] || die "--soak-iterations requires a value"
            SOAK_ITERATIONS="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

[[ "${SOAK_ITERATIONS}" =~ ^[0-9]+$ ]] || die "--soak-iterations must be a positive integer"
[[ "${SOAK_ITERATIONS}" -ge 1 ]] || die "--soak-iterations must be >= 1"

cd "${ROOT_DIR}"

if [[ "${RUN_GATE}" -eq 1 ]]; then
    log "running dedicated HTTP/3 regression gate"
    "${ROOT_DIR}/scripts/run-http3-gate.sh"
fi

if [[ "${RUN_SOAK}" -eq 1 ]]; then
    soak_matrix=(
        "http3|downstream HTTP/3 ingress and middleware parity"
        "upstream_http3|explicit upstream HTTP/3 proxy path"
        "grpc_http3|gRPC and grpc-web over HTTP/3"
        "reload|reload and restart handoff under HTTP/3-enabled runtime"
    )

    for iteration in $(seq 1 "${SOAK_ITERATIONS}"); do
        log "soak iteration ${iteration}/${SOAK_ITERATIONS}"
        for entry in "${soak_matrix[@]}"; do
            test_name="${entry%%|*}"
            label="${entry#*|}"
            log "soak ${test_name}: ${label}"
            cargo test -p rginx --locked --test "${test_name}" -- --nocapture --test-threads=1
        done
    done
fi

log "HTTP/3 Phase 0 baseline run completed"
