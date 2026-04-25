#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FUZZ_DIR="${ROOT_DIR}/fuzz"
source "${SCRIPT_DIR}/fuzz-common.sh"

TARGET=""
FORMAT="both"
CORPUS_DIR=""
OUT_DIR=""
LLVM_BIN_DIR=""
USE_FULL_CORPUS=0
TEMP_ROOT=""

usage() {
    cat <<'EOF'
Usage: run-fuzz-coverage.sh --target <name> [options]

Options:
  --target <name>
      Fuzz target to analyze
  --format <text|html|both>
      Coverage output format, default: both
  --corpus <dir>
      Corpus directory to replay
  --out-dir <dir>
      Output directory, default: fuzz/coverage/<target>
  --llvm-bin-dir <dir>
      Directory containing llvm-cov and llvm-profdata
  --full-corpus
      Replay the full on-disk corpus instead of staging only versioned `.seed` inputs
  -h, --help
      Show help
EOF
}

log() {
    printf '[fuzz-coverage] %s\n' "$*"
}

die() {
    printf '[fuzz-coverage] error: %s\n' "$*" >&2
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
        --target)
            [[ $# -ge 2 ]] || die "--target requires a value"
            TARGET="$2"
            shift 2
            ;;
        --format)
            [[ $# -ge 2 ]] || die "--format requires a value"
            FORMAT="$2"
            shift 2
            ;;
        --corpus)
            [[ $# -ge 2 ]] || die "--corpus requires a value"
            CORPUS_DIR="$2"
            shift 2
            ;;
        --out-dir)
            [[ $# -ge 2 ]] || die "--out-dir requires a value"
            OUT_DIR="$2"
            shift 2
            ;;
        --llvm-bin-dir)
            [[ $# -ge 2 ]] || die "--llvm-bin-dir requires a value"
            LLVM_BIN_DIR="$2"
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

[[ -n "${TARGET}" ]] || die "--target is required"
case "${FORMAT}" in
    text|html|both) ;;
    *)
        die "--format must be one of: text, html, both"
        ;;
esac

command -v cargo >/dev/null 2>&1 || die "cargo is required"
cargo fuzz --help >/dev/null 2>&1 || die "cargo-fuzz is not installed; run: cargo install cargo-fuzz"
rustup toolchain list | grep -q '^nightly' || die "nightly toolchain is not installed; run: rustup toolchain install nightly"

HOST_TRIPLE="$(rustup run nightly rustc -vV | sed -n 's|host: ||p')"
[[ -n "${HOST_TRIPLE}" ]] || die "failed to detect nightly host triple"

if [[ -z "${LLVM_BIN_DIR}" ]]; then
    SYSROOT="$(rustup run nightly rustc --print sysroot)"
    LLVM_BIN_DIR="${SYSROOT}/lib/rustlib/${HOST_TRIPLE}/bin"
fi

[[ -x "${LLVM_BIN_DIR}/llvm-cov" ]] || die "llvm-cov not found in ${LLVM_BIN_DIR}"
[[ -x "${LLVM_BIN_DIR}/llvm-profdata" ]] || die "llvm-profdata not found in ${LLVM_BIN_DIR}"

if [[ -z "${CORPUS_DIR}" ]]; then
    if [[ "${USE_FULL_CORPUS}" -eq 1 ]]; then
        CORPUS_DIR="${FUZZ_DIR}/corpus/${TARGET}"
        [[ -d "${CORPUS_DIR}" ]] || die "corpus directory does not exist: ${CORPUS_DIR}"
        log "using full corpus ${CORPUS_DIR}"
    else
        TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rginx-fuzz-coverage.XXXXXX")"
        fuzz_stage_seed_corpus "${FUZZ_DIR}" "${TARGET}" "${TEMP_ROOT}" CORPUS_DIR \
            || die "no versioned seed corpus found for target ${TARGET}"
        log "using staged seed corpus ${CORPUS_DIR}"
    fi
fi
[[ -d "${CORPUS_DIR}" ]] || die "corpus directory does not exist: ${CORPUS_DIR}"

if [[ -z "${OUT_DIR}" ]]; then
    OUT_DIR="${FUZZ_DIR}/coverage/${TARGET}"
fi
mkdir -p "${OUT_DIR}"

log "generating coverage profile for ${TARGET}"
fuzz_load_target_options "${FUZZ_DIR}" "${TARGET}" target_options
if [[ "${#target_options[@]}" -gt 0 ]]; then
    log "using target options ${FUZZ_DIR}/options/${TARGET}.options"
fi
(
    cd "${FUZZ_DIR}"
    coverage_cmd=(cargo +nightly fuzz coverage "${TARGET}" "${CORPUS_DIR}")
    if [[ "${#target_options[@]}" -gt 0 ]]; then
        coverage_cmd+=(-- "${target_options[@]}")
    fi
    "${coverage_cmd[@]}"
)

PROFDATA="${FUZZ_DIR}/coverage/${TARGET}/coverage.profdata"
[[ -f "${PROFDATA}" ]] || die "expected profdata was not produced: ${PROFDATA}"

BINARY_PATH="${FUZZ_DIR}/target/${HOST_TRIPLE}/coverage/${HOST_TRIPLE}/release/${TARGET}"
if [[ ! -x "${BINARY_PATH}" ]]; then
    BINARY_PATH="$(find "${FUZZ_DIR}/target/${HOST_TRIPLE}/coverage" -path "*/release/${TARGET}" -type f | head -n 1)"
fi
[[ -n "${BINARY_PATH}" && -x "${BINARY_PATH}" ]] || die "coverage instrumented binary not found for ${TARGET}"

IGNORE_REGEX='(^/rustc/|(^|/)[.]rustup/toolchains/.*/lib/rustlib/src/rust/library/|/\.cargo/registry/)'

if [[ "${FORMAT}" == "text" || "${FORMAT}" == "both" ]]; then
    REPORT_PATH="${OUT_DIR}/report.txt"
    "${LLVM_BIN_DIR}/llvm-cov" report "${BINARY_PATH}" \
        -instr-profile="${PROFDATA}" \
        --ignore-filename-regex="${IGNORE_REGEX}" | tee "${REPORT_PATH}"
    log "text report written to ${REPORT_PATH}"
fi

if [[ "${FORMAT}" == "html" || "${FORMAT}" == "both" ]]; then
    HTML_DIR="${OUT_DIR}/html"
    rm -rf "${HTML_DIR}"
    "${LLVM_BIN_DIR}/llvm-cov" show "${BINARY_PATH}" \
        -instr-profile="${PROFDATA}" \
        --format=html \
        --output-dir="${HTML_DIR}" \
        --project-title="rginx fuzz coverage: ${TARGET}" \
        --ignore-filename-regex="${IGNORE_REGEX}" \
        --show-line-counts-or-regions >/dev/null
    log "html report written to ${HTML_DIR}/index.html"
fi
