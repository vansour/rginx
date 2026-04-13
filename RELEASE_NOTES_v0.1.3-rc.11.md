# rginx v0.1.3-rc.11

Updated: `2026-04-13`

## Summary

`v0.1.3-rc.11` is an HTTP/3 maturity and release-discipline candidate.

This release candidate completes the repository's staged HTTP/3 nginx-alignment
plan through Phase 7 and moves HTTP/3 from "feature present" to
"release-gated and operator-visible".

## Highlights

### HTTP/3 0-RTT and Replay Safety

- downstream HTTP/3 early data is now opt-in via `server.http3.early_data`
- replay-safe routing is explicit via `location.allow_early_data`
- replay-unsafe routes default to conservative rejection with
  `425 Too Early`
- early-data state is exposed in:
  - `check`
  - `status`
  - `snapshot`
  - `counters`

### HTTP/3 / QUIC Runtime Telemetry

- listener-level HTTP/3 runtime telemetry now reports:
  - active HTTP/3 connections
  - active HTTP/3 request streams
  - Retry issued and failed totals
  - request accept and resolve errors
  - request-body and response-stream errors
  - connection close reason totals
- telemetry is available through:
  - `status_listener_http3`
  - `traffic_listener_http3`
  - `snapshot.status.listeners[].http3_runtime`
  - `snapshot.traffic.listeners[].http3_runtime`

### HTTP/3 Release Gate Assets

- added focused HTTP/3 soak runner:
  - `scripts/run-http3-soak.sh`
- added HTTP/3 release wrapper:
  - `scripts/run-http3-release-gate.sh`
- release-prep and GitHub release workflow now execute the HTTP/3 release gate
- added release-gate documentation:
  - `HTTP3_PHASE7_RELEASE.md`

### Documentation Cleanup

- repository HTTP/3 plan is now centered on:
  - `ARCHITECTURE_HTTP3_NGINX_ALIGNMENT_PLAN.md`
  - `HTTP3_PHASE0_BASELINE.md`
  - `HTTP3_PHASE7_RELEASE.md`
- obsolete interim HTTP/3 remediation/planning docs were removed

## Operator Notes

### Snapshot Schema

- admin snapshot schema version is now `13`

### New HTTP/3 Control-Plane Fields

- top-level status summary now includes:
  - `http3_active_connections`
  - `http3_active_request_streams`
  - `http3_retry_issued_total`
  - `http3_retry_failed_total`
  - `http3_request_accept_errors_total`
  - `http3_request_resolve_errors_total`
  - `http3_request_body_stream_errors_total`
  - `http3_response_stream_errors_total`
  - `http3_early_data_enabled_listeners`
  - `http3_early_data_accepted_requests`
  - `http3_early_data_rejected_requests`

## Validation Performed

Release preflight passed for `v0.1.3-rc.11` with:

- `cargo fmt --all --check`
- `./scripts/test-fast.sh`
- `./scripts/run-clippy-gate.sh`
- `./scripts/test-slow.sh`
- `./scripts/run-tls-gate.sh`
- `./scripts/run-http3-release-gate.sh --soak-iterations 1`
- `cargo run -p rginx -- --version`

## Known Limits

- token-validation failure telemetry and migration event telemetry are still
  constrained by the current QUIC backend surface
- the nginx comparison harness remains external to the normal test matrix and is
  still optional for prerelease preparation
