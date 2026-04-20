#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

PROJECT_NAME="${RGINX_CONTROL_E2E_PROJECT_NAME:-rginx-control-console-e2e}"
API_ADDR="${RGINX_CONTROL_E2E_API_ADDR:-127.0.0.1:18180}"
POSTGRES_PUBLISH="${RGINX_CONTROL_E2E_POSTGRES_PUBLISH:-127.0.0.1:15433}"
DRAGONFLY_PUBLISH="${RGINX_CONTROL_E2E_DRAGONFLY_PUBLISH:-127.0.0.1:16380}"
UI_DIR="${ROOT_DIR}/target/control-console-e2e-ui"

cleanup() {
    if [[ "${RGINX_CONTROL_E2E_KEEP_SERVICES:-0}" == "1" ]]; then
        return 0
    fi

    COMPOSE_PROJECT_NAME="${PROJECT_NAME}" \
        docker compose down -v --remove-orphans >/dev/null 2>&1 || true
}

wait_for_postgres() {
    for _attempt in $(seq 1 60); do
        if COMPOSE_PROJECT_NAME="${PROJECT_NAME}" docker compose exec -T postgres sh -lc \
            'export PGPASSWORD="${POSTGRES_PASSWORD}"; pg_isready -h 127.0.0.1 -p 5432 -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" >/dev/null'
        then
            return 0
        fi
        sleep 1
    done
    printf 'postgres service never became ready\n' >&2
    return 1
}

wait_for_dragonfly() {
    for _attempt in $(seq 1 60); do
        if COMPOSE_PROJECT_NAME="${PROJECT_NAME}" docker compose exec -T dragonfly sh -lc \
            'redis-cli -h 127.0.0.1 -p 6379 ping | grep PONG >/dev/null'
        then
            return 0
        fi
        sleep 1
    done
    printf 'dragonfly service never became ready\n' >&2
    return 1
}

trap cleanup EXIT INT TERM

cd "${ROOT_DIR}"

rustup target add wasm32-unknown-unknown >/dev/null
cargo build -p rginx-control-console --target wasm32-unknown-unknown >/dev/null
rm -rf "${UI_DIR}"
mkdir -p "${UI_DIR}"
wasm-bindgen \
    --target web \
    --no-typescript \
    --out-dir "${UI_DIR}" \
    --out-name console \
    "${ROOT_DIR}/target/wasm32-unknown-unknown/debug/rginx_control_console.wasm"
cp "${ROOT_DIR}/crates/rginx-control-console/static/index.html" "${UI_DIR}/index.html"
cp "${ROOT_DIR}/crates/rginx-control-console/static/console.css" "${UI_DIR}/console.css"

export COMPOSE_PROJECT_NAME="${PROJECT_NAME}"
export RGINX_CONTROL_POSTGRES_PUBLISH="${POSTGRES_PUBLISH}"
export RGINX_CONTROL_DRAGONFLY_PUBLISH="${DRAGONFLY_PUBLISH}"

docker compose up -d postgres dragonfly >/dev/null
wait_for_postgres
wait_for_dragonfly

export RGINX_CONTROL_DB_HOST="127.0.0.1"
export RGINX_CONTROL_DB_PORT="${POSTGRES_PUBLISH##*:}"
export RGINX_CONTROL_DB_USER="rginx"
export RGINX_CONTROL_DB_PASSWORD="rginx"
export RGINX_CONTROL_DB_NAME="rginx_control"
export RGINX_CONTROL_DRAGONFLY_HOST="127.0.0.1"
export RGINX_CONTROL_DRAGONFLY_PORT="${DRAGONFLY_PUBLISH##*:}"
export RGINX_CONTROL_DRAGONFLY_KEY_PREFIX="rginx:control"
export RGINX_CONTROL_API_ADDR="${API_ADDR}"
export RGINX_CONTROL_UI_DIR="${UI_DIR}"
export RGINX_CONTROL_AUTH_SESSION_SECRET="change-me-for-local-e2e"
export RGINX_CONTROL_AGENT_SHARED_TOKEN="change-me-for-node-agent"
export RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS="1"
export RGINX_CONTROL_DNS_UDP_ADDR=""
export RGINX_CONTROL_DNS_TCP_ADDR=""

exec cargo run -p rginx-web
