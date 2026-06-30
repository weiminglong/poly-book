# Operations Guide

This document collects configuration, deployment, and infrastructure details that
are useful for operators but too heavy for the main README.

## Configuration

Runtime config is layered in this order:

1. `config/default.toml`
2. Environment variables with the `PB__` prefix
3. CLI flags

At startup the process validates the loaded config against the set of recognized
keys and logs a `WARN` for any unknown `section.key` (a typo or a removed/renamed
setting), so a misspelled key — which is otherwise silently ignored and falls
back to the default — is visible in the logs rather than silently dropped.

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

# parquet_base_path accepts a local path OR a URL scheme (s3://bucket/prefix,
# gs://..., file://...); an s3:// path is wired to a real S3 object store.
# Cloud backends are configured from the process environment (parsed via
# object_store::parse_url_opts(url, env::vars())): for s3://, set AWS_REGION and
# either static AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY or rely on the default AWS
# provider chain (ECS task role / instance profile). For an S3-compatible endpoint
# (MinIO/LocalStack) also set AWS_ENDPOINT, AWS_ALLOW_HTTP=true, and
# AWS_VIRTUAL_HOSTED_STYLE_REQUEST=false (path-style). End-to-end persistence +
# restart is covered by tests/integration/s3_minio_roundtrip.rs.
[storage]
parquet_base_path = "./data"
parquet_flush_interval_secs = 300
parquet_row_group_size = 65536  # reserved; not yet wired (Parquet uses a fixed row-group size)
checkpoints_enabled = true
checkpoint_interval_secs = 60
clickhouse_url = "http://localhost:8123"
clickhouse_database = "poly_book"
clickhouse_batch_interval_secs = 1   # honored by the ClickHouse sink
clickhouse_batch_size = 10000        # honored by the ClickHouse sink

# Services default to loopback. Non-loopback API/gRPC binds require
# api.auth_token / PB__API__AUTH_TOKEN at startup.
[metrics]
listen_addr = "127.0.0.1:9090"
endpoint = "/metrics"

[api]
listen_addr = "127.0.0.1:3000"
default_depth = 20
max_depth = 200
stale_after_secs = 15
historical_backend = "parquet"  # or "clickhouse"
query_workbench_enabled = false
query_max_rows = 10000
query_timeout_secs = 30
http_request_timeout_secs = 30   # per-request HTTP timeout (504 if exceeded); not the WS stream
# auth_token = ""                # required for non-loopback API/gRPC binds; all API/WS/gRPC data routes require it (health stays open)

[wal]
base_path = "./data/wal"
segment_size_mb = 64
max_segments = 16             # hard cap on retained segments (incl. active); pruning enforces both this and the lag-byte budget
max_consumer_lag_bytes = 268435456  # 256 MB — pruning retention window (lag-byte budget)
position_commit_interval_ms = 1000
flush_interval_ms = 20        # BufWriter flush cadence (tail-reader visibility)
sync_interval_ms = 200        # fdatasync cadence (bounds OS-crash loss window)

[grpc]
enabled = false
# Loopback by default; non-loopback requires api.auth_token.
listen_addr = "127.0.0.1:50051"

[logging]
level = "info"
format = "pretty"   # "json" emits structured JSON logs
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

Feed input is bounded before it reaches storage: WebSocket text frames are capped
at 1 MiB, venue snapshots at 10,000 levels per side, `price_change` batches at
20,000 entries, REST bodies at 4 MiB before full buffering, discovery pages at
1,000 events, and CLOB metadata at 512 tokens.

### Polymarket CLOB V2

The ingest pipeline targets Polymarket CLOB V2 (live as of 2026-04-28). The V2
cutover did not change the WebSocket URL, the subscribe payload
(`{"assets_ids": [...], "type": "market"}`), or the `book` / `price_change` /
`last_trade_price` event shapes. `last_trade_price.fee_rate_bps` continues to
reflect the actual fee charged at match time (now protocol-set per market).

