# `serve-api`

`serve-api` is the read-only runtime entrypoint for the quant workstation
backend. It exists to expose the current system's strongest inspection surfaces
without turning the project into a trading control plane before the supporting
domains exist.

## Purpose

The current command is meant for:

- live feed inspection
- active asset visibility
- live order book snapshots from a server-side read model
- Parquet-backed historical replay reconstruction

It is not meant for:

- storage ingestion
- live order submission
- risk controls
- execution control
- research query serving beyond the current narrow API

## Runtime Topology

```text
WsClient -> Dispatcher -> PersistedRecord fanout -> LiveReadModel -> pb-api routes
                                                 \
                                                  -> Parquet-backed replay via pb-replay
```

The browser or client talks only to the API layer. It does not reconstruct the
book from raw feed messages directly.

## Why It Is Read-Only

The current repository has market-data ingestion, replay, metrics, and
execution journaling, but it does not yet own the full trading control-plane
requirements:

- authentication and authorization
- risk checks and kill switches
- environment separation
- audited order mutation workflows
- exchange reconciliation surfaces

Keeping `serve-api` read-only makes the current workstation honest about what
the system can support safely.

## Why It Is Parquet-First

Historical routes currently use `ParquetReader` through `pb-replay` because
Parquet is the canonical replay and audit source in the repo today. That keeps
the first API slice aligned with the existing replay semantics rather than
optimizing immediately for a broader serving backend.

ClickHouse-backed API reads are deferred to a later increment.

## Why It Does Not Persist Live Data

The first API release subscribes to live data and derives a live read model in
memory, but it does not write new market data to storage. That keeps the first
serving runtime focused on read-model correctness and route contracts instead of
mixing operator-facing API concerns with ingestion persistence concerns.

If you want durable storage, keep using `ingest` or `auto-ingest`.

## Live Modes

### Fixed Tokens

```bash
cargo run -- serve-api --tokens <TOKEN_ID>
```

Use this when you want the live read model to follow a fixed set of token IDs.

### Auto-Rotate

```bash
cargo run -- serve-api --auto-rotate
```

Use this when you want the command to follow the rotating BTC 5-minute market.

Rotation behavior:

- the active asset set is replaced on each rotation
- rotated-out assets are evicted from the live read model
- snapshot requests for inactive assets return `404`

## Current Route Surface

The current implementation exposes:

- `GET /api/v1/feed/status`
- `GET /api/v1/assets/active`
- `GET /api/v1/orderbooks/{asset_id}/snapshot`
- `GET /api/v1/replay/reconstruct`

See [docs/api.md](api.md) for route details.

## Current Browser Client

The separate Phase 4 SPA talks only to these HTTP routes. The currently shipped
web surfaces are:

- `Live Feed`
- `Replay Lab`

Current browser transport behavior:

- adaptive HTTP polling for live views
- foreground polling faster than background polling
- stale browser requests are cancelled before the next refresh cycle

This remains an interim client strategy until the deferred workstation streaming
route exists.

The SPA is developed and served separately from `serve-api` today. Packaging the
Rust API and static frontend assets together remains later work.

## Deferred From This Phase

The following are intentionally not part of the current `serve-api` slice:

- `integrity/summary`
- execution timeline routes
- SQL workbench routes
- WebSocket order book streaming
- ClickHouse-backed API reads
