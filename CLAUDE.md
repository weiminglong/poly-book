# CLAUDE.md

## Project
Poly-book: Rust workspace for Polymarket market-data ingestion, replay, storage,
and a read-only workstation API. Cargo workspace with crates under `crates/`.

## Build
```bash
cargo check                                          # type-check all crates
cargo test --workspace --exclude pb-integration-tests # unit + property tests
cargo bench                                          # Criterion benchmarks (pb-types, pb-book)
cargo +nightly fuzz run fuzz_book_delta               # fuzz targets (requires nightly)
cargo +nightly fuzz run fuzz_query_guard              # query guard fuzz target (requires nightly)
cargo run -- --help                                  # CLI binary
```

## Architecture
Single-threaded book updates, no locks on hot path. Components communicate via bounded `tokio::mpsc` channels.

```
pb-feed (WS/REST) -> dispatcher -> pb-wal -> pb-store (Parquet + ClickHouse)
                    |                 |                  ^
                    v                 v                  |
                 pb-api <-- pb-service <-- pb-replay ----+
```

Full system diagram, crate dependency graph, and runtime topology:
[docs/architecture.md](docs/architecture.md).

## Crates
- **pb-api**: Read-only HTTP API with watch-based read model and per-asset WebSocket broadcasts. Thin HTTP adapters delegate to `pb-service` traits. Routes: feed status, active assets, live snapshots, replay, integrity, execution, streaming.
- **pb-types**: Foundation types. `FixedPrice(u32)` scaled by 10,000, `FixedSize(u64)` scaled by 1,000,000. Persisted record model includes split datasets such as `BookEvent`, `TradeEvent`, `IngestEvent`, `BookCheckpoint`, `ReplayValidation`, and `ExecutionEvent`.
- **pb-book**: `L2Book` using `BTreeMap<Reverse<FixedPrice>, FixedSize>` for bids (best-first iteration). Methods: `apply_snapshot`, `apply_delta`, `best_bid/ask`, `mid_price`, `spread`, `weighted_mid_price`, `total_bid_size/total_ask_size`, `top_bids/top_asks`, `check_integrity`, `check_sequence`.
- **pb-feed**: `WsClient` (reconnect with exp backoff + jitter), `RestClient` (with `RateLimiter` via governor), `Dispatcher` (deser + normalize to split `PersistedRecord` events). Dispatcher uses `FxHashMap` for hot-path lookups.
- **pb-store**: `ParquetSink` (5-min flush, Zstd, `object_store` abstraction) and `ClickHouseSink` (1s batch, `MergeTree`).
- **pb-replay**: `EventReader` trait with `ParquetReader`/`ClickHouseReader`. `ReplayEngine` reconstructs book at any timestamp. `run_backfill` for periodic REST snapshots.
- **pb-wal**: Embedded write-ahead log. BufWriter-wrapped append-only segments with length-prefix + CRC32C framing. `WalWriter` appends, rotates, and reclaims sealed segments via `prune`/`prune_with_backpressure` (count-capped by `max_segments`, single-writer `flock`); `WalReader` tails with independent consumer positions. Versioned codec (`pb_wal::codec`) for forward-compatible `PersistedRecord` serialization.
- **pb-service**: Transport-neutral domain service layer. Defines `BookService`, `ReplayService`, `IntegrityService`, `ExecutionService`, `QueryService` traits. Concrete implementations for Parquet and ClickHouse backends. Enum dispatch (`AnyReplayService`, `AnyIntegrityService`, `AnyExecutionService`, `AnyQueryService`) for configurable backend selection.
- **pb-grpc**: gRPC read surface using tonic. Exposes `WorkstationService` with `Reconstruct`, `IntegritySummary`, and `ExecutionTimeline` RPCs. Delegates to `pb-service` traits. Configurable via `[grpc]` config section (disabled by default, port 50051).
- **pb-metrics**: Prometheus counters/histograms via `metrics` crate, axum HTTP `/metrics` endpoint.
- **pb-bin**: CLI with clap subcommands including `discover`, `ingest`, `auto-ingest`, `replay`, `backfill`, `execution-replay`, `execution-append`, `serve-api`, and `serve`. Process separation: `ingest` (feed + WAL + sinks), `serve` (checkpoint hydration + WAL tail + API), `serve-api` (combined, no WAL). Layered config: `config/default.toml` -> env (`PB__` prefix) -> CLI args.

## Per-Crate Documentation
Each crate has a `README.md` at its root with: purpose, key types, data flow,
design notes, and a **Docs to Update After Changes** table.

