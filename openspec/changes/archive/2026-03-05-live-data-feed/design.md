# Live Data Feed -- Design

## Architecture Overview

The feed pipeline is a three-stage actor system connected by bounded
`tokio::sync::mpsc` channels:

```
Polymarket WS ──> WsClient ──[WsRawMessage]──> Dispatcher ──[OrderbookEvent]──> downstream
                                                    ^
                             RestClient (on-demand) |
```

Each actor runs in its own Tokio task. `WsClient` owns the network connection;
`Dispatcher` owns deserialization and normalization; `RestClient` is invoked
ad-hoc for snapshots or market discovery.

## WebSocket Connection & Reconnection

`WsClient::run` loops forever:

1. Call `connect_and_listen` which opens a connection, subscribes to each
   `asset_id`, then enters a `tokio::select!` loop reading messages and sending
   heartbeat pings.
2. On connection error or graceful close, compute backoff and sleep before
   retrying.

### Reconnection strategy

Exponential backoff with jitter, capped at 30 seconds:

```
delay = min(BASE_BACKOFF_MS * 2^attempt + jitter(exp/4), MAX_BACKOFF_MS)
```

- `BASE_BACKOFF_MS = 100`
- `MAX_BACKOFF_MS = 30_000`
- Jitter derived from `SystemTime` subsec nanos (no external RNG dependency).
- Attempt counter resets on graceful close.

### Heartbeat

A 10-second `tokio::time::interval` sends WebSocket `Ping` frames. Pong
responses are logged at debug level. If the remote closes, the loop exits and
reconnection triggers.

## Channel-Based Message Passing

`WsRawMessage` carries the raw JSON text plus a `recv_timestamp_us` (local
microsecond clock). The dispatcher reads from this channel, deserializes into
`WsMessage<'_>` (borrowed, zero-copy on string fields), and emits normalized
`OrderbookEvent` values on the output channel.

Sequence numbers are assigned monotonically per dispatcher instance, starting
at 0 and incrementing for every emitted event.

## REST Client

- `fetch_book(token_id)` -- GET `clob.polymarket.com/book?token_id=...`
- `discover_markets(offset, limit)` -- GET
  `gamma-api.polymarket.com/events?active=true&closed=false&tag=crypto`

Both methods call `rate_limiter.acquire()` before issuing the HTTP request.

## Rate Limiter

Wraps `governor::RateLimiter` (token bucket) with:

- Quota: 150 requests per second, burst size 150.
- `acquire()` awaits `until_ready()` (non-failing, back-pressures caller).
- `Arc`-wrapped inner state; `Clone`-able for sharing across tasks.

## Error Handling

`FeedError` is a `thiserror` enum with `From` impls for `tungstenite::Error`,
`reqwest::Error`, `serde_json::Error`, `pb_types::TypesError`, and
`url::ParseError`, plus explicit `RateLimited` and `ChannelSend` variants.
