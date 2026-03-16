# Operations Guide

This document collects configuration, deployment, and infrastructure details that
are useful for operators but too heavy for the main README.

## Configuration

Runtime config is layered in this order:

1. `config/default.toml`
2. Environment variables with the `PB__` prefix
3. CLI flags

Current defaults:

```toml
[feed]
ws_url = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
rest_url = "https://clob.polymarket.com"
gamma_url = "https://gamma-api.polymarket.com"
ping_interval_secs = 10
reconnect_base_delay_ms = 100
reconnect_max_delay_ms = 30000
rate_limit_requests = 1500
rate_limit_window_secs = 10

[storage]
parquet_base_path = "./data"
parquet_flush_interval_secs = 300
parquet_row_group_size = 65536
checkpoints_enabled = true
checkpoint_interval_secs = 60
clickhouse_url = "http://localhost:8123"
clickhouse_database = "poly_book"
clickhouse_batch_interval_secs = 1
clickhouse_batch_size = 10000

[metrics]
listen_addr = "0.0.0.0:9090"
endpoint = "/metrics"

[api]
listen_addr = "0.0.0.0:3000"
default_depth = 20
max_depth = 200
stale_after_secs = 15
historical_backend = "parquet"  # or "clickhouse"
query_workbench_enabled = false
query_max_rows = 10000
query_timeout_secs = 30

[wal]
base_path = "./data/wal"
segment_size_mb = 64
max_segments = 16
max_consumer_lag_bytes = 268435456  # 256 MB
position_commit_interval_ms = 1000

[grpc]
enabled = false
listen_addr = "0.0.0.0:50051"

[logging]
level = "info"
format = "pretty"
```

Example overrides:

```bash
PB__STORAGE__PARQUET_BASE_PATH=/tmp/poly-book-data \
PB__LOGGING__LEVEL=debug \
cargo run -- auto-ingest
```

On feed reconnect success, the dispatcher clears per-asset sequence and stale
snapshot tracking before emitting `source_reset`, so downstream replay can treat
the new WebSocket session as a hard continuity boundary.

Serve the workstation API with explicit port overrides:

```bash
PB__API__LISTEN_ADDR=127.0.0.1:3000 \
PB__METRICS__LISTEN_ADDR=127.0.0.1:9090 \
cargo run -- serve-api --tokens <TOKEN_ID>
```

## Data Layout

Parquet data is partitioned by dataset and time:

```text
data/<dataset>/<year>/<month>/<day>/<hour>/*.parquet
```

Primary datasets:

- `book_events`
- `trade_events`
- `ingest_events`
- `book_checkpoints`
- `replay_validations`
- `execution_events`

### WAL Layout

WAL segments are stored under `data/wal/` (configurable via `wal.base_path`):

```text
data/wal/
├── segment_000000000000.wal
├── segment_000000000001.wal
├── consumer_serve-live.pos    # reader position file
└── ...
```

Each segment is a BufWriter-wrapped append-only file with length-prefix + CRC32C
framing. Records use a version-byte prefix for forward-compatible deserialization
(`pb_wal::codec`).

The separated `serve` runtime keeps its live consumer position in
`consumer_serve-live.pos`. That position is committed periodically during WAL
tailing and durably written with temp-file + fsync + rename semantics.
`wal.position_commit_interval_ms` controls the steady-state commit cadence for
that reader.

## CI

GitHub Actions runs the following checks on pushes and pull requests to `main`:

- `cargo check --all-targets` (requires `protobuf-compiler`)
- `cargo test --workspace --exclude pb-integration-tests` (requires `protobuf-compiler`)
- `cargo clippy --all-targets -- -D warnings` (requires `protobuf-compiler`)
- `cargo fmt --all -- --check`
- `cargo-audit` — dependency vulnerability scanning via `rustsec/audit-check`
- Web CI — `eslint`, `tsc -b`, `vitest run`, `vite build` in `web/`
- Fuzz smoke test — `fuzz_wal_corruption` and `fuzz_book_delta` (30s each, nightly)
- `cargo +nightly miri test` — undefined behavior detection for pb-types and pb-book

Additional local fuzz target:

- `cargo +nightly fuzz run fuzz_query_guard` — SQL sanitizer and normalization path for the query workbench

Supply-chain checks (`cargo-deny` for advisories, bans, and licenses) run on a
separate weekly schedule and on pushes/PRs.

## Deployment

Merges to `main` trigger the deploy workflow after CI passes.

Deployment flow:

1. Build the Docker image (multi-stage: Node for SPA, Rust for binary)
2. Push the image to Amazon ECR
3. Register a new ECS task definition
4. Update the ECS service
5. Wait for service stability

The workflow uses GitHub OIDC and an AWS IAM role stored in the
`AWS_DEPLOY_ROLE_ARN` repository secret.

## Deployment Packaging

The `Dockerfile` uses a multi-stage build:

1. **web-builder** — runs `npm ci && npx vite build` in `web/` to produce the
   static SPA assets in `dist/`.
2. **builder** — runs `cargo build --release --bin poly-book` to produce the
   Rust binary.
3. **runtime** — copies the binary, the SPA assets (to `/var/lib/poly-book/web`),
   and the default config into a minimal Debian image.

The recommended initial packaging bundles both the API binary and static SPA
assets into a single container. The `serve-api` command serves the API on `:3000`
and a separate static file server or reverse proxy can serve the SPA from the
bundled assets.

A later migration to separate containers (Rust API + Nginx/Caddy for static
assets) is straightforward when traffic or team structure warrants it.

## Infrastructure

Terraform in `infra/` provisions the AWS resources used by the current
deployment target:

