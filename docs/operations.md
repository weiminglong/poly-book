# Operations Guide

This document collects configuration, deployment, and data layout details that
are useful for operators but too heavy for the main README.

## Configuration

Runtime config is layered in this order:

1. `config/default.toml`
2. Environment variables with the `PB__` prefix
3. CLI flags

The layered sources are deserialized once into a typed settings struct, so an
unknown or misspelled key (a typo, or a removed/renamed setting) is a startup
error that names the field, a negative number is rejected, and a zero interval
or size is clamped to its minimum with a warning.

### Preflight (`doctor`)

```bash
poly-book doctor                 # full checklist, incl. network probes
poly-book doctor --skip-network  # offline/CI variant
```

Prints a pass/warn/fail table and exits non-zero on failure, so it can gate a
deploy (`poly-book doctor && poly-book ingest`). Checks: Parquet path
writability, WAL directory state (segments, consumer positions, last-write
age), REST/Gamma reachability with latency, a full WebSocket TLS handshake
through the same connector the feed uses, and API/metrics port availability.

The authoritative, commented defaults are in [`config/default.toml`](../config/default.toml);
every key there can be overridden with `PB__<SECTION>__<KEY>`.

`storage.parquet_base_path` accepts a local path, a `file://` URL, or an
`s3://bucket/prefix`. S3 is configured from the process environment (parsed via
`object_store::parse_url_opts(url, env::vars())`): set `AWS_REGION` and either
static `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` or rely on the default AWS
provider chain. For an S3-compatible endpoint (MinIO/LocalStack) also set
`AWS_ENDPOINT`, `AWS_ALLOW_HTTP=true`, and
`AWS_VIRTUAL_HOSTED_STYLE_REQUEST=false` (path-style). End-to-end persistence
plus restart is covered by `tests/integration/s3_minio_roundtrip.rs`.

Example overrides:

```bash
PB__STORAGE__PARQUET_BASE_PATH=/tmp/poly-book-data \
PB__LOGGING__LEVEL=debug \
cargo run -- ingest --auto-rotate
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

Parquet data is partitioned by dataset and time. Offline recovery also owns two
hidden prefixes below the configured Parquet base path:

```text
data/
├── <dataset>/<year>/<month>/<day>/<hour>/*.parquet
├── _recovery_objects/<dataset>/<year>/<month>/<day>/<hour>/*.parquet
│   # temporary immutable staging objects; an interrupted run may leave one active
└── _recovery_manifests/<dataset>/<year>/<month>/<day>/<hour>/<asset>.json
    # authoritative manifest-aware view for a recovered asset/hour
```

Successful recovery promotes the active object into the normal dataset tree and
removes its hidden staging object. The manifest remains authoritative so an
interrupted cleanup cannot make superseded normal files visible to application
readers. Raw DuckDB/Polars globs do not interpret these manifests.

Primary datasets:

- `book_events`
- `trade_events`
- `ingest_events`
- `book_checkpoints`
- `replay_validations`

### WAL Layout

WAL segments are stored under `data/wal/` (configurable via `wal.base_path`):

```text
data/wal/
├── segment_00000000000000000000.wal
├── segment_00000000000000000001.wal
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
| WAL frame codec | `pb_wal::codec::CURRENT_VERSION` (currently `3`) | `decode` rejects any other version byte; v1/v2 frames return an error with a "drain and re-create the WAL" hint |
| Parquet columns | `pb_store::schema::PB_SCHEMA_VERSION` (currently `"2"`, written as the `pb_schema_version` Parquet key-value metadata on every file) | `pb-replay`'s Parquet reader rejects files whose `pb_schema_version` differs from the expected version |

Frozen golden fixtures (`golden_codec_book_v3_bytes_are_stable` in `pb-wal`, and
the determinism fixture in `tests/integration/book_determinism.rs`) make any
accidental byte-format change a test failure, so a version bump is always a
deliberate act.

**Migration procedure when a persisted format changes:**

1. **Decide whether it is backward-compatible.** Adding an *optional* field that
   old readers can ignore and new readers default usually does **not** require a
   version bump. A field whose absence changes meaning, a re-typed/removed field,
   or any reordering that breaks the frozen golden bytes **does**.
2. **Bump exactly one constant** — `CURRENT_VERSION` for WAL frame changes,
   `PB_SCHEMA_VERSION` for Parquet column changes — in the same commit
   as the format change.
3. **Add a cross-version fixture** capturing the *previous* version's bytes and a
   test asserting the new reader either reads them or rejects them with a clear,
   actionable error. Update the golden fixture for the new version.
4. **Drain before deploy (WAL).** The WAL is a short-lived tail, not a long-term
   store: stop `ingest`, let `serve` consume to the end, then let the new build
   create fresh segments. Old segments are not auto-upgraded — a version mismatch
   is surfaced, not papered over.
5. **Re-snapshot / migrate Parquet.** Historical Parquet files keep
   their original `pb_schema_version`. Either (a) keep readers able to accept the
   prior version for the retention window, or (b) use a dedicated migration or
   re-export from the durable source of truth for every affected dataset.
   `backfill` only creates book checkpoints, while `reconcile` only republishes
   complete retained-WAL hours for book, trade, and ingest data; neither is a
   general migration path for all five datasets or arbitrary history.
6. **Verify with replay.** Run the golden replay regression and a
   `replay validate` over a migrated window before promoting the deploy.

## CI

GitHub Actions runs the following checks on pushes and pull requests to `main`:

- `cargo check --all-targets`
- `cargo test --workspace --exclude pb-integration-tests`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `promtool check rules` + `promtool test rules` — Prometheus alert-rule
  validation and offline incident unit tests (`monitoring` job)
- `docker build` of the production image (`docker-build` job)
- Web CI — `biome check`, `tsc -b`, `vitest run`, `vite build` in `web/`, then
  Playwright end-to-end tests against the built bundle

Outside the per-PR gate: fuzz smoke tests (all five targets at 30s each on
nightly Rust) run nightly and on pushes to `main` (`fuzz.yml`), and
`cargo-deny` (advisories, bans, licenses, sources) runs on PRs and weekly
(`supply-chain.yml`).

## Deployment

There is no cloud deployment in this repository. The published Docker image
(`release.yml` pushes it to GHCR on tags) plus `docker-compose.yml` are the
supported ways to run the system off a developer machine; any orchestrator that
can run that image and mount a volume for `/data` works. Set
`PB__API__AUTH_TOKEN` before binding the API off loopback.

## Deployment Packaging

The `Dockerfile` uses a multi-stage build:

1. **web-builder** — runs `npm ci && npx vite build` in `web/` to produce the
   static SPA assets in `dist/`.
2. **builder** — runs `cargo build --release --bin poly-book` to produce the
   Rust binary.
3. **runtime** — copies the binary, the SPA assets (to `/var/lib/poly-book/web`),
   and the default config into a minimal Debian image.

The image bundles the API binary and the built SPA into a single container.
`PB__API__STATIC_ASSETS_DIR` points at the bundled assets, so the API process
itself serves the UI on `:3000` — no separate static file server is needed.
The image's default command is the combined live workstation
(`serve-api --auto-rotate`); orchestrators override it per process
(`ingest`, `serve`, `reconcile`, ...), as `docker-compose.yml` does.

A later migration to separate containers (Rust API + Nginx/Caddy for static
assets) is straightforward when traffic or team structure warrants it.

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

### Offline Demo (`demo`)

```bash
# Replay the committed capture (demo/data) as a simulated live feed + full API
cargo run --release -- demo            # or: just demo

# Fast-forward playback, custom capture location
cargo run --release -- demo --speed 10 --data-dir /path/to/capture
```

`demo` discovers assets and the time window from the capture's Parquet layout,
pre-seeds every asset's book from its first captured venue snapshot, then
streams the records into the same read model the live pipeline uses — at
original cadence (divided by `--speed`), looping forever, with provenance
timestamps shifted to the wall clock so live surfaces read as current.
Historical routes (replay, integrity) answer from the capture with its
original timestamps; the startup banner prints copy-paste examples with
known-good values. Any directory produced by `ingest` works as
`--data-dir`.

### Docker Compose (one command)

```bash
# One-container live workstation: feed + API + web UI at http://localhost:3000
docker compose --profile minimal up --build      # or: just up

# Production process separation: ingest + serve (shared WAL volume)
docker compose --profile full up --build         # or: just up-full

# Full topology plus Prometheus/Alertmanager/Grafana (Grafana at :3001)
docker compose --profile full --profile observability up --build   # or: just up-obs
```

Profiles are mutually exclusive on ports — pick one of `minimal` or `full`.
The image serves the bundled web UI from the same process via
`PB__API__STATIC_ASSETS_DIR` (see below); only localhost ports are mapped.

### Serving the Web UI from the API process

When `api.static_assets_dir` (env: `PB__API__STATIC_ASSETS_DIR`) points at a
built `web/dist`, any non-API route serves the SPA with an `index.html`
fallback, so one process hosts both the API and the workstation UI. Unset
(the default outside Docker) keeps the API-only behavior. API, health, and
WebSocket routes always take precedence over static files.

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

# Or pair with a rotating ingest: serve follows whatever the WAL carries
cargo run -- ingest --auto-rotate   # terminal 1
cargo run -- serve              # terminal 2
```

The `serve` process hydrates from the latest `BookCheckpoint`, replays WAL
records from that offset, then live-tails the WAL. It can be killed and
restarted without data loss. Without `--tokens` it follows the WAL: assets
appear as ingest writes them and drop off ten minutes after their last record.

### Storage Recovery (`reconcile`)

The Parquet sink buffers up to `storage.parquet_flush_interval_secs` (default
300s) in memory. A crash, OOM, or SIGKILL drops that buffered window from
Parquet — but the WAL captured the same records durably. To rebuild the lost
storage window from the WAL:

```bash
# Stop ingest and every other process writing the same Parquet prefix first.
# Reconcile acquires the exclusive WAL lease and refuses to run while ingest is
# active; standalone backfill/append writers must be stopped operationally.
cargo run -- reconcile
```

`reconcile` starts at the earliest retained segment and uses strict WAL reads:
CRC failures, truncation, internal segment gaps, and codec failures abort before
publication. A UTC hour is eligible only when the first supported receive
timestamp is strictly before its start and the last supported receive timestamp
reaches or passes its end. This deliberately excludes the hour containing the
first retained record even when that record lands exactly on an hour boundary:
retention may have cut through an equal-timestamp snapshot batch. A trailing
partial hour is also skipped. An empty WAL is a successful no-op; a non-empty WAL
that proves no complete eligible hour exits non-zero and changes no Parquet data.
Receive timestamps must also be nondecreasing in WAL order; a clock/order
regression fails closed rather than weakening the coverage proof.
The command validates coverage in a first streaming pass, then replays one
eligible hour at a time, so it does not materialize the whole retained WAL in
memory. Before publication, it also refuses any eligible hour above 2,000,000
records or 128 MiB of encoded WAL payload to bound working-set risk for the
default 512 MiB maintenance-task class.

Only datasets partitioned exactly by receive time are eligible: `book_events`,
`trade_events`, and `ingest_events`. `book_checkpoints` uses exchange snapshot
time, while validation timestamps are independently supplied; those
partitions are left untouched because the retained WAL endpoints cannot prove
them complete.

For each eligible `(dataset, asset, hour)`, recovery performs a two-phase cut:

1. Write and size-verify an immutable object under `_recovery_objects/`, then
   publish a versioned manifest under `_recovery_manifests/` that makes that
   staged object authoritative to manifest-aware readers.
2. Write and verify the same bytes in the normal dataset/hour partition, then
   publish the final manifest pointing at the promoted normal object.
3. Delete superseded normal/staged objects. Their manifest tombstones remain so
   delayed or manually restored stale objects cannot become visible again.

Manifest-aware readers switch at each publication point and ignore superseded
files, so a crash leaves a complete authoritative view. Cleanup is retryable by
re-running `reconcile`. If any deletion remains unresolved, the authoritative
application view is still published but the command exits non-zero; direct
DuckDB/Polars scans are unsafe until a rerun reports zero cleanup failures.
After clean promotion and cleanup, the active object is in the normal tree for
direct inspection. The command is idempotent and does not commit a consumer
position.
Do not run `backfill` or another Parquet-writing task against the same prefix
concurrently: those commands do not share the WAL lease,
and a write racing the manifest cut would make the recovered view ambiguous.
Application reads retry manifest resolution once if cleanup crosses a request.
Stop ad-hoc DuckDB/Polars scans during the maintenance cut, and do not resume
them after a non-zero cleanup result, because those tools do not interpret
recovery manifests.
In a container deployment, run this as a one-off maintenance task;
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
| Parquet sink buffer lost (OOM/SIGKILL) | `reconcile` strictly republishes complete hourly partitions from the durable WAL (offline); the earliest retained hour and any trailing partial hour remain untouched | 0 for a fully covered, gap-free WAL hour | minutes — offline rebuild, scales with window |
| Host loss (WAL on a durable shared volume) | Run a hot standby `ingest --standby` against the shared WAL volume; it waits on the lock and **auto-promotes** the moment the primary's lock releases | ≤ last synced records, as above | seconds-to-minutes — standby poll interval + feed connect, once the standby is already running |
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
  `promtool check rules` + `promtool test rules`, so rule rot is caught without
  a live Prometheus. `monitoring/alertmanager.yml` is the routing config the
  `observability` compose profile mounts; its receivers are placeholders.

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
advanced past are pruned automatically by the ingest process (lag-
byte retention window), so disk usage stays bounded.

### Port Defaults

| Service | Port  |
|---------|-------|
| API     | 3000  |
| Metrics | 9090  |

### Current Scope

- read-only HTTP and WebSocket API
- live feed status and active asset visibility
- live in-memory order book snapshots and per-asset WS streaming
- Parquet-backed replay reconstruction and integrity summaries
- WAL gap detection, lag tracking, and backpressure-aware pruning
- health endpoint with hydration and WAL status

### Not Yet Provided

- latency summary endpoints


## Workstation Web App

The SPA currently ships:

- `Live Feed`
- `Orderbook`
- `Replay Lab`
- `Integrity`

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
