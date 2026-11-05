# CLAUDE.md

## Project
Poly-book: Rust workspace for Polymarket market-data ingestion, replay, storage,
and a read-only workstation API. Cargo workspace with crates under `crates/`.

## Build
```bash
cargo check                                          # type-check all crates
cargo test --workspace --exclude pb-integration-tests # unit + property tests
cargo bench                                          # Criterion benchmarks (pb-types, pb-book, pb-feed, pb-wal, pb-api)
cargo +nightly fuzz run fuzz_book_delta               # fuzz targets (requires nightly)
cargo +nightly fuzz run fuzz_codec_decode             # WAL codec decode fuzz target (requires nightly)
cargo run -- --help                                  # CLI binary
```

## Architecture
Single-threaded book updates, no locks on hot path. Components communicate via bounded `tokio::mpsc` channels.

```
pb-feed (WS/REST) -> dispatcher -> pb-wal -> pb-store (Parquet)
                    |                 |                  ^
                    v                 v                  |
                 pb-api <-- pb-service <-- pb-replay ----+
```

Full system diagram, crate dependency graph, and runtime topology:
[docs/architecture.md](docs/architecture.md).

## Crates
- **pb-api**: Read-only HTTP API with watch-based read model and per-asset WebSocket broadcasts. Thin HTTP adapters delegate to `pb_service::HistoricalService`. Routes: feed status, active assets, live snapshots, replay, integrity, streaming.
- **pb-types**: Foundation types. `FixedPrice(u32)` scaled by 10,000, `FixedSize(u64)` scaled by 1,000,000. Persisted record model is five split datasets: `BookEvent`, `TradeEvent`, `IngestEvent`, `BookCheckpoint`, and `ReplayValidation`.
- **pb-book**: `L2Book` using `BTreeMap<Reverse<FixedPrice>, FixedSize>` for bids (best-first iteration). Methods: `apply_snapshot`, `apply_delta`, `best_bid/ask`, `mid_price`, `spread`, `weighted_mid_price`, `total_bid_size/total_ask_size`, `top_bids/top_asks`, `check_integrity`, `check_sequence`.
- **pb-feed**: `WsClient` (reconnect with exp backoff + jitter), `RestClient` (with `RateLimiter` via governor), `Dispatcher` (deser + normalize to split `PersistedRecord` events). Dispatcher uses `FxHashMap` for hot-path lookups.
- **pb-store**: Parquet storage sink and writer. `ParquetSink` (5-min flush, Zstd, `object_store` abstraction) and `ParquetRecordWriter`.
- **pb-replay**: `EventReader` trait with `ParquetReader`. `ReplayEngine` reconstructs book at any timestamp. `run_backfill` for periodic REST snapshots.
- **pb-wal**: Embedded write-ahead log. BufWriter-wrapped append-only segments with length-prefix + CRC32C framing. `WalWriter` appends, rotates, and reclaims sealed segments via `prune`/`prune_with_backpressure` (count-capped by `max_segments`, single-writer `flock`); `WalReader` tails with independent consumer positions. Versioned codec (`pb_wal::codec`) for forward-compatible `PersistedRecord` serialization.
- **pb-service**: Transport-neutral domain service layer. One struct, `HistoricalService` (`new(base_path)` / `from_reader(ParquetReader)`), with async `reconstruct(asset_id, at_us, mode, depth)` and `integrity_summary(asset_id, start_us, end_us)` (24h window enforced via `validate_time_window`). Domain types: `ReplayResult`, `IntegritySummary`, `ContinuityEvent`, `CompletenessLevel`, `ServiceError`. Built in pb-bin via `pipeline::build_historical_service(&settings)`; held in `pb_api::AppState::historical`.
- **pb-metrics**: Prometheus counters/histograms via `metrics` crate, axum HTTP `/metrics` endpoint.
- **pb-bin**: CLI with clap subcommands including `discover`, `ingest` (`--tokens` or `--auto-rotate`), `replay`, `backfill`, `serve-api`, `serve`, `demo` (replay a committed capture as a simulated live feed behind the full API, offline), `doctor` (config + connectivity preflight, exits non-zero on failure), and `reconcile` (offline WAL→Parquet rebuild for crash recovery). Process separation: `ingest` (feed + WAL + sinks), `serve` (checkpoint hydration + WAL tail + API), `serve-api` (combined, no WAL). Layered config: `config/default.toml` -> env (`PB__` prefix) -> CLI args.

## Per-Crate Documentation
Each crate has a `README.md` at its root with: purpose, key types, data flow,
and design notes. Read it before modifying the crate, and update it if you
change the crate's public API or behavior.

