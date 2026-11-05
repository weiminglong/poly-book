# API Notes

This document describes the current read-only workstation API surface exposed by
`serve-api`.

## Stability

The current API is intentionally narrow. It is meant to support the first
backend slice of the workstation and may evolve as later phases add latency
and frontend capabilities.

The SPA ships four routes: `Live Feed`, `Orderbook`, `Replay Lab`, and
`Integrity`. Latency surfaces remain deferred.

Parquet-backed routes are storage-location agnostic: local files and configured
cloud object stores use the same reader, and crash-recovery manifests are applied
below the service layer without changing response shapes.

## Security / Trust Boundary

HTTP and WebSocket data surfaces support a shared bearer token via
`api.auth_token` / `PB__API__AUTH_TOKEN`. All services default to binding
loopback (`127.0.0.1`); startup rejects a non-loopback API bind unless that
token is configured. Health probes stay open. Never expose the metrics port to
an untrusted network. HTTP request/response routes are bounded by a
per-request timeout, a global in-flight concurrency cap, and a request-body
size limit. 5xx responses return an opaque message and log the detail
server-side.

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
- `depth`
  - optional
  - same validation rules as the live snapshot route

Returns:

- asset ID
- selected replay mode
- whether replay used a checkpoint
- replay starts from the latest checkpoint at or before `at_us` only if that
  checkpoint is inside the configured lookback floor; stale checkpoints older
  than `at_us - lookback` are ignored so reconstruct reads stay bounded
- checkpoint discovery for a reconstruct request scans only the configured
  lookback window; if none is found, replay falls back to snapshot-based
  reconstruction
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

## Deferred Endpoints

The current API does not yet implement:

- latency summary routes
