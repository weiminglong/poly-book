# Live Data Feed -- Tasks

- [x] Scaffold `pb-feed` crate with `Cargo.toml` and module declarations
- [x] Define `FeedError` enum with `thiserror` derives and `From` impls
- [x] Implement `RateLimiter` wrapping `governor` with 150 req/s quota
- [x] Implement `WsClient` with connect, subscribe, and message-read loop
- [x] Add 10-second heartbeat ping in `WsClient` via `tokio::select!`
- [x] Add exponential backoff with jitter reconnection in `WsClient::run`
- [x] Define `WsRawMessage` struct with `text` and `recv_timestamp_us`
- [x] Implement `RestClient::fetch_book` with rate limiting
- [x] Implement `RestClient::discover_markets` for Gamma API
- [x] Implement `Dispatcher` with `WsMessage` deserialization (Book, PriceChange, LastTradePrice)
- [x] Add fixed-point price/size conversion and monotonic sequence assignment in `Dispatcher`
- [x] Wire `mpsc` channels between `WsClient`, `Dispatcher`, and downstream consumers
