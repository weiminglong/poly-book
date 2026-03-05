# CLAUDE.md

## Project
Poly-book: high-performance Polymarket BTC 5-minute orderbook system in Rust. Cargo workspace with 7 crates under `crates/`.

## Build
```bash
cargo check          # type-check all crates
cargo test           # run all tests (29 tests)
cargo bench          # run Criterion benchmarks (pb-types, pb-book)
cargo run -- --help  # CLI binary
```

## Architecture
Single-threaded book updates, no locks on hot path. Components communicate via bounded `tokio::mpsc` channels.

```
pb-feed (WS/REST) -> dispatcher -> pb-book (L2Book) -> pb-store (Parquet + ClickHouse)
```

## Crates
- **pb-types**: Foundation types. `FixedPrice(u32)` scaled by 10,000, `FixedSize(u64)` scaled by 1,000,000. Zero-copy wire types with `serde(borrow)`. `OrderbookEvent`, `AssetId`, `Sequence`.
- **pb-book**: `L2Book` using `BTreeMap<Reverse<FixedPrice>, FixedSize>` for bids (best-first iteration). Methods: `apply_snapshot`, `apply_delta`, `best_bid/ask`, `mid_price`, `spread`.
- **pb-feed**: `WsClient` (reconnect with exp backoff + jitter), `RestClient` (with `RateLimiter` via governor), `Dispatcher` (deser + normalize to `OrderbookEvent`).
- **pb-store**: `ParquetSink` (5-min flush, Zstd, `object_store` abstraction) and `ClickHouseSink` (1s batch, `ReplacingMergeTree`).
- **pb-replay**: `EventReader` trait with `ParquetReader`/`ClickHouseReader`. `ReplayEngine` reconstructs book at any timestamp. `run_backfill` for periodic REST snapshots.
- **pb-metrics**: Prometheus counters/histograms via `metrics` crate, axum HTTP `/metrics` endpoint.
- **pb-bin**: CLI with clap subcommands: `discover`, `ingest`, `replay`, `backfill`. Layered config: `config/default.toml` -> env (`PB__` prefix) -> CLI args.

## Conventions
- Fixed-point over floating-point for prices and sizes — never use `f64` for orderbook state
- Channel-based message passing between components — no `Arc<Mutex<_>>`
- `thiserror` for library crate errors, `anyhow` only in pb-bin
- `tracing` for structured logging (not `log` or `println!`)
- Wire types borrow from raw buffers (`&'a str`) for zero-copy deserialization
- Storage uses `object_store` trait for filesystem abstraction (local FS / S3 / GCS)

## Config
`config/default.toml` with sections: `[feed]`, `[storage]`, `[metrics]`, `[logging]`. Environment override prefix: `PB__` with `__` separator (e.g. `PB__STORAGE__CLICKHOUSE_URL`).

## OpenSpec
Spec-driven development artifacts in `openspec/changes/archive/`. Each change has: `proposal.md`, `design.md`, `specs/*/spec.md`, `tasks.md`.
