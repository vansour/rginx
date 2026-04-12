# HTTP/3 Architecture Plan

Updated: `2026-04-12`

## Purpose

This document describes a staged plan for adding end-to-end HTTP/3 support to
`rginx`.

The target is broader than "accept HTTP/3 requests":

- downstream HTTP/3 ingress
- upstream HTTP/3 proxying
- consistent routing, policy, logging, and observability across HTTP/1.1,
  HTTP/2, and HTTP/3
- explicit reload and restart semantics for HTTP/3 listeners

The plan intentionally avoids mixing transport enablement with large semantic
changes in a single batch.

## Status

- Phase 0 is complete.
- Phase 1 is complete.
- Phase 2 is complete.
- Phase 3 is complete.
- Phase 4 is complete.
- Phase 5 is complete.
- Phase 6 is complete.
- Phase 7 is complete.
- Phase 8 is complete.
- The frozen first-release semantics live in
  `ARCHITECTURE_HTTP3_PHASE0_FREEZE.md`.
- The staged HTTP/3 delivery plan is complete.

## Current Architectural Constraints

The current codebase is organized around two assumptions that must be relaxed
before HTTP/3 can land cleanly:

1. one listener is effectively one TCP bind target
2. request and response plumbing is tightly coupled to Hyper
   `Incoming`-centric HTTP/1.1 and HTTP/2 paths

These assumptions currently leak into:

- config model and validation
- runtime listener bootstrap and restart handoff
- request-body preparation
- upstream client selection
- status and admin reporting
- reload-boundary planning

HTTP/3 should therefore be implemented as an explicit transport extension, not
as an ad hoc patch on the current TCP-only path.

## Guiding Principles

- keep the existing HTTP/1.1 and HTTP/2 paths stable while HTTP/3 is added in
  parallel
- separate transport refactors from protocol-semantics work
- do not promise feature parity for every advanced protocol feature in the
  first delivery
- make control-plane reporting accurate before shipping user-visible behavior
- treat reload and restart behavior as first-class architecture work, not as a
  postscript

## Scope Target

The intended final state is:

- downstream HTTP/1.1, HTTP/2, and HTTP/3 on the same logical listener model
- upstream HTTP/1.1, HTTP/2, and HTTP/3 proxy targets
- shared routing, access control, rate limiting, access logging, counters, and
  traffic snapshots across all downstream transports
- shared peer selection, passive health, active health, TLS policy, and
  upstream stats across all upstream transports

The initial HTTP/3 delivery should not include:

- cleartext `h3c`
- WebTransport
- extended CONNECT tunneling beyond what is required for a conservative first
  release
- a promise of websocket-over-HTTP/3 parity in the first batch

## Recommended Transport Stack

Recommended implementation stack:

- QUIC transport: `quinn`
- HTTP/3 protocol: `h3`
- Quinn adapter: `h3-quinn`

Rationale:

- the current `hyper` APIs used in `rginx` are centered on HTTP/1.1 and HTTP/2
- HTTP/3 support should be introduced as a parallel transport path rather than
  waiting for a hypothetical drop-in `hyper` migration
- this keeps the existing data plane stable while allowing a transport-neutral
  internal request pipeline to emerge

## Delivery Strategy

Recommended delivery split:

1. Batch 1: Phase 0, Phase 1, and Phase 2
2. Batch 2: Phase 3 and Phase 4
3. Batch 3: Phase 5 and Phase 6
4. Batch 4: Phase 7 and Phase 8

Each batch should be mergeable and testable on its own.

## Phase 0: Semantics Freeze

Goal:

- define the first supported HTTP/3 surface area before code changes begin

Scope:

- write down explicit first-release semantics
- define what is and is not included in the first HTTP/3 milestone
- define the downstream discovery strategy for clients

First-release recommendations:

- downstream HTTP/3 only over TLS
- no cleartext `h3c`
- downstream `Return` and `Proxy` handlers supported
- upstream HTTP/3 supported only through explicit upstream protocol
  configuration
- client discovery via `Alt-Svc`
- HTTPS RR support deferred to a later enhancement
- websocket-over-HTTP/3 and extended CONNECT deferred

Exit criteria:

- architecture notes, config semantics, and release scope are frozen
- later phases can rely on a stable HTTP/3 target

Completion status:

- completed in `ARCHITECTURE_HTTP3_PHASE0_FREEZE.md`

## Phase 1: Transport-Neutral Internal HTTP Pipeline

Goal:

- decouple request and response handling from transport-specific Hyper types

Scope:

- isolate Hyper HTTP/1.1 and HTTP/2 adaptation at the server edge
- reshape internal handler and proxy entrypoints to work on transport-neutral
  body and response abstractions
