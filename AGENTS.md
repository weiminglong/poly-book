# AGENTS.md

## Project Bootstrap

`poly-book` is a Rust workspace for Polymarket market-data ingestion, replay,
storage, and a read-only workstation API.

If you are working on the workstation API, frontend, or runtime boundaries,
read these first:

1. `docs/serve-api.md`
2. `docs/api.md`
3. `docs/operations.md`
4. `openspec/changes/2026-03-07-quant-workstation-platform/`

If you are changing replay, storage, or integrity semantics, also read:

1. `openspec/changes/2026-03-06-market-data-upgrades/`
2. `docs/operations.md`

## Current Workstation Boundary

The current Phase 3 backend is intentionally narrow:

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

## Persisting Decisions

Do not leave major design or scope decisions only in chat history.

When workstation/API/runtime scope changes, update:

- `docs/serve-api.md` for runtime behavior and constraints
- `docs/api.md` for route shape and error semantics
- `docs/operations.md` for commands, config, and ports
- `README.md` if contributor-facing discovery changes
- the active OpenSpec change under `openspec/changes/2026-03-07-quant-workstation-platform/`

When only part of a planned capability ships, document what shipped and what
remains deferred.

## Build And Validation

Use the smallest command that validates your change:

```bash
cargo check
cargo test --workspace --exclude pb-integration-tests
```

## Cursor Cloud specific instructions

### System dependency

`libssl-dev` and `pkg-config` are required for `openssl-sys` (pulled in by
`reqwest`/`tokio-tungstenite`). The update script installs them automatically.

### Running the application

- `cargo run -- discover --filter btc --limit 5` — quick smoke test that hits the
  live Polymarket Gamma API. Requires internet access.
- `cargo run -- serve-api --auto-rotate` — starts the read-only workstation API on
  `:3000` and a Prometheus metrics server on `:9090`. Use `--tokens <ID>` instead
  of `--auto-rotate` if you already know a token ID.

### Lint / test / build

All standard commands are in `CLAUDE.md` and `README.md`. Key shortcuts:

- `cargo fmt --all -- --check` — formatting
- `cargo clippy --workspace -- -D warnings` — lints
- `cargo test --workspace --exclude pb-integration-tests` — unit tests (no Docker)
- `cargo build` — dev build

Integration tests (`pb-integration-tests`) require Docker and ClickHouse via
`testcontainers`; skip them in Cloud environments without Docker.

### Configuration

Default config is at `config/default.toml`. Override with `PB__` env vars
(double-underscore separator). See `docs/operations.md` for details.
