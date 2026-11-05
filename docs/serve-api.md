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
- research query serving

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

`serve --tokens A,B` serves a fixed asset set with per-asset checkpoint
hydration. `serve` with no `--tokens` follows the WAL instead: every asset the
ingest process writes becomes active and ages out ten minutes after its last
record, so it pairs with a rotating `ingest --auto-rotate` (the `full` compose profile).
Follow mode hydrates from the retained WAL only, without checkpoints.

The browser or client talks only to the API layer. It does not reconstruct the
book from raw feed messages directly.

## Why It Is Read-Only

The current repository has market-data ingestion, replay, and metrics, but it
does not yet own the full trading control-plane requirements:

- authentication and authorization
- risk checks and kill switches
- environment separation
- audited order mutation workflows
- exchange reconciliation surfaces

Keeping `serve-api` read-only makes the current workstation honest about what
the system can support safely.

### Trust boundary & authentication

All surfaces bind to loopback by default (`127.0.0.1`), so the trust boundary is
the host. Before exposing the API on any non-loopback interface, set
`api.auth_token` (or `PB__API__AUTH_TOKEN`): when configured, every API and
WebSocket data route requires `Authorization: Bearer <token>` (constant-time
compared), while `/health/live` and `/health/ready` stay open for orchestrator
probes. Startup rejects any non-loopback API bind without a token. With no token set the workstation stays open on loopback — appropriate
only for a single-operator host. This is bearer-token authentication, not full
authorization/RBAC, which remains out of scope for the read-only workstation.

### Bundled web UI (optional)

When `api.static_assets_dir` (env: `PB__API__STATIC_ASSETS_DIR`) points at a
built `web/dist`, both `serve-api` and `serve` host the SPA from the same
process: unknown paths serve static assets with an `index.html` fallback so
client-side routes deep-link. API, health, and WebSocket routes always take
precedence, and static requests are metered under the `<unmatched>` route
label. The Docker image enables this against its bundled assets; the key is
unset by default outside Docker. Static assets are served without auth — the
UI is public code, while every data route it calls still requires the bearer
token when one is configured.

## Historical Service

Historical routes (replay, integrity) are served through
`pb_service::HistoricalService`, which wraps a `ParquetReader` from
`pb-replay`; local files and cloud object stores share the same range-read path
and recovered partitions are selected through atomic recovery manifests.

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
  tailing. Startup inventories checkpoints once for all configured assets and
  applies a distinct WAL cutoff for each asset; any asset without a checkpoint
  is replayed from the earliest retained WAL record, and a checkpoint without a
  WAL cut is ignored rather than mixed with older WAL records. Readiness stays at 503 if
  object-store checkpoint access, WAL access, or WAL decoding fails; an
  empty/partial recovery is not marked hydrated.

The `serve` process can be killed and restarted without data loss. On restart it
re-hydrates from the latest checkpoint and catches up from the WAL.
WebSocket book subscribers continue receiving incremental updates in separated
mode because projector-side broadcast fanout is configured for WAL-tail
applies, not just feed-driven `serve-api` applies.

## Why It Does Not Persist Live Data

The API processes (both `serve-api` and `serve`) derive a live read model in
memory but do not write new market data to storage. Ingestion persistence is the
responsibility of the `ingest` process.

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
- `GET /health`
- `WS /api/v1/streams/orderbook?asset_id=...`

See [docs/api.md](api.md) for route details.

### Health Endpoints

`GET /health` returns detailed operational status for the serve process (always
HTTP 200). For probes, use `GET /health/live` (liveness) and `GET /health/ready`
(200 only when ready, otherwise 503).

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
- `Orderbook`
- `Replay Lab`
- `Integrity`

Current browser transport behavior:

- WebSocket order book streaming for live book views, with automatic fallback to
  adaptive HTTP polling
- adaptive HTTP polling for feed status and active assets
- foreground polling faster than background polling
- stale browser requests are cancelled before the next refresh cycle
- virtualized order book table for deep book views
- render-throttled WebSocket updates (one re-render per animation frame)

The SPA is developed against the Vite dev server (`web/README.md`) and shipped
bundled: the Docker image sets `api.static_assets_dir` so `serve-api`/`serve`
host the built assets (see "Bundled web UI" above).

## Deferred From This Phase

The following are intentionally not part of the current serving runtime:

- latency summary routes
- multi-replica WAL fan-out (single reader is sufficient for current scale)