- ECR for image storage
- ECS Fargate Spot for compute
- S3 for Parquet storage
- VPC, subnets, IAM, and CloudWatch resources

Bootstrap:

```bash
cd infra
cp terraform.tfvars.example terraform.tfvars
terraform init
terraform apply
```

Then:

1. Set `github_org` in `terraform.tfvars`
2. Copy the `github_actions_role_arn` output into the GitHub secret
   `AWS_DEPLOY_ROLE_ARN`

## Cost Control

Set `desired_count = 0` in `infra/terraform.tfvars` and re-apply Terraform to
stop running tasks while preserving the deployed resources.

## Local Inspection

Useful helper commands:

```bash
just parquet-ls
just parquet-count
just parquet-peek
just parquet-schema
just parquet-stats
```

## Workstation API

### Combined Mode (single process)

```bash
# Serve fixed token IDs (feed + API in one process)
cargo run -- serve-api --tokens <TOKEN_ID>

# Follow the rotating BTC 5-minute market
cargo run -- serve-api --auto-rotate
```

### Separated Mode (two processes)

```bash
# Terminal 1 — ingest process (feed + WAL + storage sinks)
cargo run -- ingest --tokens <TOKEN_ID>

# Terminal 2 — serve process (checkpoint hydration + WAL tail + API)
cargo run -- serve --tokens <TOKEN_ID>
```

The `serve` process hydrates from the latest `BookCheckpoint`, replays WAL
records from that offset, then live-tails the WAL. It can be killed and
restarted without data loss.

### Historical Backend Selection

Set `api.historical_backend` to choose the query backend for replay, integrity,
and execution routes:

```bash
PB__API__HISTORICAL_BACKEND=clickhouse cargo run -- serve-api --auto-rotate
```

If ClickHouse is configured but unreachable at startup, the system falls back to
Parquet with a warning.

### gRPC Surface

Enable the gRPC read surface for programmatic access to historical queries:

```bash
PB__GRPC__ENABLED=true cargo run -- serve --tokens <TOKEN_ID>
```

The gRPC server listens on `0.0.0.0:50051` by default and exposes `Reconstruct`,
`IntegritySummary`, and `ExecutionTimeline` RPCs via the `WorkstationService`.

### Query Workbench

Enable the query workbench for ad-hoc read-only SQL against ClickHouse:

```bash
PB__API__QUERY_WORKBENCH_ENABLED=true \
PB__API__HISTORICAL_BACKEND=clickhouse \
cargo run -- serve-api --auto-rotate
```

```bash
# List available datasets
curl http://localhost:3000/api/v1/query/datasets

# Execute a query
curl -X POST http://localhost:3000/api/v1/query/sql \
  -H 'Content-Type: application/json' \
  -d '{"sql": "SELECT count() FROM book_events", "max_rows": 100}'
```

The workbench rejects write SQL and injects LIMIT if not present. Returns 503
when disabled (the default).

### Health Endpoint

The `serve` process exposes `GET /health` for liveness and readiness checks:

```bash
curl http://localhost:3000/health
# {"ready":true,"hydrated":true,"wal_lag_bytes":0,"needs_resync":false}
```

Use `needs_resync` to detect when a reader has fallen behind pruned WAL segments
and requires a fresh checkpoint hydration.

### Port Defaults

| Service | Port  |
|---------|-------|
| API     | 3000  |
| Metrics | 9090  |
| gRPC    | 50051 |

### Current Scope

- read-only HTTP, WebSocket, and gRPC API
- live feed status and active asset visibility
- live in-memory order book snapshots and per-asset WS streaming
- configurable Parquet or ClickHouse historical backend
- replay reconstruction, integrity summaries, execution timeline inspection
- WAL gap detection, lag tracking, and backpressure-aware pruning
- health endpoint with hydration and WAL status
- query workbench for ad-hoc read-only SQL (ClickHouse backend, opt-in)

### Not Yet Provided

- latency summary endpoints

The existing Docker and ECS deployment remains ingestion-oriented today. The
workstation API is not yet part of that production deployment flow.

## Workstation Web App

The SPA currently ships:

- `Live Feed`
- `Replay Lab`
- `Integrity`
- `Execution Timeline`

### Running API and Web App Together

```bash
# terminal 1 — start the Rust API
cargo run -- serve-api --auto-rotate

# terminal 2 — start the web dev server
cd web
npm install
npm run dev
```

Open `http://127.0.0.1:4173` in the browser. The Vite dev server proxies
`/api` requests to the Rust API at `127.0.0.1:3000`.

### Demo Mode (No Backend Required)

```bash
cd web
npm install
npm run dev
# open http://127.0.0.1:4173/?source=demo
```

The SPA ships seeded fixtures for all routes. Use the in-app source toggle or
the `?source=demo` query parameter.

### Port Defaults

| Service | Port |
|---------|------|
| API     | 3000 |
| Metrics | 9090 |
| Web     | 4173 |

### Overriding the Dev Proxy Target

```bash
cd web
VITE_DEV_API_PROXY_TARGET=http://127.0.0.1:3100 npm run dev
```

Or bypass the proxy entirely and fetch from an explicit origin:

```bash
cd web
VITE_API_BASE_URL=http://127.0.0.1:3000 npm run dev
```

### Web Transport Behavior

- `Live Feed` uses WebSocket order book streaming when the backend supports it,
  with automatic fallback to adaptive HTTP polling.
- Feed status and active assets use adaptive HTTP polling (1s foreground, 5s
  background).
- Stale in-flight browser requests are aborted before the next poll.

### Deferred from the Current SPA Pass

- Latency (reserved for metrics-backed summaries)
- Query Workbench SPA view (backend routes are implemented and opt-in)
