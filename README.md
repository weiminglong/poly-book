# poly-book

High-performance Polymarket BTC 5-minute orderbook ingestion, storage, and replay system built in Rust.

Designed to demonstrate low-latency data engineering patterns relevant to quantitative trading infrastructure: fixed-point arithmetic, zero-copy deserialization, lock-free channel-based architecture, and tiered storage.

## Architecture

```
Polymarket WS ──> pb-feed (deser + timestamp) ──> pb-book (L2Book) ──> pb-store
                                                       │                  ├─> Parquet (5-min flush)
                                                       │                  └─> ClickHouse (1s batch)
                                                       v
                                                  pb-metrics (Prometheus)
```

- Single-threaded book updates — no locks on hot path
- Bounded `tokio::mpsc` channels with backpressure between components
- Message-passing only, no `Arc<Mutex<_>>`

## Project Structure

```
poly-book/
├── Cargo.toml                     # Workspace root
├── config/default.toml            # Runtime configuration
├── openspec/                      # Spec-driven development artifacts
│   └── changes/archive/          # Completed change specs (proposal, design, tasks)
├── crates/
│   ├── pb-types/                  # Fixed-point decimals, wire formats, event types
│   ├── pb-book/                   # In-memory L2 orderbook engine (BTreeMap-based)
│   ├── pb-feed/                   # WebSocket + REST ingest, dispatcher, rate limiter
│   ├── pb-store/                  # Parquet sink + ClickHouse sink
│   ├── pb-replay/                 # Historical replay engine + backfill
│   ├── pb-metrics/                # Prometheus metrics + HTTP endpoint
│   └── pb-bin/                    # CLI binary (discover, ingest, replay, backfill)
```

## Key Design Decisions

### Fixed-Point Price (`FixedPrice(u32)` scaled by 10,000)
Polymarket prices are 0.00–1.00. A `u32` scaled by 10,000 gives 4-decimal precision in 4 bytes with `Copy` and trivial `Ord` — vs `Decimal` at 16 bytes with complex comparisons. Quantities use `FixedSize(u64)` scaled by 1,000,000.

### L2Book with `BTreeMap<Reverse<FixedPrice>, FixedSize>` for bids
Sorted iteration yields best-to-worst without re-sorting. Cache-friendly for small books (<50 levels typical on Polymarket). Asks use `BTreeMap<FixedPrice, FixedSize>` for lowest-first ordering.

### Zero-Copy Wire Deserialization
`serde(borrow)` with `&'a str` on WebSocket message types borrows directly from the raw WS buffer — no heap allocation for string fields during deserialization.

### Tiered Storage
- **Hot**: In-memory `L2Book` for live state
- **Cold**: Parquet files with Zstd compression, 64K row groups, time-partitioned at `data/{year}/{month}/{day}/{hour}/`
- **Warm**: ClickHouse with `ReplacingMergeTree`, 90-day TTL, 1-second batch inserts