V2 additions handled by the dispatcher and REST client:

- `tick_size_change` events parse without erroring and increment the
  `pb_messages_received_total{event_type="tick_size_change"}` counter. They
  are informational; the book engine does not enforce a minimum tick.
- `GET /book` snapshots may include `tick_size`, `min_order_size`,
  `neg_risk`, and `last_trade_price`. They are optional fields on
  `RestBookResponse`.
- V2's per-market metadata is reachable via
  `RestClient::get_clob_market_info(condition_id)` → `GET
  /clob-markets/{condition_id}`.

Premium V2 events (`best_bid_ask`, `new_market`, `market_resolved`) require
`custom_feature_enabled: true` on subscribe and are not enabled by default.

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

### Schema Versioning & Migration

Persisted data carries two independent, explicitly-versioned formats. Each has a
single source-of-truth constant and a reader that **fails closed** on an
unrecognized version rather than silently misreading bytes:

| Format | Version constant | Reader gate |
|---|---|---|
| WAL frame codec | `pb_wal::codec::CURRENT_VERSION` (currently `2`) | `decode` rejects any other version byte; pre-v2 frames return an error with a "drain and re-create the WAL" hint |
| Parquet / ClickHouse columns | `pb_store::schema::PB_SCHEMA_VERSION` (currently `"2"`, written as the `pb_schema_version` Parquet key-value metadata on every file) | `pb-replay`'s Parquet reader rejects files whose `pb_schema_version` differs from the expected version |

Frozen golden fixtures (`golden_codec_book_v2_bytes_are_stable` in `pb-wal`, and
the determinism fixture in `tests/integration/book_determinism.rs`) make any
accidental byte-format change a test failure, so a version bump is always a
deliberate act.

**Migration procedure when a persisted format changes:**

1. **Decide whether it is backward-compatible.** Adding an *optional* field that
   old readers can ignore and new readers default usually does **not** require a
   version bump. A field whose absence changes meaning, a re-typed/removed field,
   or any reordering that breaks the frozen golden bytes **does**.
2. **Bump exactly one constant** — `CURRENT_VERSION` for WAL frame changes,
   `PB_SCHEMA_VERSION` for Parquet/ClickHouse column changes — in the same commit
   as the format change.
3. **Add a cross-version fixture** capturing the *previous* version's bytes and a
   test asserting the new reader either reads them or rejects them with a clear,
   actionable error. Update the golden fixture for the new version.
4. **Drain before deploy (WAL).** The WAL is a short-lived tail, not a long-term
   store: stop `ingest`, let `serve` consume to the end, then let the new build
   create fresh segments. Old segments are not auto-upgraded — a version mismatch
   is surfaced, not papered over.
5. **Re-snapshot / backfill (Parquet/ClickHouse).** Historical Parquet files keep
   their original `pb_schema_version`. Either (a) keep readers able to accept the
   prior version for the retention window, or (b) re-write affected partitions
   from the source-of-truth (`backfill` / `reconcile`) so all files carry the new
   version. ClickHouse column changes go through a normal `ALTER TABLE` migration
   before the new writer starts.
6. **Verify with replay.** Run the golden replay regression and a
   `replay validate` over a migrated window before promoting the deploy.

## CI

GitHub Actions runs the following checks on pushes and pull requests to `main`:

- `cargo check --all-targets` (requires `protobuf-compiler`)
- `cargo test --workspace --exclude pb-integration-tests` (requires `protobuf-compiler`)
- `cargo clippy --all-targets -- -D warnings` (requires `protobuf-compiler`)
- `cargo fmt --all -- --check`
- `cargo-audit` — dependency vulnerability scanning via `rustsec/audit-check`
- `tfsec` + `tflint` (AWS ruleset) + `terraform fmt`/`validate` — Terraform
  static security scan, provider-aware lint, and validation of `infra/`
  (`supply-chain` workflow, `iac-scan` job)
