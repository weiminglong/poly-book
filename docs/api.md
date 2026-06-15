# API Notes

This document describes the current read-only workstation API surface exposed by
`serve-api`.

## Stability

The current API is intentionally narrow. It is meant to support the first
backend slice of the workstation and may evolve as later phases add integrity,
execution, query, and frontend capabilities.

The SPA ships six routes: `Live Feed`, `Orderbook`, `Replay Lab`, `Execution
Timeline`, `Integrity`, and `Query Workbench`. The Query Workbench is opt-in
(`api.query_workbench_enabled = true`, requires the ClickHouse backend). Latency
surfaces remain deferred.

## Security / Trust Boundary

There is **no built-in authentication** on the HTTP, WebSocket, or gRPC surfaces.
All services default to binding loopback (`127.0.0.1`); to serve other hosts,
place an authenticating reverse proxy in front and only then change the bind
address. Never expose the metrics port or the SQL workbench to an untrusted
network. HTTP request/response routes are bounded by a per-request timeout, a
global in-flight concurrency cap, and a request-body size limit; 5xx responses
return an opaque message and log the detail server-side.

Health/readiness endpoints: `GET /health` (detailed JSON, always 200),
`GET /health/live` (liveness), `GET /health/ready` (200 only when ready, else
503).

## Slug Resolution

All API endpoints that accept an `asset_id` (path parameter or query parameter)
support human-readable slugs as an alternative to the full 70+ digit token ID.
If the input matches a registered slug, the server resolves it to the canonical
token ID before processing. Raw token IDs continue to work unchanged.

Slugs are populated automatically from Gamma API metadata during `discover` and
`auto-ingest` / `serve-api --auto-rotate`. For BTC 5-minute markets with YES/NO
token pairs, slugs use `-yes` / `-no` suffixes (e.g.
`btc-updown-5m-1741500000-yes`).

Response DTOs that include `asset_id` also include an optional `slug` field when
a slug mapping is known. The `active_assets` list in feed status returns
`{ asset_id, slug }` objects.

## Encoding Notes

- prices and sizes serialize as fixed-point strings, not floating-point JSON
  numbers
- timestamp fields are microseconds since Unix epoch
- replay `mode` is always explicit and must be one of `recv_time` or
  `exchange_time`

## Endpoints

### `GET /api/v1/feed/status`

Returns:

- feed mode: `fixed_tokens` or `auto_rotate`
- session status: `starting`, `connected`, or `reconnecting`
- current session ID when known
- active asset count and active asset list
- last rotation timestamp when applicable
- latest global continuity warning when present

### `GET /api/v1/assets/active`

Returns one summary per currently active asset:

- `asset_id`
- `slug` (optional, from slug registry)
- `label` (optional, human-readable market description)
- `last_recv_timestamp_us`
- `last_exchange_timestamp_us`
- `stale`
- `has_book`

`stale` is derived from the configured `api.stale_after_secs` threshold.

### `GET /api/v1/orderbooks/{asset_id}/snapshot`

Query params:

- `depth`
  - optional
  - defaults to `api.default_depth`
  - must be greater than `0`
  - must not exceed `api.max_depth`

Returns:

- asset ID
- current sequence
- last update timestamp
- best bid / best ask
- mid price and spread
- bid / ask depth counts
- top-N bid and ask levels
- `stale`
- latest asset continuity warning when present

### `GET /api/v1/replay/reconstruct`

Query params:

- `asset_id`
- `at_us`
- `mode`
  - required
  - one of `recv_time` or `exchange_time`
- `source`
  - optional
  - defaults to `parquet`
  - only `parquet` is supported today
- `depth`
  - optional
  - same validation rules as the live snapshot route

Returns:

- asset ID
- selected replay mode
- whether replay used a checkpoint
- replay starts from the latest checkpoint at or before `at_us` when available,
  and only reads forward from that checkpoint horizon instead of the full
  default lookback window
- `source_reset` continuity events are treated as hard replay boundaries; if a
  reset occurs before `at_us`, reconstruction requires a fresh post-reset
  snapshot instead of stitching pre-reset state across sessions
- reconstructed top-of-book and top-N levels
- continuity events returned with the reconstruction

## Error Semantics

### `400 Bad Request`

Returned for:

