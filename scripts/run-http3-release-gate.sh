#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

RUN_GATE=1
RUN_SOAK=1
RUN_COMPARE=0
SOAK_ITERATIONS=3
RELEASE=0
NETEM_PROFILE="none"
NETEM_DEV="lo"
MTU=""
COMPARE_OUT_DIR="${ROOT_DIR}/target/http3-release/nginx-compare"
COMPARE_ARGS=()

usage() {
    cat <<'EOF'
Usage: run-http3-release-gate.sh [options] [-- compare-args...]

Phase 7 HTTP/3 release gate runner. By default it runs:
  1. the dedicated HTTP/3 regression gate
  2. the focused HTTP/3 soak matrix

Optional:
  3. the nginx comparison Docker harness

Options:
  --skip-gate
      Skip the dedicated HTTP/3 regression gate
  --skip-soak
      Skip the focused HTTP/3 soak matrix
  --with-compare
      Also run the nginx comparison Docker harness
  --soak-iterations <n>
      Repeat the HTTP/3 soak matrix n times, default: 3
  --release
      Use cargo test --release for the soak run
  --netem-profile <none|loss|reorder|jitter>
      Linux tc netem profile for the soak run
  --netem-dev <iface>
      Interface for tc netem, default: lo
  --mtu <bytes>
      Temporarily force the soak interface MTU
  --compare-out-dir <dir>
      Output directory for nginx comparison artifacts
  -h, --help
      Show help

Arguments after `--` are passed through to scripts/run-nginx-compare-docker.sh.
EOF
}

log() {
    printf '[http3-release] %s\n' "$*"
}

die() {
    printf '[http3-release] error: %s\n' "$*" >&2
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
        --with-compare)
            RUN_COMPARE=1
            shift
            ;;
        --soak-iterations)
            [[ $# -ge 2 ]] || die "--soak-iterations requires a value"
            SOAK_ITERATIONS="$2"
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
        --compare-out-dir)
            [[ $# -ge 2 ]] || die "--compare-out-dir requires a value"
            COMPARE_OUT_DIR="$2"
            shift 2
            ;;
        --)
            shift
            COMPARE_ARGS=("$@")
            break
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
    soak_args=(--iterations "${SOAK_ITERATIONS}" --netem-profile "${NETEM_PROFILE}" --netem-dev "${NETEM_DEV}")
    if [[ "${RELEASE}" -eq 1 ]]; then
        soak_args+=(--release)
    fi
    if [[ -n "${MTU}" ]]; then
        soak_args+=(--mtu "${MTU}")
    fi
    log "running focused HTTP/3 soak matrix"
    "${ROOT_DIR}/scripts/run-http3-soak.sh" "${soak_args[@]}"
fi

if [[ "${RUN_COMPARE}" -eq 1 ]]; then
    log "running nginx comparison harness"
    "${ROOT_DIR}/scripts/run-nginx-compare-docker.sh" \
        --out-dir "${COMPARE_OUT_DIR}" \
        -- "${COMPARE_ARGS[@]}"
fi

log "HTTP/3 release gate completed"
