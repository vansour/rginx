#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

log() {
    printf '[http3-gate] %s\n' "$*"
}

cd "${ROOT_DIR}"

log "running dedicated HTTP/3 regression gate"

matrix=(
    "http3|downstream HTTP/3 ingress, policy, compression, access log, Alt-Svc"
    "upstream_http3|upstream HTTP/3 proxying, SNI override, client identity"
    "grpc_http3|gRPC, grpc-web, deadlines, and active health checks over HTTP/3"
    "reload|HTTP/3 reload, restart, and drain semantics"
    "admin|snapshot, status, delta, and wait control plane"
    "check|config validation and runtime reporting for HTTP/3 listeners"
    "ocsp|dynamic OCSP refresh state for TLS-backed listeners"
)

for entry in "${matrix[@]}"; do
    test_name="${entry%%|*}"
    label="${entry#*|}"
    log "running ${test_name}: ${label}"
    cargo test -p rginx --test "${test_name}" --locked --quiet -- --test-threads=1
done

log "HTTP/3 gate completed"
