# API Notes

This document describes the current read-only workstation API surface exposed by
`serve-api`.

## Stability

The current API is intentionally narrow. It is meant to support the first
backend slice of the workstation and may evolve as later phases add integrity,
execution, query, and frontend capabilities.

The current SPA consumes only these routes for `Live Feed` and `Replay Lab`.

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
- filters updates to the requested asset
- handles ping/pong and graceful close
- slow consumers that fall behind the broadcast buffer receive a fresh full
  snapshot to re-sync

## Deferred Endpoints

The current API does not yet implement:

- SQL workbench routes
- ClickHouse-backed API reads
- latency summary routes