- `promtool check rules` + `promtool test rules` — Prometheus alert-rule
  validation and offline incident unit tests; `amtool check-config` + routing
  assertions — Alertmanager routing validation (`monitoring` job)
- `cargo bench --workspace --no-run` — compiles every Criterion benchmark so the
  latency harness can't rot (`bench` job; statistical regression gating is
  local-only, since shared runners are too noisy)
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

Terraform in `infra/` provisions the AWS resources for the deployment target:

- ECR for image storage
- ECS Fargate compute — the ingest service keeps an **on-demand base**
  (`ingest_on_demand_base`) with Spot only for overflow, so a Spot reclaim never
  drops all capture
- a read-only **`serve`** API service (`serve.tf`) on the shared WAL
- **EFS** (`efs.tf`) for the durable write-ahead log, mounted into both ingest
  and serve (survives task restarts / host loss; pairs with the `--standby`
  writer failover)
- optional **single-node ClickHouse** on ECS+EFS (`clickhouse.tf`, opt-in via
  `enable_clickhouse_service`; prefer managed ClickHouse for production), with
  Cloud Map private DNS for `serve` discovery and password authentication
  injected through ECS secrets
- S3 for Parquet storage (SSE-KMS CMK, versioning, lifecycle, access logging to a
  hardened SSE-S3 log bucket)
- VPC (with Flow Logs), subnets, security groups (metrics port in-VPC only; EFS
  egress scoped to the VPC), IAM (incl. scoped EFS mount perms), ECR (immutable
  tags, scan-on-push), and CloudWatch resources

Runtime secrets:

- `serve_api_auth_token_secret_arn` — required when `serve_desired_count > 0`
  because the ECS serve task binds `PB__API__LISTEN_ADDR=0.0.0.0:3000`.
- `clickhouse_password_secret_arn` — required when `enable_clickhouse_service`
  is true; injected as `CLICKHOUSE_PASSWORD` for the ClickHouse container.
- `clickhouse_app_url_secret_arn` — required when `enable_clickhouse_service`
  is true; injected as `PB__STORAGE__CLICKHOUSE_URL` for app/serve tasks. The
  secret value should include credentials, for example
  `http://poly_book:<password>@clickhouse.poly-book.internal:8123`.

IAM is split by runtime role: the live ingest task keeps S3 read/write/list
without `s3:DeleteObject`, offline `reconcile` has a separate maintenance task
role with delete permission for Parquet recovery, serve uses a read-only S3 role
plus EFS consumer-position writes, and the optional ClickHouse task has a
separate no-S3 task role.

> **Status:** the `serve`/EFS/ClickHouse/on-demand topology
> passes `terraform validate` + `fmt` **and a `tfsec` static security scan** (CI
> `iac-scan` job; intentional trade-offs are documented inline with
> `#tfsec:ignore:<AVD-ID>` + rationale) but has **not** been `terraform apply`ed
> against a live account. Applying it, plus a Spot-reclaim and a restart/failover
> drill, is the remaining verification.

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

### Storage Recovery (`reconcile`)

The Parquet sink buffers up to `storage.parquet_flush_interval_secs` (default
300s) in memory. A crash, OOM, or SIGKILL drops that buffered window from
Parquet — but the WAL captured the same records durably. To rebuild the lost
storage window from the WAL:

```bash
# Stop the ingest process first (reconcile must run offline — it replaces whole
# (dataset, asset, hour) partitions and would race a live sink).
cargo run -- reconcile
```

`reconcile` reads the retained WAL and, for every `(dataset, asset, hour)`
partition it covers, deletes the existing Parquet files and rewrites the complete
partition from the WAL. It is idempotent (safe to re-run) and authoritative for
any partition it touches. It does not commit a consumer position, so each run
reconciles the full retained WAL. ClickHouse is not rebuilt by this command.
In ECS, run this as a maintenance task using the dedicated reconcile task role;
the always-on ingest task role intentionally lacks `s3:DeleteObject`.

