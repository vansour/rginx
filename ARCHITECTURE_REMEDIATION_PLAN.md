# Architecture Remediation Plan

Updated: `2026-04-11`

## Purpose

This document turns the current architecture review and risk list into an execution plan.

The plan is intentionally staged:

- fix correctness issues before reshaping core models
- add regression coverage before changing semantics
- keep user-visible behavior changes explicit
- avoid mixing data-plane fixes with large control-plane refactors

## Starting Point

The current review produced five main concerns:

1. multi-listener `access_log_format` is effectively read from a single global server view
2. vhost selection is first-match-by-order instead of exact-or-more-specific-first
3. compression fallback can return an empty body on collection failure
4. compression overwrites existing `Vary` semantics instead of merging them
5. the runtime/control-plane model still carries a legacy single-server assumption even after explicit multi-listener support landed

These problems are not all equal. The first four are correctness issues. The fifth is a structural issue that is already leaking into correctness and observability.

## Delivery Strategy

Recommended delivery split:

1. Batch 1: Phase 0 and Phase 1
2. Batch 2: Phase 2, Phase 3, and Phase 4
3. Batch 3: Phase 5 and Phase 6

Each batch should be mergeable on its own.

## Phase 0: Regression Baseline

Goal:

- lock down the intended semantics before fixing implementation

Scope:

- add targeted unit tests around request dispatch, vhost selection, and compression
- add targeted integration tests around multi-listener logging and host/wildcard routing

Primary files:

- `crates/rginx-http/src/handler/dispatch.rs`
- `crates/rginx-http/src/router.rs`
- `crates/rginx-http/src/compression.rs`
- `crates/rginx-app/tests/`

Required coverage:

- listener-specific `access_log_format` is honored per listener
- `api.example.com` beats `*.example.com`
- more-specific wildcard beats less-specific wildcard
- HTTP host routing and TLS SNI certificate selection stay aligned
- compression failure preserves the original response body
- `Vary` keeps pre-existing values while adding `Accept-Encoding`

Exit criteria:

- the new tests reproduce the current issues on the pre-fix branch
- the new tests become stable gates for later phases

## Phase 1: Correctness Fixes

Goal:

- fix deterministic behavior bugs without changing core configuration models yet

Scope:

- make access-log formatting listener-aware
- change vhost selection from declaration-order match to best-match selection
- fix compression fallback and `Vary` handling

Primary files:

- `crates/rginx-http/src/handler/dispatch.rs`
- `crates/rginx-http/src/router.rs`
- `crates/rginx-core/src/config.rs`
- `crates/rginx-http/src/compression.rs`

Implementation notes:

- read `access_log_format` from the current listener context, not from the legacy global server snapshot
- define a deterministic host-match ordering:
  - exact host first
  - then the most specific wildcard
  - then default vhost
- on compression collect failure, return the original response path instead of synthesizing an empty body
- merge `Vary: Accept-Encoding` with any existing `Vary` value

Exit criteria:

- all Phase 0 tests pass
- no control-plane schema changes yet

## Phase 2: Control-Plane Output Cleanup

Goal:

- remove incorrect single-listener reporting from status, check, and admin output

Scope:

- audit all reads of `config.server.*` in runtime status and CLI reporting
- replace single `listen_addr` reporting with listener-oriented reporting
- update any affected admin output format and tests

Primary files:

- `crates/rginx-http/src/state/lifecycle.rs`
- `crates/rginx-http/src/state/snapshots.rs`
- `crates/rginx-app/src/main.rs`
- `crates/rginx-app/src/admin_cli/status.rs`
- `crates/rginx-app/tests/admin/`
- `crates/rginx-app/tests/check.rs`

Key decision:

- decide whether this is a backwards-compatible extension or a snapshot-schema change

Exit criteria:

- explicit multi-listener configs are reported accurately by `check`, admin status, and snapshots
- no reporting path depends on "the first listener" as a proxy for the whole runtime

## Phase 3: ConfigSnapshot Model Refactor

Goal:

- remove the structural ambiguity that keeps reintroducing single-server behavior

Scope:

- redefine the role of `ConfigSnapshot.server`
- separate default-vhost semantics from listener-local server settings
- migrate remaining call sites away from legacy global-server access

