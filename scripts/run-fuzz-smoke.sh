#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FUZZ_DIR="${ROOT_DIR}/fuzz"
source "${SCRIPT_DIR}/fuzz-common.sh"

SECONDS_PER_TARGET=10
EXPLICIT_TARGETS=()
USE_FULL_CORPUS=0
TEMP_ROOT=""

usage() {
    cat <<'EOF'
Usage: run-fuzz-smoke.sh [options]

Options:
  --seconds <n>
      Run each fuzz target for n seconds, default: 10
  --target <name>
      Run only the named target; may be passed multiple times
  --full-corpus
      Replay the full on-disk corpus instead of staging only versioned `.seed` inputs
  -h, --help
      Show help
EOF
}

log() {
    printf '[fuzz-smoke] %s\n' "$*"
}

die() {
    printf '[fuzz-smoke] error: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [[ -n "${TEMP_ROOT}" && -d "${TEMP_ROOT}" ]]; then
        rm -rf "${TEMP_ROOT}"
    fi
}

trap cleanup EXIT

while [[ $# -gt 0 ]]; do
    case "$1" in
        --seconds)
            [[ $# -ge 2 ]] || die "--seconds requires a value"
            SECONDS_PER_TARGET="$2"
            shift 2
            ;;
        --target)
            [[ $# -ge 2 ]] || die "--target requires a value"
            EXPLICIT_TARGETS+=("$2")
            shift 2
            ;;
        --full-corpus)
            USE_FULL_CORPUS=1
            shift
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

[[ "${SECONDS_PER_TARGET}" =~ ^[0-9]+$ ]] || die "--seconds must be a positive integer"
[[ "${SECONDS_PER_TARGET}" -ge 1 ]] || die "--seconds must be >= 1"

command -v cargo >/dev/null 2>&1 || die "cargo is required"
cargo fuzz --help >/dev/null 2>&1 || die "cargo-fuzz is not installed; run: cargo install cargo-fuzz"
rustup toolchain list | grep -q '^nightly' || die "nightly toolchain is not installed; run: rustup toolchain install nightly"

TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rginx-fuzz-smoke.XXXXXX")"

cd "${FUZZ_DIR}"

if [[ "${#EXPLICIT_TARGETS[@]}" -eq 0 ]]; then
    mapfile -t targets < <(cargo fuzz list)
else
    targets=("${EXPLICIT_TARGETS[@]}")
fi

[[ "${#targets[@]}" -ge 1 ]] || die "no fuzz targets found"

for target in "${targets[@]}"; do
    log "running ${target} for ${SECONDS_PER_TARGET}s"
    dict_path="${FUZZ_DIR}/dictionaries/${target}.dict"
    fuzz_args=("-max_total_time=${SECONDS_PER_TARGET}")
    if [[ "${USE_FULL_CORPUS}" -eq 1 ]]; then
        corpus_dir="${FUZZ_DIR}/corpus/${target}"
        [[ -d "${corpus_dir}" ]] || die "corpus directory does not exist: ${corpus_dir}"
        log "using full corpus ${corpus_dir}"
    else
        fuzz_stage_seed_corpus "${FUZZ_DIR}" "${target}" "${TEMP_ROOT}" corpus_dir \
            || die "no versioned seed corpus found for target ${target}"
        log "using staged seed corpus ${corpus_dir}"
    fi
    if [[ -f "${dict_path}" ]]; then
        log "using dictionary ${dict_path}"
        fuzz_args+=("-dict=${dict_path}")
    fi
    fuzz_load_target_options "${FUZZ_DIR}" "${target}" target_options
    if [[ "${#target_options[@]}" -gt 0 ]]; then
        log "using target options ${FUZZ_DIR}/options/${target}.options"
        fuzz_args+=("${target_options[@]}")
    fi
    fuzz_cmd=(cargo +nightly fuzz run "${target}" "${corpus_dir}")
    if [[ "${#fuzz_args[@]}" -gt 0 ]]; then
        fuzz_cmd+=(-- "${fuzz_args[@]}")
    fi
    "${fuzz_cmd[@]}"
done

log "fuzz smoke completed"
