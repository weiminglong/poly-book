# CLAUDE.md

## Project
Poly-book: Rust workspace for Polymarket market-data ingestion, replay, storage,
and a read-only workstation API. Cargo workspace with crates under `crates/`.

## Build
```bash
cargo check          # type-check all crates
cargo test           # run workspace tests
cargo bench          # run Criterion benchmarks (pb-types, pb-book)
cargo run -- --help  # CLI binary
```

## Architecture
Single-threaded book updates, no locks on hot path. Components communicate via bounded `tokio::mpsc` channels.

```
pb-feed (WS/REST) -> dispatcher -> pb-store (Parquet + ClickHouse)
                    |                         ^
                    v                         |
                 pb-api <------ pb-replay ----+
```

## Crates
- **pb-api**: Read-only HTTP API and live read model for workstation clients. Current routes: feed status, active assets, live snapshots, Parquet-backed replay reconstruction.
- **pb-types**: Foundation types. `FixedPrice(u32)` scaled by 10,000, `FixedSize(u64)` scaled by 1,000,000. Persisted record model includes split datasets such as `BookEvent`, `TradeEvent`, `IngestEvent`, `BookCheckpoint`, `ReplayValidation`, and `ExecutionEvent`.
- **pb-book**: `L2Book` using `BTreeMap<Reverse<FixedPrice>, FixedSize>` for bids (best-first iteration). Methods: `apply_snapshot`, `apply_delta`, `best_bid/ask`, `mid_price`, `spread`.
- **pb-feed**: `WsClient` (reconnect with exp backoff + jitter), `RestClient` (with `RateLimiter` via governor), `Dispatcher` (deser + normalize to split `PersistedRecord` events).
- **pb-store**: `ParquetSink` (5-min flush, Zstd, `object_store` abstraction) and `ClickHouseSink` (1s batch, `ReplacingMergeTree`).
- **pb-replay**: `EventReader` trait with `ParquetReader`/`ClickHouseReader`. `ReplayEngine` reconstructs book at any timestamp. `run_backfill` for periodic REST snapshots.
- **pb-metrics**: Prometheus counters/histograms via `metrics` crate, axum HTTP `/metrics` endpoint.
- **pb-bin**: CLI with clap subcommands including `discover`, `ingest`, `auto-ingest`, `replay`, `backfill`, `execution-replay`, and `serve-api`. Layered config: `config/default.toml` -> env (`PB__` prefix) -> CLI args.

## Conventions
- Fixed-point over floating-point for prices and sizes — never use `f64` for orderbook state
- Channel-based message passing between components — no `Arc<Mutex<_>>`
- `thiserror` for library crate errors, `anyhow` only in pb-bin
- `tracing` for structured logging (not `log` or `println!`)
- Wire types borrow from raw buffers (`&'a str`) for zero-copy deserialization
- Storage uses `object_store` trait for filesystem abstraction (local FS / S3 / GCS)

## Config
`config/default.toml` with sections: `[feed]`, `[storage]`, `[metrics]`, `[api]`, `[logging]`. Environment override prefix: `PB__` with `__` separator (e.g. `PB__STORAGE__CLICKHOUSE_URL`).

## Read This First
If you are working on the workstation API, frontend, or runtime boundaries, read
these in order:

1. `docs/serve-api.md` — current runtime purpose, constraints, and deferred scope
2. `docs/api.md` — current route contract and error semantics
3. `docs/operations.md` — config, ports, and local run commands
4. `openspec/changes/2026-03-07-quant-workstation-platform/` — source of truth for current workstation scope and future module boundaries

If you are changing replay, storage, or integrity semantics, also read:

1. `openspec/changes/2026-03-06-market-data-upgrades/`
2. `docs/operations.md`

## Persisting Design Decisions
Do not leave major boundary decisions only in chat history.

When you change the workstation/API/runtime scope:

- update `docs/serve-api.md` for runtime behavior and constraints
- update `docs/api.md` for route shape and error semantics
- update `docs/operations.md` for commands, config, and ports
- update `README.md` if contributor discovery changes
- update the active OpenSpec change under `openspec/changes/2026-03-07-quant-workstation-platform/`

When you implement only part of a planned capability, explicitly document what
shipped and what remains deferred. Keep the docs and OpenSpec honest about
current boundaries.

## Current Workstation Boundary
The current Phase 3 workstation backend is intentionally narrow:

- read-only only
- Parquet-first replay source
- no live data persistence in `serve-api`
- current routes:
  - `GET /api/v1/feed/status`
  - `GET /api/v1/assets/active`
  - `GET /api/v1/orderbooks/{asset_id}/snapshot`
  - `GET /api/v1/replay/reconstruct`

Deferred for later phases:

- integrity summary routes
- execution timeline routes
- SQL workbench routes
- WebSocket order book streaming
- ClickHouse-backed API reads
- frontend SPA implementation

## Git Workflow
- **Branch**: `feat/`, `fix/`, `docs/` prefix with kebab-case (e.g. `feat/discover-btc-5m-slug-lookup`)
- **Commit**: imperative sentence, PR number suffix (e.g. `Fix discover command ... (#6)`)
- **PR**: squash-merge into `main`, auto-delete branch on merge is enabled
- Always run `cargo test` before committing

## OpenSpec
Spec-driven development artifacts live under `openspec/changes/`.

- Active changes define current and upcoming work
- Archived changes live under `openspec/changes/archive/`
- Each change has: `proposal.md`, `design.md`, `specs/*/spec.md`, `tasks.md`

For workstation work, prefer the active change
`openspec/changes/2026-03-07-quant-workstation-platform/` rather than only the
archive.
