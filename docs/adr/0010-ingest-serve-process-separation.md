# ADR-0010: Ingest/Serve Process Separation

## Status
Accepted

## Date
2026-07-06 (records a decision implemented with the serving-architecture
rework in March 2026)

## Context
The original `serve-api` runtime was a single process owning venue WebSocket
connectivity, normalization, book state, and browser-facing HTTP/WS serving.
That shape has concrete failure-coupling problems:

- a serving-side bug (a panicking handler, a slow client, a memory spike) can
  take down capture, which is the one thing the system must never lose
- a restart of the process loses all book state and leaves seconds of empty
  books while the feed rebuilds from the venue
- the durability-critical write path and the read path compete for the same
  scheduler and the same locks

Capture and serving have different availability requirements: the write path
must be maximally boring and never stall; the read path should be free to
restart, redeploy, and evolve.

## Decision
Split the runtime into two processes with the WAL (ADR-0008) as the only
handoff between them:

- **`ingest`**: venue WebSocket → dispatcher → WAL writer + storage sinks
  (Parquet, ClickHouse) + periodic checkpoints. Serves no HTTP.
- **`serve`**: on cold start, loads the latest `BookCheckpoint` (which carries
  the WAL offset it was taken at), replays WAL records from that offset, then
  live-tails the WAL into a watch-based read model serving HTTP/WS and the
  optional gRPC surface. Writes no market data.

There is no network protocol between the two — they share a WAL directory on
the filesystem. Checkpoint hydration bounds cold-start work to the checkpoint
interval's worth of WAL records, and the live tail resumes from the exact
post-hydration position so no record is applied twice.

The combined `serve-api` mode (feed + API in one process, no WAL) is retained
for development and demos, not as the production topology.

## Alternatives Considered
- **Keep the monolith**: simplest operationally, but serving faults remain in
  the capture blast radius, and restart-without-data-loss is impossible.
  Rejected for the coupling alone.
- **Unix domain socket between the processes**: adds a protocol layer,
  serialization, connection management, and a new failure mode (socket
  backpressure) — while still being single-host. The WAL already provides
  durable, ordered, resumable delivery; a socket provides none of those.
  Rejected.
- **Shared-memory ring buffer**: the fastest option, but volatile — it would
  still need the WAL for durability, so it only adds a second transport to
  maintain. Rejected as premature at current throughput.
- **Network RPC (gRPC/HTTP) from ingest to serve**: enables multi-host, at the
  cost of latency, delivery semantics, and operational surface the current
  single-host deployment does not need. Deferred rather than rejected — the
  WAL boundary does not preclude adding a networked transport later.

## Consequences
- **Serve restarts are free**: `serve` can be killed and redeployed at will;
  it re-hydrates from the latest checkpoint and catches up from the WAL.
  Readiness (`/health/ready`) flips only after hydration completes.
- **Capture is isolated from read-path faults**: a misbehaving client or
  handler cannot stall the append path. In the other direction, ingest treats
  WAL failure as fatal and exits for a supervisor restart rather than running
  without durability.
- **Filesystem coupling**: both processes must share a host or volume. This is
  the accepted single-host constraint inherited from ADR-0008; the
  docker-compose `full` profile ships exactly this topology (shared WAL
  volume).
- **Codec version lockstep**: ingest and serve must run compatible builds of
  the WAL codec. A serve replica lagging behind an ingest codec bump surfaces
  as decode errors during tailing; the diagnosis and recovery procedure
  (check `pb_wal::codec::CURRENT_VERSION`, drain, re-create) is documented in
  the on-call runbook (`monitoring/RUNBOOK.md`) and docs/operations.md.
- **Two processes to operate instead of one**: two sets of logs, metrics, and
  restarts. Mitigated by the compose profiles, `/health` surfaces exposing
  WAL lag (`wal_lag_bytes`) and resync state (`needs_resync`), and alert
  rules covering consumer lag.
- **Recovery paths are testable offline**: checkpoint + WAL hydration and
  handoff-position correctness are covered by integration tests
  (`tests/integration/checkpoint_wal_hydration.rs`) rather than only being
  exercised in incidents.
