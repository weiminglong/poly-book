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

### Combined Mode (`serve-api`)

```text
WsClient -> Dispatcher -> PersistedRecord fanout -> LiveReadModel -> pb-api routes
                                                 \
                                                  -> pb-service -> pb-replay
```

### Separated Mode (`serve`)

```text
Checkpoint hydration -> WAL tail -> LiveReadModel -> pb-api routes
                                                  \
                                                   -> pb-service -> pb-replay
```

In separated mode, the `serve` process does not run venue connectivity. Instead
it hydrates from the latest `BookCheckpoint` (Parquet), replays WAL records from
the checkpoint offset, then live-tails the WAL written by a separate `ingest`
process. The live tail resumes from the exact post-hydration WAL position so
startup does not re-apply records that were already consumed during hydration.

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

## Configurable Historical Backend

Historical routes (replay, integrity, execution) are served through `pb-service`
traits with configurable backends:

- `api.historical_backend = "parquet"` (default) — uses `ParquetReader` via `pb-replay`
- `api.historical_backend = "clickhouse"` — uses `ClickHouseReader` via `pb-replay`

If ClickHouse is configured but unavailable at startup, the system probes
connectivity with a 3-second timeout and falls back to Parquet with a warning.

## Process Separation

The serving runtime supports two operational modes:

### Combined (`serve-api`)

Runs feed connectivity and API in a single process. No WAL involvement. The live
read model is fed directly from the dispatcher channel.

### Separated (`ingest` + `serve`)

- **`ingest`**: Runs venue WebSocket, dispatcher, WAL writer, and storage sinks.
  Does not serve HTTP.
- **`serve`**: Reads the latest `BookCheckpoint` from Parquet, replays WAL from
  the checkpoint's offset, then live-tails the WAL for new records. Serves HTTP/WS.
  The live WAL consumer commits its position periodically during steady-state
  tailing.

The `serve` process can be killed and restarted without data loss. On restart it
re-hydrates from the latest checkpoint and catches up from the WAL.
WebSocket book subscribers continue receiving incremental updates in separated
mode because projector-side broadcast fanout is configured for WAL-tail
applies, not just feed-driven `serve-api` applies.

## Why It Does Not Persist Live Data

The API processes (both `serve-api` and `serve`) derive a live read model in
memory but do not write new market data to storage. Ingestion persistence is the
responsibility of the `ingest` or `auto-ingest` processes.

The read model's projector keeps a cached published view for REST reads and
only rebuilds per-asset snapshot materializations for assets actually touched by
an incoming record. This avoids full book-vector rebuilds across all active
assets on every delta while keeping the HTTP and WebSocket contracts unchanged.

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

## Slug Resolution

All API routes that accept `asset_id` support human-readable slugs as an
alternative to full 70+ digit Polymarket token IDs. Slugs are populated
automatically from Gamma API metadata during market discovery.

- In **auto-rotate** mode, slugs are populated on each rotation cycle
- In **fixed tokens** mode, slugs are available only if the token IDs were
  passed as slugs that resolve through the registry (e.g., after a prior
  discovery)
- The `/api/v1/assets/resolve?q=...` endpoint allows explicit slug-to-token-ID
  lookup

For BTC 5-minute markets, slugs follow the pattern
`btc-updown-5m-{timestamp}-yes` / `btc-updown-5m-{timestamp}-no`.

## Current Route Surface

The current implementation exposes:

- `GET /api/v1/feed/status`
- `GET /api/v1/assets/active`
- `GET /api/v1/assets/resolve`
- `GET /api/v1/orderbooks/{asset_id}/snapshot`
- `GET /api/v1/replay/reconstruct`
- `GET /api/v1/integrity/summary`
- `GET /api/v1/execution/orders`
- `GET /health`
- `GET /api/v1/query/datasets`
- `POST /api/v1/query/sql`
- `WS /api/v1/streams/orderbook?asset_id=...`

See [docs/api.md](api.md) for route details.

### gRPC Surface

When `grpc.enabled = true`, the `serve` and `serve-api` processes also start a
gRPC server (default `0.0.0.0:50051`) exposing the same historical query
services via the `WorkstationService`:

- `Reconstruct` — replay book reconstruction at a target timestamp
- `IntegritySummary` — integrity check results for an asset time window
- `ExecutionTimeline` — execution event timeline for an order

The gRPC service delegates to the same `pb-service` traits used by the HTTP
routes, so backend selection (`parquet` / `clickhouse`) applies equally.

### Query Workbench

When `api.query_workbench_enabled = true` and the historical backend is
`clickhouse`, the query workbench endpoints become active:

- `GET /api/v1/query/datasets` — lists available tables and their column schemas
- `POST /api/v1/query/sql` — executes a read-only SQL query with guard rails

Guard rails:
- Write keywords (`INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`, `CREATE`,
  `TRUNCATE`) are rejected at the adapter level
- `LIMIT` is injected if not present (default `query_max_rows = 10000`)
- Queries time out after `query_timeout_secs` (default 30s)
- Both endpoints return 503 when the query workbench is disabled

### Health Endpoint

`GET /api/v1/health` returns operational status for the serve process:

```json
{
  "ready": true,
  "hydrated": true,
  "wal_lag_bytes": 0,
  "needs_resync": false
}
```

- `ready` — true when hydration is complete and the read model is serving
- `hydrated` — true after checkpoint + WAL replay finishes
- `wal_lag_bytes` — byte distance between WAL reader position and latest data
- `needs_resync` — true if the WAL reader's committed segment has been pruned
  (requires a fresh checkpoint hydration to recover)

## Current Browser Client

The separate SPA talks to these HTTP and WebSocket routes. The currently shipped
web surfaces are:

- `Live Feed`
- `Replay Lab`
- `Integrity`
- `Execution Timeline`

Current browser transport behavior:

- WebSocket order book streaming for live book views, with automatic fallback to
  adaptive HTTP polling
- adaptive HTTP polling for feed status and active assets
- foreground polling faster than background polling
- stale browser requests are cancelled before the next refresh cycle
- virtualized order book table for deep book views
- render-throttled WebSocket updates (one re-render per animation frame)

The SPA is developed and served separately from `serve-api` today. Packaging the
Rust API and static frontend assets together remains later work.

## Deferred From This Phase

The following are intentionally not part of the current serving runtime:

- latency summary routes
- multi-replica WAL fan-out (single reader is sufficient for current scale)
