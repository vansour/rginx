#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

ITERATIONS=1
RELEASE=0

usage() {
    cat <<'EOF'
Usage: run-soak.sh [options]

Options:
  --iterations <n>
      重复执行整套 soak 矩阵的次数，默认 1
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
    cargo_args=(cargo test --release --locked -p rginx)
else
    cargo_args=(cargo test --locked -p rginx)
fi

matrix=(
    "phase1|HTTP/1.1 request ID and plain proxy path"
    "http2|TLS termination and inbound HTTP/2"
    "upstream_http2|HTTPS upstream ALPN HTTP/2"
    "grpc_proxy|gRPC and grpc-web proxy path"
    "upgrade|Upgrade and WebSocket tunnel path"
    "reload|reload and restart handoff stability"
    "dns_refresh|hostname upstream DNS refresh"
    "proxy_protocol|inbound PROXY protocol client IP chain"
)

for iteration in $(seq 1 "${ITERATIONS}"); do
    printf '[soak] iteration %s/%s\n' "${iteration}" "${ITERATIONS}"
    for entry in "${matrix[@]}"; do
        test_name="${entry%%|*}"
        label="${entry#*|}"
        printf '[soak] running %-18s %s\n' "${test_name}" "${label}"
        "${cargo_args[@]}" --test "${test_name}" -- --nocapture
    done
done

printf '[soak] completed %s iteration(s)\n' "${ITERATIONS}"
