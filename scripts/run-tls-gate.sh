#!/usr/bin/env bash
set -euo pipefail

cargo test -p rginx --locked \
  --test tls_policy \
  --test downstream_mtls \
  --test upstream_mtls \
  --test upstream_http2 \
  --test upstream_server_name \
  --test grpc_proxy \
  --test access_log \
  --test admin \
  --test check \
  --test reload \
  --test ocsp \
  -- --test-threads=1
