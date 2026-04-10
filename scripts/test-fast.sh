#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

log() {
    printf '[test-fast] %s\n' "$*"
}

cd "${ROOT_DIR}"

log "running fast unit and crate-local tests"
cargo test -p rginx-core --lib --locked --quiet -- --test-threads=1
cargo test -p rginx-config --lib --locked --quiet -- --test-threads=1
cargo test -p rginx-http --lib --locked --quiet -- --test-threads=1
cargo test -p rginx-runtime --lib --locked --quiet -- --test-threads=1
cargo test -p rginx-observability --lib --locked --quiet -- --test-threads=1
cargo test -p rginx --bin rginx --locked --quiet -- --test-threads=1