- reduce direct dependence on `Incoming` and `Response<Incoming>` outside the
  transport adapters

Primary files:

- `crates/rginx-http/src/handler/mod.rs`
- `crates/rginx-http/src/handler/dispatch.rs`
- `crates/rginx-http/src/proxy/request_body.rs`
- `crates/rginx-http/src/proxy/forward/mod.rs`
- `crates/rginx-http/src/proxy/forward/response.rs`
- `crates/rginx-http/src/server/connection.rs`
- `crates/rginx-http/src/server/graceful.rs`

Implementation notes:

- keep Hyper-specific adaptation near the accept and connection layers
- keep the existing H1 and H2 behavior unchanged while refactoring
- do not couple the internal pipeline to Quinn or `h3` yet

Exit criteria:

- HTTP/1.1 and HTTP/2 continue to pass existing tests
- handler and proxy internals are no longer shaped primarily by Hyper
  `Incoming`

Completion status:

- completed by pushing Hyper request-body adaptation to `server/graceful.rs`
- completed by changing internal handler and proxy entrypoints to use
  `HttpBody`
- completed by changing upstream response finalization to consume
  transport-neutral boxed bodies

## Phase 2: Listener and Control-Plane Model Upgrade

Goal:

- replace the implicit TCP-only listener model with an explicit transport-aware
  listener model

Scope:

- allow one logical listener to own multiple transport bindings
- represent TCP and UDP bind points explicitly
- expose HTTP/3 state in `check`, `status`, `snapshot`, and admin output
- define reload and restart boundaries for new HTTP/3 fields

Primary files:

- `crates/rginx-core/src/config.rs`
- `crates/rginx-config/src/model.rs`
- `crates/rginx-config/src/validate/server.rs`
- `crates/rginx-config/src/compile/server.rs`
- `crates/rginx-http/src/transition.rs`
- `crates/rginx-http/src/state/snapshots.rs`
- `crates/rginx-app/src/main.rs`
- `crates/rginx-app/src/admin_cli/status.rs`
- `crates/rginx-runtime/src/bootstrap/listeners.rs`

Recommended config direction:

- add explicit HTTP/3 listener settings rather than overloading the existing
  `listen` field
- model UDP bind settings and QUIC transport settings explicitly
- keep legacy single-listener configuration working

Exit criteria:

- the runtime can describe listener transport inventory accurately
- restart-boundary planning no longer assumes every listener is only TCP

Completion status:

- completed by adding explicit listener-level `http3` configuration metadata
- completed by exposing transport-aware listener bindings in compiled runtime
  state
- completed by updating `check`, `status`, and snapshot output to report TCP and
  UDP listener bindings
- completed by defining HTTP/3-specific reload and restart boundary fields

## Phase 3: Downstream HTTP/3 Minimum Viable Path

Goal:

- accept downstream HTTP/3 requests and route them through the existing data
  plane

Scope:

- add QUIC endpoint bootstrap and accept loops
- terminate HTTP/3 requests and feed them into the transport-neutral internal
  handler path
- support `Return` and basic `Proxy`
- expose HTTP/3 readiness to clients through `Alt-Svc`

Primary files:

- `crates/rginx-runtime/src/bootstrap/`
- `crates/rginx-http/src/server/`
- new HTTP/3 transport modules under `crates/rginx-http/src/`
- `crates/rginx-http/src/handler/dispatch.rs`

Implementation notes:

- keep HTTP/3 as a parallel listener path, not a replacement for H1 and H2
- do not block this phase on advanced gRPC-over-H3 support
- keep QUIC transport configuration conservative in the first pass

Exit criteria:

- a configured listener can serve HTTP/3 requests
- host routing, path routing, `Return`, and basic upstream proxying work over
  HTTP/3
- clients can discover HTTP/3 via `Alt-Svc`

Completion status:

- completed by adding a downstream QUIC/HTTP/3 accept path alongside the
  existing Hyper HTTP/1.1 and HTTP/2 path
- completed by bridging incoming HTTP/3 request bodies into the existing
  transport-neutral handler pipeline
- completed by bridging internal `HttpResponse` values back onto HTTP/3
  response streams
- completed by advertising HTTP/3 availability through `Alt-Svc` on compatible
  TLS listeners

## Phase 4: Downstream HTTP/3 Feature Parity for Core Middleware

Goal:

- align HTTP/3 ingress behavior with the existing HTTP/1.1 and HTTP/2
  downstream feature set

Scope:

- downstream request size enforcement
- downstream request-body read timeout semantics
- downstream response idle timeout semantics
- access control and rate limiting
- request ID generation and propagation
- access logging and counters
- traffic snapshots and listener stats

Primary files:

- `crates/rginx-http/src/handler/access_log.rs`
- `crates/rginx-http/src/handler/dispatch.rs`
- `crates/rginx-http/src/rate_limit.rs`
- `crates/rginx-http/src/state/connections.rs`
- `crates/rginx-http/src/state/traffic.rs`
- `crates/rginx-http/src/state/lifecycle.rs`
- new HTTP/3 body and stream timeout adapters

Focus areas:

- keep the same routing and policy semantics across H1, H2, and H3
- decide which response transforms remain valid on HTTP/3
- ensure access logs and admin output record HTTP/3 traffic accurately

Exit criteria:

- HTTP/3 is part of the same policy and observability model as existing
  downstream transports

Completion status:

- completed by validating route access control over downstream HTTP/3
- completed by validating route rate limiting over downstream HTTP/3
- completed by validating response compression over downstream HTTP/3
- completed by validating request ID propagation and access log output over
  downstream HTTP/3
- completed by validating listener traffic accounting over downstream HTTP/3

## Phase 5: Upstream HTTP/3 Client and Proxy Path

Goal:

- proxy from downstream requests to upstream HTTP/3 targets

Scope:

- add upstream HTTP/3 client profiles and connection management
- support explicit `Http3` upstream protocol selection
- preserve existing peer selection, failover, and health-registry integration
- support upstream TLS validation, server-name override, and client identity
  configuration

Primary files:

- `crates/rginx-core/src/config/upstream.rs`
- `crates/rginx-config/src/model.rs`
- `crates/rginx-config/src/validate/upstream.rs`
- `crates/rginx-config/src/compile/upstream.rs`
- `crates/rginx-http/src/proxy/clients/mod.rs`
- new upstream HTTP/3 client modules under `crates/rginx-http/src/proxy/`
- `crates/rginx-http/src/proxy/forward/mod.rs`

Implementation notes:

- do not overload the existing `Auto` upstream protocol behavior in the first
  pass
- first ship explicit `protocol: Http3`
- keep the HTTP/1.1 and HTTP/2 upstream client cache stable

Exit criteria:

- an explicit HTTP/3 upstream can be selected and proxied successfully
- failover rules remain coherent for replayable requests

Completion status:

- completed by extending upstream protocol configuration with explicit
  `Http3`
- completed by adding a dedicated upstream HTTP/3 client path alongside the
  existing Hyper client path
- completed by reusing existing peer selection, passive health, failover, and
  upstream stats semantics for upstream HTTP/3 requests
- completed by validating upstream HTTP/3 proxying with both basic TLS and
  `server_name_override` plus client-identity configuration

## Phase 6: gRPC, grpc-web, and Active Health over HTTP/3

Goal:

- extend HTTP/3 support to the project’s gRPC-oriented features

Scope:

- gRPC over downstream HTTP/3
- gRPC over upstream HTTP/3
- deadline and trailer behavior over HTTP/3
- grpc-web compatibility review
- active gRPC health checks over HTTP/3

Primary files:

- `crates/rginx-http/src/handler/grpc.rs`
- `crates/rginx-http/src/proxy/forward/grpc.rs`
- `crates/rginx-http/src/proxy/grpc_web/`
- `crates/rginx-http/src/proxy/health.rs`
- `crates/rginx-http/src/proxy/health/grpc_health_codec.rs`

Focus areas:

- trailer handling and observability extraction
- grpc-web translation viability over downstream HTTP/3
- deadline semantics and timeout mapping
- parity between H2 gRPC and H3 gRPC diagnostics

Exit criteria:

- gRPC over HTTP/3 is no longer a separate experimental lane
- active gRPC health checks can target HTTP/3-capable upstreams

Completion status:

- completed by validating downstream gRPC over HTTP/3 against explicit HTTP/3
  upstreams, including gRPC response trailers
- completed by validating grpc-web binary proxying over downstream and upstream
  HTTP/3
- completed by validating `grpc-timeout` deadline handling over HTTP/3 upstream
  paths
- completed by extending active gRPC health checks to target explicit HTTP/3
  upstream peers
- completed by covering the Phase 6 behavior in dedicated `grpc_http3`
  integration tests

## Phase 7: TLS, QUIC Runtime Semantics, Reload, and Restart

Goal:

- make HTTP/3 transport lifecycle behavior explicit and operationally safe

Scope:

- QUIC listener TLS settings
- HTTP/3 certificate and SNI reporting
- OCSP and certificate diagnostics for HTTP/3 listeners
- UDP listener reload semantics
- graceful restart and listener inheritance for UDP sockets
- connection drain semantics for long-lived QUIC sessions

Primary files:

- `crates/rginx-http/src/tls/`
- `crates/rginx-http/src/state/tls_runtime/`
- `crates/rginx-runtime/src/bootstrap/listeners.rs`
- `crates/rginx-runtime/src/restart.rs`
- `crates/rginx-http/src/transition.rs`

Key decisions:

- which HTTP/3 listener fields are reloadable
- which HTTP/3 transport settings require restart
- how inherited UDP sockets are represented during graceful restart
- whether `Alt-Svc` changes are reloadable or restart-boundary state

Exit criteria:

- reload and restart behavior for HTTP/3 is documented by code and tests
- control-plane diagnostics reflect actual runtime behavior

Completion status:

- completed by extending TLS listener diagnostics to expose HTTP/3 listener
  bindings, QUIC/TLS version constraints, and `h3` ALPN state alongside
  existing certificate, SNI, and OCSP reporting
- completed by preserving explicit HTTP/3 UDP listener sockets across graceful
  restart handoff
- completed by rejecting new HTTP/3 handshakes during drain while allowing
  in-flight HTTP/3 requests to finish before listener shutdown completes
- completed by validating HTTP/3 restart handoff and listener-removal drain
  behavior in dedicated reload integration tests

## Phase 8: Validation, Benchmarking, and Release Gate

Goal:

- validate HTTP/3 as a supported project capability rather than an opt-in demo

Required coverage:

- crate-local unit tests for HTTP/3 config, routing, TLS, and proxy behavior
- integration tests for downstream HTTP/3
- integration tests for upstream HTTP/3
- integration tests for reload, restart, admin status, and access logs with
  HTTP/3 enabled

Required script updates:

- `scripts/test-fast.sh`
- `scripts/test-slow.sh`
- `scripts/run-tls-gate.sh`
- `scripts/run-soak.sh`
- `scripts/nginx_compare/`

Recommended new integration test groups:

- `http3`
- `upstream_http3`
- `grpc_http3`
- `reload_http3`
- `multi_listener_http3`
- `alt_svc`

Exit criteria:

- HTTP/3 is exercised by the normal project test and release gates
- benchmark and soak tooling includes HTTP/3 scenarios

Completion status:

- completed by making `scripts/test-fast.sh` and `scripts/test-slow.sh` treat
  HTTP/3 coverage as part of the default fast and slow validation paths
- completed by extending `scripts/run-tls-gate.sh` to include downstream,
  upstream, and gRPC-over-HTTP/3 regression suites
- completed by extending `scripts/run-soak.sh` to include downstream HTTP/3,
  upstream HTTP/3, and gRPC-over-HTTP/3 soak scenarios
- completed by extending `scripts/run-benchmark-matrix.py` with HTTP/3 and
  gRPC-over-HTTP/3 curl benchmark entries
- completed by extending `scripts/nginx_compare/` with an explicit rginx
  HTTP/3 benchmark scenario and an unsupported marker for nginx in the current
  harness

## Cross-Cutting Risks

### Risk 1: Transport Refactor Without Behavior Freeze

If HTTP/3 transport work begins before semantics are frozen, request handling,
gRPC behavior, and reload semantics will drift during implementation.

Mitigation:

- complete Phase 0 before transport work starts

### Risk 2: Hyper-Coupling Reappears During H3 Integration

If new code keeps bending HTTP/3 around Hyper-specific request types, the code
will become harder to reason about than it is today.

Mitigation:

- complete Phase 1 before serious HTTP/3 transport work
- keep transport adapters at the edge

### Risk 3: Listener Model Drift Between Runtime and Admin Output

HTTP/3 adds UDP transport and QUIC state; if listener reporting is not fixed
early, `check`, `status`, and snapshots will lie about runtime shape.

Mitigation:

- land Phase 2 before claiming first-class HTTP/3 support

### Risk 4: Reload and Restart Become Underspecified

The current restart flow is explicitly fd-inheritance-based. HTTP/3 will add
UDP socket inheritance and QUIC drain behavior, which must not be left implicit.

Mitigation:

- treat Phase 7 as release-critical, not optional cleanup

## Recommended Immediate Next Steps

1. Start Phase 8 by folding the HTTP/3 suites into the normal release gate and
   long-running validation scripts.
2. Add broader soak and benchmark coverage for downstream and upstream HTTP/3
   traffic profiles.
3. Tighten any remaining HTTP/3-specific release criteria around reload,
   restart, and access-log observability.

## Suggested First Milestone

Recommended first milestone:

- downstream HTTP/3 ingress for `Return` and basic `Proxy`
- `Alt-Svc` advertisement on compatible listeners
- no upstream HTTP/3 yet
- no gRPC-over-H3 parity requirement yet

This keeps the first shipping target valuable while avoiding premature
entanglement with the hardest protocol and lifecycle edges.