Primary files:

- `crates/rginx-core/src/config.rs`
- `crates/rginx-config/src/compile/mod.rs`
- `crates/rginx-config/src/compile/server.rs`
- `crates/rginx-http/src/handler/dispatch.rs`
- `crates/rginx-http/src/state/`
- `crates/rginx-runtime/src/`

Recommended approach:

- do not delete fields blindly
- first classify every current `config.server` read into one of:
  - listener-local property
  - default-vhost property
  - actual global runtime property
- then migrate each call site to the correct owner

Exit criteria:

- there is no ambiguous "primary server" semantics left in the runtime model
- listener-local behavior is sourced from listener-local data

## Phase 4: Host and SNI Semantics Unification

Goal:

- make HTTP host routing, TLS certificate selection, and diagnostics follow the same matching rules

Scope:

- unify best-match rules across HTTP host selection and TLS SNI selection
- ensure diagnostics reflect the same effective winner that runtime selection uses

Primary files:

- `crates/rginx-http/src/router.rs`
- `crates/rginx-http/src/tls/sni.rs`
- `crates/rginx-http/src/state/tls_runtime/bindings.rs`
- `crates/rginx-http/src/state/tls_runtime/listeners.rs`

Semantics target:

- exact name beats wildcard
- more-specific wildcard beats less-specific wildcard
- no divergence between chosen certificate and chosen vhost for the same host

Exit criteria:

- HTTPS requests do not end up with "certificate selected from one rule, route selected from another"
- admin and check diagnostics reflect actual runtime semantics

## Phase 5: Response Transform Pipeline Hardening

Goal:

- make response mutation order explicit and safe across streaming, compression, grpc-web, HEAD, and timeout paths

Scope:

- audit the ordering of:
  - upstream response sanitization
  - grpc-web translation
  - timeout wrappers
  - compression
  - HEAD body stripping
  - gRPC observability finalization
- if necessary, introduce a more explicit response-transform layer

Primary files:

- `crates/rginx-http/src/handler/dispatch.rs`
- `crates/rginx-http/src/compression.rs`
- `crates/rginx-http/src/handler/grpc.rs`
- `crates/rginx-http/src/proxy/forward/response.rs`

Focus areas:

- avoid unexpected full-body buffering where streaming is expected
- preserve trailer semantics
- ensure HEAD handling does not corrupt accounting or transform logic
- make fallback paths deterministic

Exit criteria:

- response transformations are documented by code structure and tests
- streaming and non-streaming paths behave predictably under failure

## Phase 6: Full Validation and Release Gate

Goal:

- validate the refactor set as a release-quality change, not just a local fix

Required gates:

- `scripts/test-fast.sh`
- `scripts/test-slow.sh`
- `scripts/run-tls-gate.sh`

Recommended extra gates after Phase 3 or later:

- `scripts/run-soak.sh`
- nginx comparison baseline through `scripts/nginx_compare/main.py`

Required output:

- short release note summarizing:
  - correctness fixes
  - control-plane/reporting changes
  - config/runtime model changes
  - any admin snapshot compatibility impact

Exit criteria:

- all gates pass
- no unresolved snapshot-schema ambiguity remains
- no known divergence remains between listener semantics, host routing semantics, and TLS SNI semantics

## PR Breakdown

### PR 1

- Phase 0
- Phase 1

Why:

- highest-value bug fixes
- lowest refactor risk
- fastest path to real user-visible correctness improvement

### PR 2

- Phase 2
- Phase 3
- Phase 4

Why:

- all three deal with ownership and selection semantics
- these changes should be reviewed together because they reshape the model boundary

### PR 3

- Phase 5
- Phase 6

Why:

- response pipeline hardening is easier to review after model cleanup settles
- final validation should run on the near-final shape

## Non-Goals

This plan does not try to solve unrelated roadmap work such as:

- new protocol support
- new admin features unrelated to the reviewed risks
- broad performance tuning not connected to the identified correctness issues
- aesthetic refactors without semantic payoff

## Final Priority Order

1. regression coverage
2. correctness fixes
3. control-plane accuracy
4. configuration model cleanup
5. host and SNI semantic unification
6. response pipeline hardening
7. full release validation
