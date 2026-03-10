# AGENTS.md

## Project Bootstrap

`poly-book` is a Rust workspace for Polymarket market-data ingestion, replay,
storage, and a read-only workstation API. Crates live under `crates/`.

If you are working on the workstation API, frontend, or runtime boundaries,
read these first:

1. `docs/serve-api.md`
2. `docs/api.md`
3. `docs/operations.md`

If you are changing replay, storage, or integrity semantics, also read:

1. `openspec/changes/archive/2026-03-06-market-data-upgrades/`
2. `docs/operations.md`

## Updating Docs After Changes (MANDATORY)

Every crate has a `README.md` with a **Docs to Update After Changes** table.

After modifying any crate:
1. Update the crate's own README if you changed its public API, types, or behavior
2. Check the **Docs to Update After Changes** table and propagate to every listed
   target (docs/, config, CLAUDE.md, other crate READMEs, web/)
3. Do not consider a task complete until doc propagation is done

## Current Workstation Boundary

The workstation backend is read-only with process separation:

- `ingest` process: feed → dispatcher → WAL + storage sinks (Parquet, ClickHouse)
- `serve` process: checkpoint hydration → WAL tail → watch-based read model → HTTP/WS
- `serve-api`: combined mode (feed + API in one process, no WAL)
- configurable backend: `api.historical_backend = "parquet" | "clickhouse"`
- optional gRPC surface: `grpc.enabled = true` on port 50051
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

## Persisting Decisions

Do not leave major design or scope decisions only in chat history.

When workstation/API/runtime scope changes, update:

- `docs/serve-api.md` for runtime behavior and constraints
- `docs/api.md` for route shape and error semantics
- `docs/operations.md` for commands, config, and ports
- `README.md` if contributor-facing discovery changes
- the archived OpenSpec change under `openspec/changes/archive/2026-03-07-quant-workstation-platform/`

When only part of a planned capability ships, document what shipped and what
remains deferred.

## Build And Validation

Use the smallest command that validates your change:

```bash
cargo check                                          # type-check
cargo test --workspace --exclude pb-integration-tests # unit + property tests
cargo clippy --all-targets -- -D warnings             # lints
cargo fmt --all -- --check                            # formatting
```

Integration tests (`pb-integration-tests`) require Docker and ClickHouse via
`testcontainers`; skip them in environments without Docker.

Fuzz targets (`fuzz/`) require `cargo-fuzz` and a nightly toolchain; skip
unless specifically requested.

CI requires `protobuf-compiler` (apt) for `pb-grpc` builds.

## Conventions

- Fixed-point over floating-point for prices and sizes (never `f64` for orderbook state)
- Channel-based message passing between components (no `Arc<Mutex<_>>`)
- `thiserror` for library crate errors, `anyhow` only in pb-bin
- `tracing` for structured logging (not `log` or `println!`)
- `object_store` trait for filesystem abstraction (local FS / S3 / GCS)

## Configuration

Default config is at `config/default.toml` with sections: `[feed]`,
`[storage]`, `[metrics]`, `[wal]`, `[api]`, `[grpc]`, `[logging]`.

Override with `PB__` env vars (double-underscore separator, e.g.
`PB__API__HISTORICAL_BACKEND=clickhouse`).

## Running

```bash
cargo run -- discover --filter btc --limit 5          # smoke test (live API)
cargo run -- serve-api --auto-rotate                   # workstation on :3000
cargo run -- serve-api --tokens <TOKEN_ID>             # fixed token mode
```
