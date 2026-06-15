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
| `RestClient` | HTTP client for REST API discovery, snapshot fetching, and V2 market metadata (`get_clob_market_info`). |
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
  `discover_by_slug`), snapshot backfill (`fetch_book`), and CLOB V2 market
  metadata lookup (`get_clob_market_info`).

## CLOB V2 notes

The Polymarket CLOB V2 cutover (2026-04-28) did not change the WebSocket URL
(`wss://ws-subscriptions-clob.polymarket.com/ws/market`), the subscription
shape (`{"assets_ids": [...], "type": "market"}`), or the existing `book` /
`price_change` / `last_trade_price` payloads. The `fee_rate_bps` field on
`last_trade_price` continues to reflect the fee actually charged at match
time (now protocol-set per market rather than embedded in the order).

V2 additions handled here:

- `tick_size_change` events parse via `WsMessage::TickSizeChange` and are
  observed via the `pb_messages_received_total{event_type="tick_size_change"}`
  Prometheus counter. They are informational — `L2Book` stores prices at full
  `FixedPrice` precision and does not enforce a minimum tick.
- `GET /book` responses may carry `tick_size`, `min_order_size`, `neg_risk`,
  and `last_trade_price`. They are optional fields on `RestBookResponse`.
- `GET /clob-markets/{condition_id}` is exposed as
  `RestClient::get_clob_market_info`.

Premium V2 events (`best_bid_ask`, `new_market`, `market_resolved`) require
`custom_feature_enabled: true` on subscribe and are not modeled today.

## Design Notes

- Dispatcher uses `FxHashMap` for hot-path lookups on trusted venue data.
  See [ADR-0006](../../docs/adr/0006-fxhashmap-dispatcher.md).
- Unified `parse_side()` function for bid/ask parsing (deduplicated from previous
  per-site implementations).
- WsClient reconnects with exponential backoff plus jitter. The exponential term
  is capped below `reconnect_max_delay_ms` so jitter still varies the delay at the
  cap (otherwise every client would reconnect at exactly the max). The backoff
  attempt counter resets after a session that stays connected ≥30s, so a later
  disconnect retries promptly instead of inheriting an ever-growing delay.
- A liveness watchdog forces a reconnect if no frame (data or pong) arrives within
  ~3× the ping interval, so a half-open TCP connection cannot silently stall the
  feed for many minutes.
- On reconnect success, the dispatcher clears per-asset sequence and stale
  snapshot tracking before emitting `SourceReset`, so downstream replay does not
  stitch state across feed sessions or reject the first fresh post-reconnect
  snapshot as stale.
- Snapshot staleness: only *strictly older* snapshots (`exchange_ts < last_ts`)
  are skipped. Polymarket emits one `book` per trade at millisecond resolution,
  so two trades in the same millisecond produce equal-timestamp snapshots whose
  later one carries newer state; equal timestamps are accepted, and exact
  retransmits of identical state are deduplicated by the venue `hash`.
- Atomic snapshots: a `book` message's levels are all converted before any are
  emitted. A mid-message conversion failure emits a single `SourceReset` marker
  (and leaves the staleness tracker untouched) instead of a partial snapshot
  that would be indistinguishable from a complete one. A `price_change` batch
  skips an unparseable entry rather than aborting the remaining valid deltas.
- Venue cross-check: the dispatcher keeps a per-asset shadow `L2Book` (seeded by
  snapshots, advanced by deltas) purely to compare our reconstructed top-of-book
  against the venue-stated `best_bid`/`best_ask` on each `price_change` entry. A
  divergence emits an `IngestEventKind::BookMismatch` event (a queryable data
  hole) + `pb_book_mismatches_total`, surfacing silently-dropped/corrupt updates
  (A.74/A.109). Shadow books are dropped on continuity reset. Frames that match
  no known message type increment `pb_unknown_messages_dropped_total` (A.110).
- Self-healing: on a detected divergence the dispatcher requests a resnapshot
  (`with_resnapshot_tx`); `run_resnapshot_worker` (debounced per asset) fetches a
  fresh REST book and re-injects it as a synthetic WS `book` message via the
  shared raw channel, so the normal snapshot path rebuilds the book. The
  REST→WS-`book` conversion is round-trip-tested against `WsMessage` so it matches
  the live wire format. (Wired in `ingest`; `auto-ingest` relies on rotation.)
- Wire types borrow from raw buffers (`&'a str`) for zero-copy deserialization.
  See [ADR-0004](../../docs/adr/0004-zero-copy-deserialization.md).
- Tests cover malformed JSON, `parse_side` coverage, dispatcher behavior,
  same-millisecond and duplicate snapshot handling, atomic snapshot emission,
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
