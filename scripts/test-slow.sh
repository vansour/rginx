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
cargo test -p rginx --tests --locked --quiet -- --test-threads=1
