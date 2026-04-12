#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

cd "${ROOT_DIR}"

cargo test -p rginx --locked \
  --test tls_policy \
  --test downstream_mtls \
  --test upstream_mtls \
  --test upstream_http2 \
  --test http3 \
  --test upstream_http3 \
  --test grpc_http3 \
  --test upstream_server_name \
  --test grpc_proxy \
  --test access_log \
  --test admin \
  --test check \
  --test reload \
  --test ocsp \
  -- --test-threads=1
