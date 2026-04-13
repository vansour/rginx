#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

ITERATIONS=1
RELEASE=0
NETEM_PROFILE="none"
NETEM_DEV="lo"
MTU=""

usage() {
    cat <<'EOF'
Usage: run-http3-soak.sh [options]

Focused HTTP/3 soak runner with optional Linux netem fault profiles.

Options:
  --iterations <n>
      Repeat the HTTP/3 soak matrix n times, default: 1
  --release
      Use cargo test --release
  --netem-profile <none|loss|reorder|jitter>
      Apply a Linux tc netem profile around each test target
  --netem-dev <iface>
      Interface for tc netem, default: lo
  --mtu <bytes>
      Temporarily force the interface MTU during each test run
  -h, --help
      Show help
EOF
}

log() {
    printf '[http3-soak] %s\n' "$*"
}

die() {
    printf '[http3-soak] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --iterations)
            [[ $# -ge 2 ]] || die "--iterations requires a value"
            ITERATIONS="$2"
            shift 2
            ;;
        --release)
            RELEASE=1
            shift
            ;;
        --netem-profile)
            [[ $# -ge 2 ]] || die "--netem-profile requires a value"
            NETEM_PROFILE="$2"
            shift 2
            ;;
        --netem-dev)
            [[ $# -ge 2 ]] || die "--netem-dev requires a value"
            NETEM_DEV="$2"
            shift 2
            ;;
        --mtu)
            [[ $# -ge 2 ]] || die "--mtu requires a value"
            MTU="$2"
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

[[ "${ITERATIONS}" =~ ^[0-9]+$ ]] || die "--iterations must be a positive integer"
[[ "${ITERATIONS}" -ge 1 ]] || die "--iterations must be >= 1"
[[ "${NETEM_PROFILE}" =~ ^(none|loss|reorder|jitter)$ ]] \
    || die "--netem-profile must be one of: none, loss, reorder, jitter"
if [[ -n "${MTU}" ]]; then
    [[ "${MTU}" =~ ^[0-9]+$ ]] || die "--mtu must be numeric"
    [[ "${MTU}" -ge 1280 ]] || die "--mtu must be >= 1280 for QUIC/HTTP/3"
fi

need_privileged=0
if [[ "${NETEM_PROFILE}" != "none" || -n "${MTU}" ]]; then
    need_privileged=1
fi
if [[ "${need_privileged}" -eq 1 ]]; then
    have tc || die "tc is required when --netem-profile is used"
    have ip || die "ip is required when --mtu is used"
    [[ "${EUID}" -eq 0 ]] || die "root privileges are required for netem/mtu operations"
fi

cd "${ROOT_DIR}"

if [[ "${RELEASE}" -eq 1 ]]; then
    cargo_args=(cargo test --release --locked -p rginx)
else
    cargo_args=(cargo test --locked -p rginx)
fi

matrix=(
    "http3|downstream HTTP/3 ingress, early-data policy, compression, access log"
    "upstream_http3|upstream HTTP/3 pooling and proxy path"
    "grpc_http3|gRPC, grpc-web, deadlines, and health checks over HTTP/3"
    "reload|HTTP/3 reload, restart, and drain semantics"
    "admin|HTTP/3 control-plane telemetry and snapshot output"
    "check|HTTP/3 listener validation and reporting"
    "ocsp|HTTP/3 TLS / OCSP runtime state"
)

orig_mtu=""

clear_faults() {
    if [[ "${need_privileged}" -eq 1 ]]; then
        tc qdisc del dev "${NETEM_DEV}" root >/dev/null 2>&1 || true
        if [[ -n "${orig_mtu}" ]]; then
            ip link set dev "${NETEM_DEV}" mtu "${orig_mtu}" >/dev/null 2>&1 || true
        fi
    fi
}

trap clear_faults EXIT

apply_faults() {
    clear_faults
    if [[ -z "${orig_mtu}" && "${need_privileged}" -eq 1 ]]; then
        orig_mtu="$(ip -o link show dev "${NETEM_DEV}" | awk '{for (i = 1; i <= NF; i++) if ($i == "mtu") { print $(i + 1); exit }}')"
    fi
    case "${NETEM_PROFILE}" in
        none)
            ;;
        loss)
            tc qdisc replace dev "${NETEM_DEV}" root netem delay 15ms 5ms loss 1.5%
            ;;
        reorder)
            tc qdisc replace dev "${NETEM_DEV}" root netem delay 25ms 8ms reorder 10% 50%
            ;;
        jitter)
            tc qdisc replace dev "${NETEM_DEV}" root netem delay 40ms 20ms distribution normal
            ;;
    esac
    if [[ -n "${MTU}" ]]; then
        ip link set dev "${NETEM_DEV}" mtu "${MTU}"
    fi
}

for iteration in $(seq 1 "${ITERATIONS}"); do
    log "iteration ${iteration}/${ITERATIONS}"
    for entry in "${matrix[@]}"; do
        test_name="${entry%%|*}"
        label="${entry#*|}"
        if [[ "${need_privileged}" -eq 1 ]]; then
            log "applying profile=${NETEM_PROFILE} dev=${NETEM_DEV} mtu=${MTU:--}"
            apply_faults
        fi
        log "running ${test_name}: ${label}"
        "${cargo_args[@]}" --test "${test_name}" -- --nocapture --test-threads=1
        if [[ "${need_privileged}" -eq 1 ]]; then
            clear_faults
        fi
    done
done

log "HTTP/3 soak completed"
