# Architecture Remediation Release Note

Updated: `2026-04-11`

## Summary

This release closes the architecture remediation plan tracked in
`ARCHITECTURE_REMEDIATION_PLAN.md`.

The work was completed in six phases:

1. regression baseline
2. correctness fixes
3. control-plane output cleanup
4. runtime model cleanup
5. host/SNI/diagnostic semantic unification
6. response pipeline hardening and release validation

## User-Visible Outcomes

- explicit multi-listener deployments now honor listener-local `access_log_format`
- host routing now prefers:
  - exact hostnames over wildcard hostnames
  - more specific wildcards over less specific wildcards
- TLS SNI selection and HTTP host routing now follow the same best-match rules
- TLS diagnostic output now reflects runtime default-certificate behavior more accurately
- `status`, `snapshot`, and `check` no longer collapse multi-listener state into a single pseudo-listener view
- response finalization order is now explicit for:
  - compression
  - HEAD body stripping
  - gRPC observability wrapping
  - upstream idle/deadline body wrappers

## Internal Structural Outcomes

- `ConfigSnapshot.server` was removed from the compiled runtime model
- compiled runtime state now relies on explicit listeners plus default-vhost/vhost ownership
- control-plane status data now exposes real listener inventory
- admin snapshot schema was incremented from `10` to `11`
- duplicated TLS SNI diagnostic logic in `rginx-app` was removed in favor of reusing `rginx-http` runtime snapshot output

## Validation

Required gates completed successfully:

- `./scripts/test-fast.sh`
- `./scripts/test-slow.sh`
- `./scripts/run-tls-gate.sh`

Recommended extra gate also completed successfully:

- `./scripts/run-soak.sh --iterations 1`

Not run in this cycle:

- nginx comparison baseline via `scripts/nginx_compare/main.py`

## Compatibility Notes

- admin snapshot consumers must tolerate schema version `11`
- CLI `status` and `check` output changed from single `listen=` style summaries to listener inventory output:
  - `listen_addrs=...`
  - `status_listener ...`
  - `check_listener ...`

## Risk Status After Remediation

Closed:

- listener-local access log misapplication
- first-match host routing ambiguity
- stale `Content-Length` after compression fallback failure
- `Vary` overwrite during compression
- multi-listener control-plane misreporting
- drift between HTTP host matching, TLS SNI matching, and TLS diagnostics

Residual:

- full release-comparison benchmarking against nginx was not rerun in this validation pass
- broader performance characterization of the new response pipeline was not expanded beyond existing test and soak coverage