Each fact has one home. Do not restate it elsewhere; link instead:
- routes and error semantics: `docs/api.md`
- config keys and defaults: `config/default.toml` (commented)
- commands, ports, data layout: `docs/operations.md`
- runtime modes and trust boundary: `docs/serve-api.md`
- crate graph and data flow: `docs/architecture.md`
- design decisions and rejected alternatives: `docs/adr/`

## Conventions
- Fixed-point over floating-point for prices and sizes — never use `f64` for orderbook state
- Channel-based message passing between components — no `Arc<Mutex<_>>`
- `thiserror` for library crate errors, `anyhow` only in pb-bin
- `tracing` for structured logging (not `log` or `println!`)
- Wire types borrow from raw buffers (`&'a str`) for zero-copy deserialization
- Storage uses the `object_store` trait for filesystem abstraction (local FS / S3)
- `mimalloc` as global allocator in pb-bin for lower p99 latency
- `FxHashMap` (rustc-hash) for internal hot-path lookups in trusted-data contexts
- `proptest` for property-based invariant testing in pb-types and pb-book
- Error variants use structured fields with operational context (asset_id, expected/got, url)
- ADRs in `docs/adr/` document key architectural decisions

## Config
`config/default.toml` with sections: `[feed]`, `[storage]`, `[metrics]`, `[wal]`, `[api]`, `[logging]`. Environment override prefix: `PB__` with `__` separator (e.g. `PB__STORAGE__PARQUET_BASE_PATH`).

## Read This First
If you are working on the workstation API, frontend, or runtime boundaries, read
these in order:

1. `docs/serve-api.md` — current runtime purpose, constraints, and deferred scope
2. `docs/api.md` — current route contract and error semantics
3. `docs/operations.md` — config, ports, and local run commands

If you are changing replay, storage, or integrity semantics, also read
`docs/adr/0008` through `0011` and `docs/operations.md`.

## Persisting Design Decisions
Do not leave major boundary decisions only in chat history.

When you change the workstation/API/runtime scope:

- update `docs/serve-api.md` for runtime behavior and constraints
- update `docs/api.md` for route shape and error semantics
- update `docs/operations.md` for commands, config, and ports
- update `README.md` if contributor discovery changes
- add or amend an ADR in `docs/adr/` if a boundary decision changes

When you implement only part of a planned capability, explicitly document what
shipped and what remains deferred. Keep the docs honest about current
boundaries.

## Current Workstation Boundary
The workstation backend is read-only with process separation:

- `ingest` process: feed → dispatcher → WAL + Parquet storage sink
- `serve` process: checkpoint hydration → WAL tail → watch-based read model → HTTP/WS
- `serve-api`: combined mode (feed + API in one process, no WAL)
- historical routes (replay, integrity) are Parquet-backed via `pb_service::HistoricalService`
- WAL coordination: gap detection, lag tracking, backpressure pruning for multi-replica setups
- current routes:
  - `GET /api/v1/feed/status`
  - `GET /api/v1/assets/active`
  - `GET /api/v1/assets/resolve`
  - `GET /api/v1/orderbooks/{asset_id}/snapshot`
  - `GET /api/v1/replay/reconstruct`
  - `GET /api/v1/integrity/summary`
  - `GET /health`, `GET /health/live`, `GET /health/ready`
  - `WS /api/v1/streams/orderbook?asset_id=...`

Deferred for later phases:

- latency summary routes

## CI Required Checks
The following jobs in `.github/workflows/ci.yml` should pass before merging:

- `check` — `cargo check --all-targets`
- `test` — `cargo test --workspace --exclude pb-integration-tests` plus the non-Docker integration suite
- `integration-docker` — the #[ignore]d Docker-backed integration case (S3 round-trip through the production `ParquetReader` against an S3 mock)
- `clippy` — `cargo clippy --all-targets -- -D warnings`
- `fmt` — `cargo fmt --all -- --check`
- `monitoring` — `promtool check rules` + `promtool test rules` (alert-rule syntax + offline incident unit tests)
- `docker-build` — builds the production image (release profile, bundled SPA)
- `web` — `biome check` + `tsc -b` + `vitest run` + `vite build`
- `e2e` — Playwright end-to-end tests (depends on `web` build artifact)

Additional workflows: `fuzz.yml` (30s smoke runs of all five fuzz targets, nightly and on pushes to `main`), `supply-chain.yml` (cargo-deny: advisories, bans, licenses, sources; on PRs and weekly), `release.yml` (tags), `pages.yml` (hosted demo).

## Git Workflow
- **Branch**: `feat/`, `fix/`, `docs/` prefix with kebab-case (e.g. `feat/discover-btc-5m-slug-lookup`)
- **Commit**: imperative sentence, PR number suffix (e.g. `Fix discover command ... (#6)`)
- **PR**: squash-merge into `main`, auto-delete branch on merge is enabled
- Always run `cargo test` before committing
