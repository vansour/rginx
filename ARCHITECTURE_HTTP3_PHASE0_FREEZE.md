# HTTP/3 Phase 0 Semantics Freeze

Updated: `2026-04-12`

Status: `completed`

## Purpose

This document freezes the intended semantics for the first HTTP/3 delivery
cycle before implementation work begins.

It is not a schema document and it is not a transport implementation design.
Its role is narrower:

- define the first supported HTTP/3 surface area
- define what is explicitly deferred
- define the initial client-discovery policy
- define which compatibility expectations later phases must preserve

Phase 1 and later phases should treat this file as a contract unless a later
document explicitly replaces it.

## Terminology

For clarity, this document uses three milestone labels:

- `M1`: first downstream HTTP/3 release
- `M2`: upstream HTTP/3 proxy support
- `M3`: HTTP/3 gRPC and advanced parity work

These labels are semantic groupings, not release numbers.

## Frozen Decisions

### 1. Activation Model

HTTP/3 is frozen as an explicit opt-in capability.

Implications:

- HTTP/3 must not be enabled implicitly just because a listener already has TLS
- HTTP/3 configuration is listener-scoped, not a global process toggle
- the exact config field names are deferred to Phase 2
- legacy single-listener configurations must keep working without silent HTTP/3
  activation

### 2. Transport and Discovery Model

The initial client-discovery strategy is frozen as `Alt-Svc`.

Implications:

- `h3c` is out of scope
- first-release HTTP/3 is TLS-only
- HTTP/3 discovery should be advertised from compatible HTTPS responses via
  `Alt-Svc`
- HTTPS resource records are deferred and are not required for M1
- DNS-level automatic discovery is not part of the first implementation target

### 3. Downstream Scope for M1

The first downstream HTTP/3 milestone is intentionally narrow.

Included in M1:

- ordinary downstream HTTP requests over HTTP/3
- `Return` handlers
- `Proxy` handlers
- the same host and path routing semantics used by existing HTTP/1.1 and
  HTTP/2 paths
- the same route access control semantics
- the same route rate-limit semantics
- request ID propagation and generation
- access logging integration
- counters and traffic snapshot integration

Explicitly not required in M1:

- downstream gRPC over HTTP/3
- downstream grpc-web over HTTP/3
- websocket-over-HTTP/3
- extended CONNECT support beyond what a later milestone explicitly approves
- WebTransport
- QUIC datagram application features

### 4. Upstream Scope Freeze

Upstream HTTP/3 is frozen as a later milestone, not part of M1.

When upstream HTTP/3 is introduced:

- it must be explicit, not silently folded into the current `Auto` behavior
- first support should require explicit upstream protocol selection
- current failover semantics remain unchanged:
  - retry only idempotent requests
  - retry only replayable requests

This avoids turning "support upstream HTTP/3" into "change every current
upstream selection rule at once".

### 5. Compatibility Expectations Across Transports

Later HTTP/3 phases are required to preserve these invariants:

- the same `Host` to vhost selection rules as current HTTP/1.1 and HTTP/2
- the same route priority rules
- the same route access-control decisions
- the same rate-limit decisions
- the same request ID semantics
- the same access-log field meanings
- the same traffic and admin accounting semantics, modulo transport-specific
  fields added later

This is a freeze on behavior, not on implementation details.

### 6. Control-Plane Expectations

HTTP/3 must not be declared supported while control-plane output still hides or
misreports transport state.

Before support is claimed:

- `check` must describe HTTP/3 listener state accurately
- `status` must describe HTTP/3 listener state accurately
- `snapshot` and `delta` must not collapse HTTP/3 into a TCP-only view

The exact output format is deferred, but accurate transport-aware reporting is
not optional.

### 7. Reload and Restart Expectations

Phase 0 freezes one negative rule:

- HTTP/3 listener lifecycle behavior must not be left implicit

Implications:

- reloadability of HTTP/3 fields is deferred, not assumed
- restart-boundary behavior for HTTP/3 listeners must be made explicit before
  release
- UDP socket inheritance and QUIC drain behavior are later design work, not an
  implementation footnote

### 8. Security and TLS Expectations

Phase 0 freezes these security expectations:

- first-release HTTP/3 is tied to TLS-enabled listeners
- HTTP/3 certificate selection must not diverge from the listener’s effective
  SNI and vhost rules
- any future HTTP/3 TLS diagnostics must reflect runtime behavior, not a
  synthetic TCP-only view

Deferred from M1 unless later phases explicitly add them:

- downstream mTLS over HTTP/3
- full OCSP runtime parity for HTTP/3 listeners
- advanced QUIC transport tuning exposure

## Deferred Items

The following are explicitly deferred beyond Phase 0 and must not be treated as
accidental commitments:

- downstream gRPC over HTTP/3
- downstream grpc-web over HTTP/3
- upstream HTTP/3 proxy support
- active HTTP/3 health checks
- downstream mTLS over HTTP/3
- websocket-over-HTTP/3
- WebTransport
- HTTPS RR support
- automatic `Auto`-mode upgrade to upstream HTTP/3

## First Milestone Contract

The frozen first milestone is:

- downstream HTTP/3 ingress
- explicit opt-in listener configuration
- `Alt-Svc`-based discovery
- `Return` and basic `Proxy`
- parity for routing, policy, request ID, logging, counters, and traffic stats
- no claim yet for upstream HTTP/3 or gRPC-over-HTTP/3

If implementation work grows beyond this contract, that expansion must be
captured in a new architecture decision, not smuggled into Phase 1 or Phase 3.

## Phase 0 Exit Criteria

Phase 0 is complete when:

- the first-release HTTP/3 semantics are frozen in writing
- deferred items are explicit
- the staged architecture plan points to this freeze document

Those criteria are satisfied by this document together with
`ARCHITECTURE_HTTP3_PLAN.md`.