### Failover & Recovery (RTO/RPO)

The current deployment is **single-feed, single-writer** by design: exactly one
`ingest` process owns the WAL at a time, enforced by an advisory `flock` on
`<wal.base_path>/.wal.lock`. This is the correctness foundation for failover — a
standby can never interleave appends into the shared WAL while the primary is
alive (`WalWriter::open` fails fast with `WriterLocked`), and the lock is released
crash-safely on process exit, so a standby can take over without a stale lock to
clear. The takeover semantics (standby reads everything the primary durably
synced, then continues appending — no data loss across the handoff) are covered by
the `standby_writer_takes_over_shared_wal_after_primary_exit` unit test.

Recovery objectives by failure mode:

| Failure | Recovery action | RPO (data loss) | RTO (time to serve) |
|---|---|---|---|
| `serve` (API) process crash | Restart; re-hydrate from latest checkpoint + WAL tail | 0 (read-only; no data originates here) | seconds — bounded by checkpoint hydration + WAL replay |
| `ingest` process crash, same host | Restart `ingest`; flock is already released; it resumes appending to the last segment | ≤ `wal.sync_interval_ms` of un-`fdatasync`'d records (default 200 ms) on OS-crash/power-loss; **0** on a clean process kill | seconds — process start + lock acquire |
| Parquet sink buffer lost (OOM/SIGKILL) | `reconcile` rebuilds affected partitions from the durable WAL (offline) | 0 for any window the WAL still retains | minutes — offline rebuild, scales with window |
| Host loss (WAL on durable/EFS volume) | Run a hot standby `ingest --standby` against the shared WAL volume; it waits on the lock and **auto-promotes** the moment the primary's lock releases | ≤ last synced records, as above | seconds-to-minutes — standby poll interval + feed connect, once the standby is already running |
| Host loss (WAL on ephemeral storage) | Parquet on S3 is durable; the in-flight WAL tail is lost | the un-flushed Parquet window not yet mirrored to the WAL volume | minutes |

