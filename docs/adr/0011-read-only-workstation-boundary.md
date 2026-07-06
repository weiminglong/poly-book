# ADR-0011: Read-Only Workstation Boundary

## Status
Accepted

## Date
2026-07-06 (records a boundary in force since the workstation API shipped)

## Context
The workstation backend exposes live books, replay reconstruction, integrity
summaries, execution timelines, and a SQL workbench over HTTP, WebSocket, and
optionally gRPC. The obvious next step — order entry, cancels, risk toggles —
would turn it into a trading control plane.

A control plane has prerequisites this repository deliberately does not own
yet: real authentication and authorization, risk checks and kill switches,
environment separation, audited mutation workflows, and exchange
reconciliation. Shipping mutation routes without those would not make the
system more capable; it would make it dishonest about what it can support
safely.

There is also an architectural reason: browser clients should be able to
inspect the system without being anywhere near the capture path.

## Decision
Every serving surface is read-only, and the trust boundary is explicit:

- **No mutation routes.** HTTP, WebSocket, and gRPC expose only reads over
  data that ingest already persisted. The API processes derive a live read
  model in memory but never write market data; persistence belongs to the
  `ingest`/`auto-ingest` processes alone. The one write-shaped surface — the
  SQL workbench — is guarded (write keywords rejected, identifiers
  allowlisted to the six advertised datasets, `LIMIT` injected, server-side
  readonly enforcement) and fuzzed (`fuzz_query_guard`).
- **Loopback by default.** All surfaces bind to `127.0.0.1`; the default
  trust boundary is the host.
- **Bearer token required off loopback.** Startup refuses any non-loopback
  API or gRPC bind unless `api.auth_token` is set. When set, every data route
  (HTTP, WS, gRPC) requires `Authorization: Bearer <token>`, compared in
  constant time. `/health/live` and `/health/ready` stay open for
  orchestrator probes; bundled static UI assets are served without auth (the
  UI is public code — every data route it calls is still gated).

This is authentication, not authorization: one token, one implied operator,
no roles.

## Alternatives Considered
- **Ship mutation routes now, gated by config**: a disabled-by-default order
  path is still an order path — it must be audited, risk-checked, and
  secured as if enabled. Rejected: the supporting domains do not exist yet,
  and a config flag is not a security boundary.
- **Full authz/RBAC for the read surface**: roles, scopes, and audit trails
  for a single-operator read-only workstation is machinery without a user.
  Deferred until there is a multi-user requirement; the token check is
  deliberately minimal and honest about being minimal.
- **Rely purely on network isolation (no token at all)**: acceptable on
  loopback — and that is exactly the default — but "we assumed the network
  was private" is not a posture worth codifying for non-loopback binds.
  Rejected; the startup check makes the safe configuration the only one that
  boots.
- **mTLS between browser and API**: stronger than bearer tokens but painful
  for browser clients and disproportionate for the current deployment model.
  Not adopted; a reverse proxy can add TLS termination without code changes.

## Consequences
- **Blast-radius control**: a compromised or buggy API process can leak what
  it can read, but cannot corrupt captured data, alter history, or touch a
  venue. Combined with process separation (ADR-0010), the read surface can be
  restarted, exposed, or broken without endangering capture.
- **Browser clients are fully decoupled from ingest**: the SPA reconstructs
  nothing from raw feed messages and holds no venue credentials; it talks
  only to read routes.
- **The workstation stays honest**: the route surface matches what the system
  can actually support safely today, rather than sketching a control plane it
  cannot back with risk controls.
- **Single-operator assumption is load-bearing**: one shared token means no
  per-user audit trail and no revocation granularity. Anyone needing
  multi-user access control must add real authz first — that work is
  explicitly deferred, not implicitly promised.
- **Future mutation features carry a documented bar**: order workflows enter
  only alongside authentication/authorization, risk checks, environment
  separation, and audited mutations (see docs/serve-api.md), not before.
