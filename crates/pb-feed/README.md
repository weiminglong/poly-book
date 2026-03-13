# pb-feed

WebSocket ingest, REST discovery, and dispatch pipeline. Connects to the
Polymarket venue, receives raw messages, and normalizes them into split
`PersistedRecord` events for downstream storage and live state.

## Key Types

| Type | Description |
|------|-------------|
| `WsClient` | WebSocket client with automatic reconnect (exponential backoff + jitter). |
| `WsConfig` | Connection settings: URL, ping interval, reconnect params. |
| `WsRawMessage` | Raw text message with receive timestamp from the WebSocket stream. |
| `FeedMessage` | Enum wrapping `WsRawMessage` and `WsLifecycleEvent` for the ingest channel. |
| `WsLifecycleEvent` | Reconnect lifecycle event with session ID and details. |
| `RestClient` | HTTP client for REST API discovery and snapshot fetching. |
| `RestConfig` | REST endpoint URLs (CLOB base, Gamma base). |
| `RateLimiter` | Token-bucket rate limiter wrapping `governor`. |
| `Dispatcher` | Deserializes raw WS messages and normalizes into `PersistedRecord` events. |
| `FeedError` | Error type for feed operations. |

## Data Flow

```text
Polymarket WS ──▶ WsClient ──▶ Dispatcher ──▶ PersistedRecord channel
                                    ▲
Polymarket REST ──▶ RestClient ─────┘

Channel consumers: pb-store (ParquetSink, ClickHouseSink), pb-api (LiveReadModel)
```

- `WsClient` sends `FeedMessage` values (wrapping `WsRawMessage` and
  `WsLifecycleEvent`) on a `tokio::mpsc` channel.
- `Dispatcher` receives `FeedMessage` values, deserializes raw messages, maps
  lifecycle events to `IngestEvent` records, and emits `PersistedRecord` events
  on an outbound channel.
- `RestClient` is used independently for market discovery (`discover_markets`,
  `discover_by_slug`) and snapshot backfill (`fetch_book`).

## Design Notes

- Dispatcher uses `FxHashMap` for hot-path lookups on trusted venue data.
  See [ADR-0006](../../docs/adr/0006-fxhashmap-dispatcher.md).
- Unified `parse_side()` function for bid/ask parsing (deduplicated from previous
  per-site implementations).
- WsClient reconnects with exponential backoff plus jitter (improved distribution
  using Knuth's multiplicative hash constant) to avoid thundering herd on venue
  restarts.
- On reconnect success, the dispatcher clears per-asset sequence and stale
  snapshot tracking before emitting `SourceReset`, so downstream replay does not
  stitch state across feed sessions or reject the first fresh post-reconnect
  snapshot as stale.
- Wire types borrow from raw buffers (`&'a str`) for zero-copy deserialization.
  See [ADR-0004](../../docs/adr/0004-zero-copy-deserialization.md).
- 50 tests covering malformed JSON, `parse_side` coverage, dispatcher behavior,
  lifecycle events, and run loop shutdown.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New venue message type | `pb-types` wire types, `Dispatcher` parsing logic |
| WS reconnect behavior | `docs/operations.md` feed config section |
| New `PersistedRecord` variant from feed | `pb-store` schema + writer, `pb-replay` reader |
| Rate limiter config | `config/default.toml` `[feed]` section, `docs/operations.md` |
| Feed config keys added/removed | `config/default.toml`, `docs/operations.md` |
| Changes affect ingest topology | `docs/architecture.md`, check the active OpenSpec change |