**Automatic writer promotion** is available: start the standby as
`ingest --standby` and it polls the shared WAL lock, promoting itself to the
active writer the instant the primary releases it (no manual intervention; the
takeover preserves the primary's durably-synced records). The promotion *logic* is
unit-tested in-process (`pipeline::open_wal_writer_with_standby` tests +
`pb_wal`'s takeover test).

**Measuring the durability-layer RTO locally:** `just failover-drill` measures the
code-controlled component of the failover RTO on the current hardware — the
wall-clock from "primary process gone" through the standby acquiring the WAL
lease, recovering the tail, resuming durable appends, and re-reading the full
history (it prints a per-phase breakdown and asserts no records were lost across
the handoff). On a dev laptop this is tens of milliseconds for a 50k-record
backlog; run it on the target instance type for a real baseline.

**Still deferred** (needs a real multi-replica deployment to author and verify):
a redundant second *feed* with arbitration
(the standby connects to the feed only after it promotes, so there is a capture
gap equal to the promotion latency), and the *full* wall-clock RTO from a live
failover drill — which adds container scheduling + health-check time on top of the
locally-measured durability-layer handoff above. The single-writer flock +
`--standby` auto-promotion + durable storage + `reconcile` is the supported
recovery path.

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

The gRPC server listens on `127.0.0.1:50051` by default and exposes
`Reconstruct`, `IntegritySummary`, and `ExecutionTimeline` RPCs via the
`WorkstationService`. If `api.auth_token` is configured, callers must send
`Authorization: Bearer <token>` metadata. Startup rejects non-loopback gRPC
binds without that token. Expensive gRPC backend work is bounded by a 30s
deadline and a 128-request global in-flight cap; saturated servers return
`RESOURCE_EXHAUSTED`, and timed-out work returns `DEADLINE_EXCEEDED`.

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

The workbench rejects write SQL, dangerous ClickHouse table functions (including
quoted identifiers), and table references outside the advertised dataset
allowlist. It injects `LIMIT` if not present. ClickHouse transport and decode
errors strip request URL context before they become internal API errors, so
credential-bearing backend URLs do not enter API logs. Returns 503 when disabled
(the default).

### Health Endpoints

The `serve` process exposes:

- `GET /health` — detailed JSON (always HTTP 200), for humans/dashboards.
- `GET /health/live` — liveness; 200 whenever the process is up.
- `GET /health/ready` — readiness; 200 only when hydrated and not awaiting
  resync, otherwise 503. Point load-balancer / orchestrator probes here.

```bash
curl http://localhost:3000/health
# {"ready":true,"hydrated":true,"wal_lag_bytes":0,"needs_resync":false}
```

### Alerting & Runbook

Prometheus metrics are served at `/metrics` (default `:9090`). Committed alerting
config lives in [`monitoring/`](../monitoring/):

- `monitoring/alerts.yml` — Prometheus alert rules (WAL append failure, sink
  flush failure, feed silent/stale, book mismatch, crossed book, sequence gaps,
  unknown messages, WAL consumer lag). Load via `rule_files:`.
- `monitoring/alertmanager.yml` — Alertmanager **routing**: `critical` →
  `pagerduty-critical` (pages, re-pages hourly), `warning` → `slack-warning`,
  `info` → `slack-info`, with a critical-inhibits-lower rule. Secret-free — the
  PagerDuty routing key and Slack webhook are read at runtime from files mounted
  at `/etc/alertmanager/secrets/` (`pagerduty_routing_key`, `slack_webhook_url`),
  so the config is safe to commit and validate in CI.
- `monitoring/RUNBOOK.md` — one on-call action section per alert, plus the alert
  routing table.
- `monitoring/grafana-dashboard.json` — importable dashboard (message rate, feed
  staleness, WAL lag, recv→durable p50/p99, durability/storage failures,
  data-quality events, snapshots/deltas).
- `monitoring/alerts_test.yml` — `promtool` rule unit tests that simulate
  incidents offline (e.g. WAL append failing, silent/stale feed, crossed book,
  WAL lag) and assert the matching alert fires. The CI `monitoring` job runs
  `promtool check rules` + `promtool test rules` AND `amtool check-config
  monitoring/alertmanager.yml` + severity→receiver routing assertions, so both
  rule rot and routing drift are caught without a live Prometheus/Alertmanager.
  (Only the live PagerDuty/Slack integration secrets and a running Alertmanager
  remain environment-specific — the routing logic itself is now verified offline.)

### Time discipline (NTP/PTP)

Replay ordering does **not** depend on host clock accuracy: events carry a
process-monotonic `ingest_ordinal` stamped at ingest, and replay sorts by it, so
the reconstructed book is deterministic regardless of wall-clock skew. The clock
*does* matter for two things:

- **Exchange-time replay** (`mode=exchange_time`) and the recv→durable latency
  metric compare host time against venue timestamps. Run **NTP (chrony/ntpd)** on
  every ingest host; keep offset within **±100 ms** for meaningful latency
  figures (PTP if you need tighter). The `ClockSkew` alert fires when venue
  timestamps run >2 s ahead of receive time — resync NTP (see RUNBOOK).
- **Partition placement**: a wildly-wrong clock would file events into the wrong
  hour partition; out-of-range timestamps are quarantined to `invalid_timestamp`
  rather than silently misfiled.

Use `needs_resync` to detect when a reader has fallen behind pruned WAL segments
and requires a fresh checkpoint hydration. WAL segments all consumers have
advanced past are pruned automatically by the ingest/auto-ingest process (lag-
byte retention window), so disk usage stays bounded.

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

Open `http://127.0.0.1:4173` in the browser. The Vite dev server binds loopback
by default and proxies `/api` requests to the Rust API at `127.0.0.1:3000`. Set
`VITE_DEV_HOST=0.0.0.0` only for an intentional LAN-exposed dev session.

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
