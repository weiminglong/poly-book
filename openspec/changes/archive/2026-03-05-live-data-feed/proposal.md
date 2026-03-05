# Live Data Feed

## Why

The poly-book system requires real-time orderbook data from Polymarket to build
and maintain a local BTC 5-minute orderbook. A WebSocket connection provides
streaming updates (book snapshots, price changes, last-trade prices), while a
REST client enables on-demand book snapshots and market discovery via the Gamma
API. Robust reconnection with exponential backoff and rate limiting are essential
for sustained uptime against a public API with quotas.

## What Changes

- Add `WsClient` with automatic reconnection, heartbeat pings, and subscription
  management for Polymarket CLOB WebSocket.
- Add `RestClient` for fetching book snapshots and discovering active crypto
  markets via the Gamma API, gated by a token-bucket rate limiter.
- Add `Dispatcher` that deserializes raw WS JSON into typed `OrderbookEvent`
  values with monotonic sequencing.
- Add `RateLimiter` wrapping the `governor` crate with 150 req/s quota.
- Add `FeedError` enum covering WS, REST, serde, type-conversion, rate-limit,
  and channel-send failure modes.

## Capabilities

### New Capabilities

- `websocket-client`: Persistent WS connection to
  `wss://ws-subscriptions-clob.polymarket.com/ws/market` with automatic
  reconnection (exponential backoff + jitter, 100 ms base, 30 s cap), 10 s
  heartbeat pings, and per-asset subscription management.
- `rest-client`: REST snapshot fetcher (`/book?token_id=`) and Gamma API market
  discovery (`/events?active=true&closed=false&tag=crypto`) with rate limiting.
- `message-dispatcher`: Zero-copy JSON deserialization (`serde_json::from_str`
  with borrowed `WsMessage<'_>`) producing normalized `OrderbookEvent` values
  with fixed-point price/size and monotonic sequence numbers.

### Modified Capabilities

(none)

## Impact

- New crate `pb-feed` with modules: `ws`, `rest`, `dispatcher`, `rate_limiter`,
  `error`.
- Depends on `pb-types` for `OrderbookEvent`, `FixedPrice`, `FixedSize`,
  `AssetId`, `Sequence`, and wire types.
- Downstream consumers receive `OrderbookEvent` via `tokio::sync::mpsc` channel.
