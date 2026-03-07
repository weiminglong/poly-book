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

## Deferred Endpoints

The current API does not yet implement:

- integrity summary routes
- execution timeline routes
- SQL workbench routes
- WebSocket order book streaming
- ClickHouse-backed API reads
