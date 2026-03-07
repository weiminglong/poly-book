# Cloud Agent Starter Skill — poly-book

Use this skill whenever you need to build, run, or test the poly-book codebase
in a Cloud Agent environment. It covers environment setup, per-area testing
workflows, and the workstation API dev loop.

---

## 1. Environment Bootstrap

### Toolchain

The repo pins Rust **1.94.0** via `rust-toolchain.toml` (includes `rustfmt`
and `clippy`). On a fresh Cloud Agent VM, Rust is typically pre-installed but
may be an older version. If the pinned toolchain is missing, `rustup` will
auto-install it on first `cargo` invocation. To install manually:

```bash
rustup toolchain install 1.94.0
```

If `cargo` itself is missing:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
```

### First build

```bash
cargo check                 # fast type-check, validates deps resolve
cargo test --workspace --exclude pb-integration-tests   # unit + non-Docker integration tests
```

`cargo check` compiles every crate without linking, completing in roughly
1–2 minutes on a cold cache. Subsequent incremental checks are much faster.

### justfile shortcuts

The repo ships a `justfile` (requires `just`). Install if missing:

```bash
cargo install just
```

Key recipes:

| Recipe | What it does |
|--------|-------------|
| `just check` | `cargo check` |
| `just test` | `cargo test` (all, including integration) |
| `just clippy` | `cargo clippy --workspace -- -D warnings` |
| `just fmt` | `cargo fmt --all` |
| `just ci` | fmt-check → clippy → test (mirrors GitHub CI) |

### CI gate (what must pass before push)

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace --exclude pb-integration-tests
```

These match the four CI jobs in `.github/workflows/ci.yml`. The env var
`RUSTFLAGS=-Dwarnings` is set in CI, so any compiler warnings are errors.

---

## 2. Configuration & Environment Variables

### Config layering (highest priority wins)

1. CLI flags
2. Environment variables with prefix `PB__` and `__` separator
3. `config/default.toml`

### Useful overrides for local development

```bash
# Quieter or louder logs
export PB__LOGGING__LEVEL=debug          # or trace, warn, error
# Alternative: RUST_LOG=pb_api=debug,pb_replay=trace

# Change data directory
export PB__STORAGE__PARQUET_BASE_PATH=/tmp/pb-test-data

# Move API to a different port
export PB__API__LISTEN_ADDR=0.0.0.0:3001

# Move metrics to a different port
export PB__METRICS__LISTEN_ADDR=0.0.0.0:9091
```

### Default ports

| Service | Port | Config key |
|---------|------|-----------|
| API | 3000 | `api.listen_addr` |
| Metrics | 9090 | `metrics.listen_addr` |

### No feature flags or login required

The codebase has no feature-flag system, authentication, or login flow.
All functionality is toggled via CLI subcommands and config values.
ClickHouse storage is off by default (`--clickhouse false`).

---

## 3. Testing by Codebase Area

### 3a. Foundation types (`pb-types`)

Unit tests cover `FixedPrice`, `FixedSize`, wire deserialization, and event
construction.

```bash
cargo test -p pb-types
```

Benchmarks (optional):

```bash
cargo bench -p pb-types
```

### 3b. Order book (`pb-book`)

Tests cover snapshot apply, delta apply, best bid/ask, mid-price, spread,
and edge cases (empty book, crossed book).

```bash
cargo test -p pb-book
```

Benchmarks (optional):

```bash
cargo bench -p pb-book
```

### 3c. Feed layer (`pb-feed`)

Tests cover the dispatcher (message splitting into book/trade/ingest events),
rate limiter, and REST client response parsing. These are pure unit tests
with no network calls.

```bash
cargo test -p pb-feed
```

### 3d. Storage (`pb-store`)

No standalone crate-level unit tests. Storage is tested via integration tests
(see §3g).

### 3e. Replay (`pb-replay`)

No standalone crate-level unit tests. Replay is tested via integration tests
(see §3g).

### 3f. API (`pb-api`)

Unit tests cover route handlers and the live read-model. These use in-process
axum test utilities — no running server needed.

```bash
cargo test -p pb-api
```

### 3g. Integration tests

Located in `tests/integration/`. Run them (excluding Docker-dependent ones):

```bash
cargo test --workspace --exclude pb-integration-tests
```

Wait — the above *excludes* the integration package entirely. To run the
non-Docker integration tests explicitly:

```bash
cargo test -p pb-integration-tests \
  --test book_determinism \
  --test parquet_roundtrip \
  --test dispatcher_pipeline \
  --test replay_engine \
  --test schema_conversion
```

These tests use `tempfile::tempdir()` for Parquet I/O and build test data
in-memory — no external services or fixtures required.

#### ClickHouse integration tests (Docker required)

These are `#[ignore]`-tagged and require a running Docker daemon:

