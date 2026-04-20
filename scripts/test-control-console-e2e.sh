#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
E2E_DIR="${ROOT_DIR}/crates/rginx-control-console/e2e"

cd "${E2E_DIR}"

if [[ ! -d node_modules ]]; then
    npm install
fi

if [[ ! -d "${HOME}/.cache/ms-playwright" ]]; then
    npx playwright install chromium
fi

npx playwright test "$@"
