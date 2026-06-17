# pb-api

Read-only HTTP API and live read model for workstation clients. Serves live feed
health, order book snapshots, historical replay, integrity summaries, execution
timelines, and WebSocket streaming — all without coupling browser clients to
ingest internals.

## Routes

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Detailed health JSON (always 200; for humans/dashboards) |
| GET | `/health/live` | Liveness probe — always 200 while the process is up |
| GET | `/health/ready` | Readiness probe — 200 when hydrated and not awaiting resync, else 503 |
| GET | `/api/v1/feed/status` | Feed health, session state, active assets |
| GET | `/api/v1/assets/active` | Currently tracked assets with metadata |
| GET | `/api/v1/assets/resolve` | Resolve a slug or condition ID to an asset ID |
| GET | `/api/v1/orderbooks/{asset_id}/snapshot` | Live L2 book snapshot |
| GET | `/api/v1/replay/reconstruct` | Point-in-time book reconstruction |
| GET | `/api/v1/integrity/summary` | Dataset continuity and validation metrics |
| GET | `/api/v1/execution/orders` | Order lifecycle timeline |
| GET | `/api/v1/query/datasets` | Available datasets and column schemas |
| POST | `/api/v1/query/sql` | Execute guarded read-only SQL query |
| WS | `/api/v1/streams/orderbook` | Live book streaming via WebSocket |

Full route contracts and error semantics: [docs/api.md](../../docs/api.md).

## Key Types

| Type | Description |
|------|-------------|
| `LiveReadModel` | Server-side book state maintained from the ingest channel. Groups snapshots for connected assets. |
| `BookBroadcast` | `tokio::sync::broadcast` channel for WebSocket incremental updates. |
| `AppState` | Shared axum state holding the live model, broadcast, slug registry, service backends, WAL lag tracking, and config. |
| `ApiConfig` | API server configuration (depth limits, stale thresholds, query guard settings). |
| `HealthResponse` | Typed health response struct (avoids `serde_json::json!` dynamic `Value` tree allocation). |
| `ApiError` | Structured error type mapped to HTTP status codes. |

DTOs in `dto.rs`: `FeedStatusResponse`, `LiveOrderBookSnapshot`,
`ReplayReconstructionResponse`, `IntegritySummaryResponse`,
`ExecutionTimelineResponse`, `BookUpdateMessage`, `QueryResultResponse`,
`QueryColumn`, `DatasetSchemaResponse`, `DatasetInfo`, and others.

## Data Flow

```text
PersistedRecord channel ──▶ LiveReadModel ──┬──▶ REST handlers
                                            └──▶ BookBroadcast ──▶ WS clients

AnyReplayService ──▶ replay/integrity/execution handlers
AnyQueryService  ──▶ query workbench handlers (datasets, SQL)
```

## Design Notes

- The API is intentionally read-only. No mutation routes exist in v1.
  See [docs/serve-api.md](../../docs/serve-api.md) for runtime constraints.
- **Trust boundary**: there is no authentication. Services bind loopback by
  default and must sit behind an authenticating reverse proxy before being
  exposed off-host. HTTP request/response routes are bounded by a per-request
  timeout, a global in-flight concurrency cap, and a request-body size limit;
  the long-lived WS route is intentionally exempt from the request timeout. 5xx
  responses return an opaque message and log the real detail server-side.
- `LiveReadModel` receives `PersistedRecord` events and maintains in-memory
  book state without persisting to disk. After each snapshot materialization,
  delta, and checkpoint apply it runs `L2Book::check_integrity`; a crossed/locked
  book increments `pb_crossed_books_total` and surfaces a `crossed_book`
  continuity warning instead of being served silently.
- The projector keeps a cached published projection and only rebuilds
  `AssetReadView` snapshots for assets touched by a record, instead of
  re-materializing every tracked book on each update.
- WebSocket streaming uses a broadcast channel with capacity 256. Slow consumers
  that fall behind receive a fresh full snapshot to re-sync.
- WS fan-out is bounded: a global cap on concurrent sessions (excess upgrades get
  503), a small inbound message/frame size limit (it is a read-only stream), and
  a server heartbeat with an idle timeout that reaps half-open peers.
- Both `serve-api` and separated `serve` mode route projector updates through
  the same broadcast fanout, so WAL-tail updates are streamed to WS clients as
  incremental book messages instead of snapshot-only sessions.
- Historical reads use configurable backend (Parquet or ClickHouse) via `pb-service` enum dispatch.
- Query workbench (`/query/datasets`, `/query/sql`) is ClickHouse-only and optional (`query_service: Option<AnyQueryService>`).
- WebSocket `asset_id` accepts either the canonical token ID or a registered
  slug, matching the REST snapshot route.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| Route added or removed | `docs/api.md`, `docs/serve-api.md`, `CLAUDE.md` current routes list |
| DTO shape changed | `docs/api.md` response schema, `web/src/types.ts` TypeScript interfaces |
| New deferred route documented | `docs/serve-api.md` deferred section |
| `LiveReadModel` behavior changed | `docs/serve-api.md` runtime section |
| WebSocket protocol changed | `web/src/useOrderBookStream.ts`, `docs/api.md` |
| Route or capability added/removed | Update the active OpenSpec change `tasks.md`, `proposal.md` if scope shifts |
| API config keys changed | `config/default.toml` `[api]` section, `docs/operations.md` |

## Tests

68 tests covering health states, error format consistency, depth and time parameter
validation, execution limits, `ServiceError` to `ApiError` mapping,
`PerAssetBroadcast` unit tests, and `LiveReadModel` tests.