```bash
cargo test -p pb-integration-tests --test clickhouse_roundtrip -- --ignored
```

They use `testcontainers` to spin up an ephemeral ClickHouse container. Skip
these in Cloud Agent runs unless Docker is available.

### 3h. Binary / CLI (`pb-bin`)

The binary crate has minimal tests (one for auto-ingest discovery filtering).

```bash
cargo test -p pb-bin
```

To verify the CLI builds and prints help:

```bash
cargo run -- --help
```

---

## 4. Running the Workstation API Locally

The `serve-api` subcommand starts a read-only API backed by a live WebSocket
feed and Parquet-based replay.

### Fixed-token mode

```bash
cargo run -- serve-api --tokens <TOKEN_ID>
```

### Auto-rotate mode

```bash
cargo run -- serve-api --auto-rotate
```

Both modes start the API on port 3000 and metrics on port 9090 by default.

### Smoke-testing the API

Once the server is running:

```bash
# Feed status
curl -s http://localhost:3000/api/v1/feed/status | jq .

# Active assets
curl -s http://localhost:3000/api/v1/assets/active | jq .

# Live snapshot (requires an active asset_id from the feed)
curl -s "http://localhost:3000/api/v1/orderbooks/<ASSET_ID>/snapshot?depth=10" | jq .

# Replay reconstruct (requires Parquet data on disk)
curl -s "http://localhost:3000/api/v1/replay/reconstruct?asset_id=<ID>&at_us=<TS>&mode=recv_time" | jq .
```

### Note on network dependencies

`serve-api` connects to the live Polymarket WebSocket and REST endpoints
(`wss://ws-subscriptions-clob.polymarket.com`, `https://clob.polymarket.com`,
`https://gamma-api.polymarket.com`). In a Cloud Agent without outbound WS
access, the feed will fail to connect and retry with exponential backoff. The
API server will still start and serve 503s for live endpoints. Replay routes
will work if Parquet data exists on disk.

---

## 5. Inspecting Parquet Data

If the data directory (`./data` by default) contains ingested Parquet files:

```bash
# List files
find ./data -name '*.parquet'

# Peek with DuckDB (if installed)
duckdb -c "SELECT * FROM './data/**/*.parquet' LIMIT 20"
```

Layout: `data/<dataset>/<year>/<month>/<day>/<hour>/*.parquet`

Datasets: `book_events`, `trade_events`, `ingest_events`, `book_checkpoints`,
`replay_validations`, `execution_events`.

---

## 6. Quick-Reference: Validating a Change

Use the smallest command that covers your change:

| What changed | Command |
|-------------|---------|
| Types / fixed-point | `cargo test -p pb-types` |
| Book logic | `cargo test -p pb-book` |
| Feed / dispatcher | `cargo test -p pb-feed` |
| API routes / live state | `cargo test -p pb-api` |
| Parquet storage or replay | `cargo test -p pb-integration-tests --test parquet_roundtrip --test replay_engine` |
| Schema / Arrow conversion | `cargo test -p pb-integration-tests --test schema_conversion` |
| Dispatcher pipeline | `cargo test -p pb-integration-tests --test dispatcher_pipeline` |
| Book determinism | `cargo test -p pb-integration-tests --test book_determinism` |
| Everything (no Docker) | `cargo test --workspace --exclude pb-integration-tests` + integration tests above |
| Full CI mirror | `just ci` or the three commands from §1 |

After running tests, always check:

```bash
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

---

## 7. Common Pitfalls

- **Never use `f64` for prices or sizes.** The codebase uses `FixedPrice(u32)`
  scaled ×10 000 and `FixedSize(u64)` scaled ×1 000 000.
- **No `Arc<Mutex<_>>` on hot paths.** Channel-based message passing only.
- **`thiserror` in library crates, `anyhow` only in `pb-bin`.**
- **`tracing` for logging** — never `println!` or the `log` crate.
- **Integration test exclusion in AGENTS.md** — the default test command
  (`cargo test --workspace --exclude pb-integration-tests`) does *not* run
  any integration tests. Run them explicitly per §3g when your change touches
  storage, replay, or schema code.

---

## 8. Updating This Skill

When you discover a new testing trick, environment workaround, or operational
pattern while working on poly-book:

1. Open this file (`.cursor/skills/cloud-agent-starter.md`).
2. Add the new knowledge to the relevant section, or create a new section if
   it doesn't fit.
3. Keep entries concrete and command-oriented — future agents need copy-paste
   commands, not prose.
4. Commit the update alongside your code change so the skill stays in sync
   with the repo.

Examples of things worth adding:

- A new CLI subcommand and how to test it.
- A workaround for a Cloud Agent environment limitation (e.g., missing system
  library, Docker unavailability).
- A new integration test file and what it covers.
- A new config key that affects local development.
- A manual testing workflow for a new API route.