**MANDATORY**: Before modifying a crate, read its README. After making changes:
1. Update the crate's own README if you changed its public API, types, or behavior
2. Check the **Docs to Update After Changes** table and propagate to every listed
   target (docs/, config, CLAUDE.md, other crate READMEs, web/)
3. Do not consider a task complete until doc propagation is done

## Conventions
- Fixed-point over floating-point for prices and sizes — never use `f64` for orderbook state
- Channel-based message passing between components — no `Arc<Mutex<_>>`
- `thiserror` for library crate errors, `anyhow` only in pb-bin
- `tracing` for structured logging (not `log` or `println!`)
- Wire types borrow from raw buffers (`&'a str`) for zero-copy deserialization
- Storage uses `object_store` trait for filesystem abstraction (local FS / S3 / GCS)
- `mimalloc` as global allocator in pb-bin for lower p99 latency
- `FxHashMap` (rustc-hash) for internal hot-path lookups in trusted-data contexts
- `proptest` for property-based invariant testing in pb-types and pb-book
- Error variants use structured fields with operational context (asset_id, expected/got, url)
- ADRs in `docs/adr/` document key architectural decisions

## Config
`config/default.toml` with sections: `[feed]`, `[storage]`, `[metrics]`, `[wal]`, `[api]`, `[grpc]`, `[logging]`. Environment override prefix: `PB__` with `__` separator (e.g. `PB__STORAGE__CLICKHOUSE_URL`).

## Read This First
If you are working on the workstation API, frontend, or runtime boundaries, read
these in order:

1. `docs/serve-api.md` — current runtime purpose, constraints, and deferred scope
2. `docs/api.md` — current route contract and error semantics
3. `docs/operations.md` — config, ports, and local run commands
4. `openspec/changes/archive/2026-03-07-quant-workstation-platform/` — archived workstation scope and future module boundaries

If you are changing replay, storage, or integrity semantics, also read:

1. `openspec/changes/archive/2026-03-06-market-data-upgrades/`
2. `docs/operations.md`

## Persisting Design Decisions
Do not leave major boundary decisions only in chat history.

When you change the workstation/API/runtime scope:

- update `docs/serve-api.md` for runtime behavior and constraints
- update `docs/api.md` for route shape and error semantics
- update `docs/operations.md` for commands, config, and ports
- update `README.md` if contributor discovery changes
- update the archived OpenSpec change under `openspec/changes/archive/2026-03-07-quant-workstation-platform/` if scope boundaries change

When you implement only part of a planned capability, explicitly document what
shipped and what remains deferred. Keep the docs and OpenSpec honest about
current boundaries.

## Current Workstation Boundary
The workstation backend is read-only with process separation:

- `ingest` process: feed → dispatcher → WAL + storage sinks (Parquet, ClickHouse)
- `serve` process: checkpoint hydration → WAL tail → watch-based read model → HTTP/WS
- `serve-api`: combined mode (feed + API in one process, no WAL)
- configurable backend: `api.historical_backend = "parquet" | "clickhouse"` with auto-fallback
- optional gRPC surface: `grpc.enabled = true` exposes WorkstationService on port 50051
- WAL coordination: gap detection, lag tracking, backpressure pruning for multi-replica setups
- current routes:
  - `GET /api/v1/feed/status`
  - `GET /api/v1/assets/active`
  - `GET /api/v1/assets/resolve`
  - `GET /api/v1/orderbooks/{asset_id}/snapshot`
  - `GET /api/v1/replay/reconstruct`
  - `GET /api/v1/integrity/summary`
  - `GET /api/v1/execution/orders`
  - `GET /health`
  - `GET /api/v1/query/datasets`
  - `POST /api/v1/query/sql`
  - `WS /api/v1/streams/orderbook?asset_id=...`

Deferred for later phases:

- latency summary routes

## CI Required Checks
The following jobs in `.github/workflows/ci.yml` should pass before merging:

- `check` — `cargo check --all-targets`
- `test` — `cargo test --workspace --exclude pb-integration-tests`
- `clippy` — `cargo clippy --all-targets -- -D warnings`
- `fmt` — `cargo fmt --all -- --check`
- `audit` — rustsec dependency vulnerability scan
- `web` — `biome check` + `tsc -b` + `vitest run` + `vite build`
- `e2e` — Playwright end-to-end tests (depends on `web` build artifact)
- `fuzz` — 30s smoke fuzz runs (WAL corruption, book delta); additional local target `fuzz_query_guard`
- `miri` — undefined behavior checks on pb-types, pb-book

Additional workflows: `codeql.yml` (SAST), `supply-chain.yml` (cargo-deny).

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

For workstation work, refer to the archived change
`openspec/changes/archive/2026-03-07-quant-workstation-platform/` for scope and
module boundary definitions.