### Cloud-Ready
The `object_store` crate abstracts Parquet writes — switch from local filesystem to S3/GCS/Azure with a config change. ClickHouse URL is configurable via environment variable.

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) (1.75+)
- [just](https://github.com/casey/just) (optional, task runner — `brew install just`)
- [DuckDB](https://duckdb.org/) (optional, for Parquet inspection — `brew install duckdb`)
- ClickHouse (optional, for warm storage)

### Build and Test

```bash
just check       # type-check all crates
just test        # run all tests
just bench       # run Criterion benchmarks
just clippy      # lint with warnings as errors
just ci          # fmt-check + clippy + test (mirrors CI)
```

Or with cargo directly:

```bash
cargo build
cargo test
cargo bench
```

Run `just --list` to see all available recipes.

### Usage

#### Discover active BTC 5-minute markets

```bash
just discover
# or: cargo run -- discover --filter btc
```

#### Auto-discover and ingest (recommended)

```bash
just auto-ingest
# or: cargo run -- auto-ingest
```

Continuously discovers live BTC 5-minute markets and rotates ingestion automatically.

#### Ingest specific tokens

```bash
just ingest <TOKEN_ID>
# or: cargo run -- ingest --tokens <TOKEN_ID> --parquet --metrics
```

#### Replay book state at a historical timestamp

```bash
just replay <TOKEN_ID> <TIMESTAMP_US>
# or: cargo run -- replay --token <TOKEN_ID> --at <TIMESTAMP_US> --source parquet
```

#### Backfill historical snapshots via REST

```bash
just backfill <TOKEN_ID>
# or: cargo run -- backfill --tokens <TOKEN_ID> --interval-secs 60
```

#### Inspect ingested Parquet data (requires DuckDB)

```bash
just parquet-stats    # row count, timestamp range, event type breakdown
just parquet-peek     # first 20 rows
just parquet-schema   # column names and types
```

### Configuration

Runtime config is layered: `config/default.toml` -> environment variables (`PB__` prefix) -> CLI args.

```toml
[feed]
ws_url = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
ping_interval_secs = 10
rate_limit_requests = 1500

[storage]
parquet_base_path = "./data"
parquet_flush_interval_secs = 300
clickhouse_url = "http://localhost:8123"

[metrics]
listen_addr = "0.0.0.0:9090"
```

## Crate Details

| Crate | Description |
|-------|-------------|
| **pb-types** | `FixedPrice(u32)`, `FixedSize(u64)`, zero-copy wire types (`WsMessage`, `RestBookResponse`, `GammaEvent`), `OrderbookEvent`, `AssetId`, `Sequence` |
| **pb-book** | `L2Book` with `apply_snapshot`, `apply_delta`, `best_bid/ask`, `mid_price`, `spread`, sequence gap detection. 12 unit tests, Criterion benchmarks |
| **pb-feed** | `WsClient` (reconnect with exp backoff + jitter), `RestClient` (book fetch, Gamma API discovery), `Dispatcher` (deser + normalize), `RateLimiter` (governor, 1500 req/10s) |
| **pb-store** | `ParquetSink` (buffered 5-min flush, Zstd, `object_store` abstraction), `ClickHouseSink` (1s/10K-row batch inserts, `ReplacingMergeTree` DDL) |
| **pb-replay** | `ParquetReader` + `ClickHouseReader` (unified `EventReader` trait), `ReplayEngine` (reconstruct book at timestamp T), `run_backfill` (periodic REST snapshots) |
| **pb-metrics** | Prometheus counters/histograms (`messages_received`, `deltas_applied`, `gaps_detected`, latency), axum HTTP `/metrics` endpoint |
| **pb-bin** | CLI with `discover`, `ingest`, `auto-ingest`, `replay`, `backfill` subcommands. Layered config (TOML/env/CLI), structured logging via `tracing` |

## Polymarket API

| Endpoint | Purpose |
|----------|---------|
| `wss://ws-subscriptions-clob.polymarket.com/ws/market` | Real-time orderbook (no auth, ping every 10s) |
| `https://clob.polymarket.com/book?token_id=<id>` | REST book snapshot (1,500 req/10s limit) |
| `https://gamma-api.polymarket.com/events` | Market discovery |

Message types: `book` (snapshot), `price_change` (delta), `last_trade_price` (trade).

## Development Workflow

This project uses [OpenSpec](https://github.com/Fission-AI/OpenSpec) for spec-driven development. Each feature phase has a proposal, design doc, requirement specs (Given/When/Then), and implementation tasks archived in `openspec/changes/archive/`.

| Change | Scope |
|--------|-------|
| `pb-types-foundation` | Fixed-point types, wire formats, events |
| `pb-book-engine` | L2Book with BTreeMap, snapshot/delta ops |
| `live-data-feed` | WebSocket client, REST client, dispatcher |
| `storage-pipeline` | Parquet sink, ClickHouse sink, Arrow schemas |
| `replay-backfill` | Replay engine, readers, backfill CLI |
| `observability` | Prometheus metrics, CLI, layered config |

## CI/CD

### Continuous Integration

Every push and PR to `main` runs four checks in parallel:

| Check | Command | Purpose |
|-------|---------|---------|
| Check | `cargo check --all-targets` | Type-check all code |
| Test | `cargo test` | Run all 29 unit tests |
| Clippy | `cargo clippy --all-targets` | Lint for correctness and idioms |
| Format | `cargo fmt --all -- --check` | Verify rustfmt formatting |

### Continuous Deployment

Merging to `main` triggers automated deployment to AWS ECS:

```
merge to main -> CI passes -> Docker build -> push to ECR -> update ECS service
```

- **OIDC auth** — no long-lived AWS keys; GitHub Actions assumes an IAM role scoped to `main`
- **Fargate Spot** — ~$4-5/mo running, ~$0.08/mo stopped
- **Rolling deploy** — new task definition registered, service updated, waits for stability

### Infrastructure

Terraform configs in `infra/` provision all AWS resources:

```
ECR (container registry) + ECS Fargate Spot (compute) + S3 (Parquet storage)
VPC (2 public subnets, no NAT) + IAM (OIDC, task roles) + CloudWatch (logs)
```

#### Setup

```bash
cd infra
cp terraform.tfvars.example terraform.tfvars
# Edit terraform.tfvars to set github_org
terraform init && terraform apply
# Copy github_actions_role_arn output -> GitHub secret AWS_DEPLOY_ROLE_ARN
```

#### Cost Control

Set `desired_count = 0` in `terraform.tfvars` and `terraform apply` to stop all tasks (~$0.08/mo for idle resources). Set back to `1` to resume.

## Future: Market Making Extension (v2)

The single-threaded book architecture directly supports adding a market making strategy:

```
pb-feed -> pb-book (L2Book) -> pb-strategy (decision) -> pb-execution (order placement)
```

No architecture changes needed — the v1 hot path is already the correct shape for sub-microsecond book-update-to-order-decision latency.

## License

MIT
