# Quant Workstation Platform -- Design

## Overview

The frontend should present `poly-book` as a market-data integrity and replay
workstation, not as a generic exchange terminal. The architecture must expose
the repository's strongest capabilities while preserving the existing system
boundaries between ingest, storage, replay, metrics, and execution state.

The first shipped version is intentionally read-only. It is meant to answer:

- what is happening on the live feed right now
- whether the data is continuous and trustworthy
- how replay behaves under different time semantics
- where latency is being spent
- what happened to locally recorded orders and fills

## Topology

The serving topology is:

```text
Polymarket WS/REST -> ingest pipeline -> storage + metrics
                                  |
                                  +-> live read model -> Rust API -> SPA
                                                   |
                                                   +-> replay / integrity readers
                                                   +-> ClickHouse query adapter
```

The browser does not talk directly to ingest internals, Parquet files, or
Prometheus endpoints. A dedicated Rust API layer owns browser-facing contracts.

## Serving Architecture

### Rust API layer

The API layer provides versioned REST and WebSocket interfaces under
`/api/v1`. It is responsible for:

- live feed and asset status
- live order book snapshots and update streams
- replay and integrity queries
- execution timeline queries
- latency summary endpoints
- guarded read-only SQL execution

The API layer may run in the same deployment unit as the current runtime
initially, but it must remain separately deployable later.

### Live read model

Live book views should be maintained server-side from a non-blocking fanout of
`PersistedRecord` values or equivalent derived events. The browser must not
reconstruct live state by replaying raw feed messages itself.

The live read model is responsible for:

- latest book state by active asset
- feed/session status
- last event timestamp
- reconnect and continuity indicators
- freshness metadata

### Historical and analytical reads

Historical replay remains grounded in the existing replay readers and datasets.
For deployed interactive queries:

- ClickHouse is the primary interactive serving backend
- Parquet remains the canonical source for audit and replay semantics
- DuckDB is reserved for local or development workflows

This keeps the workstation responsive without changing the source of truth for
historical reconstruction.

## Product Surfaces

### Live Feed

The live feed surface shows:

- active assets and session status
- top of book and depth
- spread and mid updates
- last update time
- continuity warnings such as reconnect boundaries or gap markers

### Replay Lab

The replay lab shows:

- replayed books at a timestamp or across a window
- explicit `recv_time` versus `exchange_time` selection
- continuity events returned with the replay
- whether replay started from a checkpoint
- replay validation and drift visibility

### Integrity

The integrity surface shows:

- dataset freshness
- missing windows
- reconnects and stale snapshot skips
- gap events
- replay validation outcomes
- clear complete versus best-effort labels

### Latency

The latency surface shows:

- websocket latency summaries
- message processing time
- storage flush timing
- execution latency traces derived from `execution_events`

### Execution Timeline

The execution surface is read-only and shows:

- order lifecycle transitions
- exchange acknowledgements, rejects, and fills
- timestamps and latency trace fields
- explicit separation from public market-data state

### Query Workbench

The query workbench provides:

- schema browsing for split datasets
- canned examples
- read-only SQL execution against an approved backend
- row limits, timeout limits, and disabled-by-default controls

## Safety And Scope Boundaries

The workstation's first version has no authenticated mutating actions. It does
not claim:

- live order submission
- cancel or replace controls
- risk checks
- permissions or roles
- strategy deployment
- parameter tuning

Future modules may add those capabilities, but only behind dedicated auth,
risk, audit, and environment boundaries. Their OpenSpec definitions exist to
prevent premature coupling, not to imply they are already implemented.

## Public Interface Shape

The planned API surfaces include:

- `GET /api/v1/feed/status`
- `GET /api/v1/assets/active`
- `GET /api/v1/orderbooks/{asset_id}/snapshot`
- `WS /api/v1/streams/orderbook?asset_id=...`
- `GET /api/v1/replay/reconstruct?...`
- `GET /api/v1/replay/window?...`
- `GET /api/v1/integrity/summary`
- `GET /api/v1/integrity/assets/{asset_id}`
- `GET /api/v1/latency/summary?...`
- `GET /api/v1/execution/orders?...`
- `POST /api/v1/query/sql`

The SQL interface must be read-only, row-limited, time-bounded, and disabled by
default outside approved environments.

## Current Phase 3 Implementation Boundary

The first backend slice now implemented is narrower than the full planned API
surface. The current shipped routes are:

- `GET /api/v1/feed/status`
- `GET /api/v1/assets/active`
- `GET /api/v1/orderbooks/{asset_id}/snapshot`
- `GET /api/v1/replay/reconstruct`

Current backend constraints:

- live state is derived from an in-process read model
- replay reads are Parquet-only today
- the runtime is read-only and does not persist live data

The following planned surfaces remain deferred:

- `GET /api/v1/integrity/summary`
- execution timeline routes
- SQL workbench routes
- WebSocket order book streaming
- ClickHouse-backed API reads

## Rollout

The rollout order is:

1. Define the workstation capabilities and future module boundaries in OpenSpec
2. Add the Rust API layer and live read model
3. Add the SPA with seeded sample data and smoke coverage
4. Add CI validation and later deployment packaging

This order keeps the frontend honest about the repository's current strengths
and avoids presenting unfinished trading control-plane work as shipped product.