- invalid `depth`
- invalid replay `mode`
- unsupported replay `source`

### `404 Not Found`

Returned for:

- live snapshot requested for an inactive asset
- replay reconstruction when no historical snapshot exists before the requested
  time

### `503 Service Unavailable`

Returned for:

- live snapshot requested for an active asset whose live book has not yet been
  initialized from a snapshot

### `500 Internal Server Error`

Returned for:

- Parquet reader failures
- replay failures other than missing-snapshot lookup
- unexpected internal route failures

### `GET /api/v1/integrity/summary`

Query params:

- `asset_id`
- `start_us`
- `end_us`
  - `start_us` must be less than `end_us`
  - the window may not exceed **24 hours**; a larger range returns `400`

Returns:

- asset ID, time window
- book and ingest event counts
- reconnect, gap, and stale snapshot skip counts
- validation match/mismatch counts
- completeness label: `complete` or `best_effort`
- continuity events within the window

### `GET /api/v1/execution/orders`

Query params:

- `start_us`
- `end_us`
  - `start_us` must be less than `end_us`
  - the window may not exceed **24 hours**; a larger range returns `400`
  - the same window cap and result-limit clamp are enforced for the gRPC
    `ExecutionTimeline` RPC
- `order_id`
  - optional; filters to a single order
- `asset_id`
  - optional; filters to a single asset
- `limit`
  - optional
  - defaults to `100`
  - must be between `1` and `1000`

Returns:

- execution events ordered by timestamp
- each event includes kind, side, price, size, status, reason, and latency
  trace fields
- total count (before limit truncation)

### `WS /api/v1/streams/orderbook`

Query params:

- `asset_id`
  - required; must be an active asset

Behavior:

- upgrades to WebSocket
- sends an initial full snapshot message on connect
- sends incremental book update messages as JSON text frames
- accepts either a raw token ID or a registered slug in `asset_id`
- filters updates to the requested asset after slug resolution
- handles ping/pong and graceful close
- slow consumers that fall behind the broadcast buffer receive a fresh full
  snapshot to re-sync
- in separated `serve` mode, incremental frames are sourced from WAL-tail
  projector updates, so clients continue receiving live deltas after the
  initial snapshot

### `GET /api/v1/assets/resolve`

Query params:

- `q`
  - required; a slug or raw token ID to resolve

Returns:

- `found`: boolean indicating whether the input resolved to a known asset
- `asset_id`: the canonical token ID (present when `found` is `true`)
- `slug`: the slug for this asset (present when a slug mapping exists)

This endpoint always returns 200. Use `found` to check resolution status.

### `GET /health`

Returns operational status:

- `ready`: true when hydration is complete and read model is serving
- `hydrated`: true after checkpoint + WAL replay finishes
- `wal_lag_bytes`: byte distance between WAL reader position and latest data
- `needs_resync`: true if WAL reader needs re-hydration

### `GET /api/v1/query/datasets`

Returns available dataset schemas when query workbench is enabled.

Returns 503 when `api.query_workbench_enabled` is `false` (the default).

Response:

- `datasets`: array of dataset objects, each with `name`, `description`, and
  `columns` (array of `{ name, data_type }`)

### `POST /api/v1/query/sql`

Executes a read-only SQL query when query workbench is enabled.

Returns 503 when `api.query_workbench_enabled` is `false`.

Request body (JSON):

- `sql`: the SQL query string (required)
- `max_rows`: optional row limit override (default from config)

Guard rails:

- Only a single read-only statement is allowed
- Root statement must be `SELECT`, `WITH`, `SHOW`, `DESCRIBE`, or `EXPLAIN`
- Write keywords (`INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`, `CREATE`,
  `TRUNCATE`, `RENAME`, `GRANT`, `REVOKE`, `ATTACH`, `DETACH`) are rejected
  with 400 after comments and quoted literals are stripped from the guard scan
- `LIMIT` is injected if not present
- Queries time out after `api.query_timeout_secs` (default 30s)

Response:

- `columns`: array of `{ name, data_type }`
- `rows`: array of row arrays (JSON values)
- `row_count`: number of rows returned
- `truncated`: true if row limit was hit
- `execution_time_ms`: query execution duration

## Deferred Endpoints

The current API does not yet implement:

- latency summary routes
