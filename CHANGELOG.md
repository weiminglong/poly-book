# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/) where it
is practical to do so.

## [Unreleased]

### Fixed

- The dispatcher's venue cross-check raised a `BookMismatch` on nearly every
  delta of a market with an empty ask side (the venue reports that side as
  `best_ask = "1"`, which was treated as a real level) and on every
  intermediate state of a multi-entry `price_change` message. It now treats
  `"0"`/`"1"` as side-specific empty markers and compares once per asset per
  message. The committed demo capture's `ingest_events` were rewritten without
  the 16,998 spurious events (56 genuine one-tick disagreements remain); its
  venue data is unchanged.
- `serve` no longer requires `--tokens`. Without it, it follows whatever the
  ingest process writes to the WAL, so the `full` compose profile (rotating
  `ingest --auto-rotate` + `serve`) starts and tracks the live market instead of
  failing with a usage error.

### Changed

- WAL codec v3: `FixedPrice` and `FixedSize` are stored as their raw scaled
  integers (4 and 8 bytes) instead of length-prefixed decimal strings, which
  shrinks a book delta frame by about a sixth and removes a decimal parse from
  every replayed record. The serde impls now branch on
  `is_human_readable()`, so JSON responses still carry exact decimal strings.
  v2 frames are rejected with a drain-the-WAL hint like v1. The decimal
  formatter is plain `write!` now, which removed the last four `unsafe` blocks
  in the workspace; `unsafe_code = "forbid"` has no exceptions, and the miri
  CI job that existed to check those blocks is gone.
- `ingest` and `auto-ingest` are one command: `ingest --tokens A,B` or
  `ingest --auto-rotate`. The WAL drain loop and the five-minute rotation loop
  each existed twice (the second rotation copy, in `serve-api`, cancelled the
  outgoing market immediately and so dropped its final seconds from the live
  view). One shared feed runtime now drives both `ingest` and `serve-api`,
  with the REST resnapshot self-heal wired into every subscription.
- Configuration is one typed `Settings` struct deserialized once from
  `config/default.toml` plus `PB__` overrides. An unknown or misspelled key is
  now a startup error naming the field instead of a warning (the separate
  key-allowlist module and `doctor`'s copy of that check are gone), negative
  values are rejected, and out-of-range values are clamped in one place. The
  dead `storage.parquet_row_group_size` key is removed. `--log-level info` on
  the command line now wins over `logging.level` in the file, as any explicit
  flag should.
- CI trimmed from 15 jobs to 9, dropping the two longest (`bench` at about 13
  minutes and `fuzz` at about 12) from the per-PR gate: the
  `bench` compile-only job (redundant with `cargo check --all-targets`),
  `release-build` (subsumed by `docker-build`), `hygiene`, `audit`
  (duplicated `cargo-deny`), the CodeQL workflow (it scanned only YAML and
  TypeScript), and the Alertmanager routing assertions are gone. The fuzz smoke
  runs moved to `fuzz.yml`, nightly and on pushes to `main`.

### Removed

- About 800 lines of tests that only restated `#[derive]`s and `match` arms
  (serde round-trips of derived impls, `Display` of enum names, `Default` is
  zero, HTTP status code tables), along with the dead code some of them
  existed for: the `status_to_static` label table, two unused metrics-server
  entry points, a bench-only duplicate of the read-model consumer, the
  never-constructed `CompletenessLevel::Sparse`, and two integrity-summary
  fields that were always zero. Property tests, golden fixtures, and the
  behavioral tests are untouched.
- ClickHouse as a second storage backend and query engine: the sink, record
  writer, reader, DDL, `storage.clickhouse_*` and `api.historical_backend`
  config keys, the `--clickhouse` ingest flags, the `--source` replay flag,
  the Docker-backed round-trip and cross-backend tests, and the compose
  service. Parquet was already the default and the only backend `demo`,
  `serve` hydration, and the checkpoint path used; every reader change had to
  be made twice. Ad-hoc SQL over the Parquet tree is available with DuckDB
  (`just parquet-*`).
- Execution journaling: the `ExecutionEvent`/`LatencyTrace` types, the
  `execution_events` dataset, `execution-append`/`execution-replay`, the
  `GET /api/v1/execution/orders` route, and the SPA's Execution page. Nothing
  in the system produced execution events except a human typing them in.
- The SQL query workbench (`/api/v1/query/*`, `api.query_*` keys, the
  `fuzz_query_guard` target, the SPA's Query page). It required ClickHouse.
- The pb-service trait and enum-dispatch layer (`BookService`, `ReplayService`,
  `IntegrityService`, `ExecutionService`, `QueryService`, `Any*Service`).
  `BookService` had no implementor and the others had exactly one backend
  left. One struct, `HistoricalService`, now answers replay and integrity
  queries.
- `infra/` (Terraform that was never applied), the `deploy.yml` workflow (never
  succeeded, and it re-ran the full CI on every push to `main`), the
  `iac-scan` job, the `openspec/` archive (its decisions live in `docs/adr/`),
  `AGENTS.md` (a diverging copy of `CLAUDE.md`), `docs/latency.md`
  (superseded by the measured `docs/PERFORMANCE.md`), the verbatim config
  re-dump in `docs/operations.md`, and the per-crate "Docs to Update After
  Changes" tables. Each fact now has one documented home.
- The gRPC surface (`pb-grpc`, the `[grpc]` config section, the tonic/prost
  dependencies, and the `protoc` build requirement). It mirrored the HTTP
  routes, was disabled by default, and had no consumer. A fresh clone now
  builds with only a Rust toolchain.

## [0.1.0] - 2026-07-06

The first tagged release: the ingestion, durability, replay, and workstation
surfaces are complete and verified end to end, the demo paths run with zero
infrastructure, and the release pipeline publishes runnable artifacts.

- Hardening bundle: `unsafe` is now forbidden workspace-wide via
  `[workspace.lints]` (pb-types alone overrides to `deny` for its four
  SAFETY-documented, miri-verified serializer sites — policy recorded in
  SECURITY.md); two liveness alerts close the blindest failure mode
  (`TargetDown` on `up == 0` and `IngestAbsent` on a missing ingest series,
  with promtool incident tests and runbook sections — every other alert
  evaluates to an empty vector when the process is simply gone); and a new
  `integration-docker` CI job runs the previously `#[ignore]`d Docker-backed
  integration cases (ClickHouse roundtrips, Parquet-vs-ClickHouse cross-backend
  equivalence, MinIO S3 roundtrip) on every PR.
- Order-book design claims are now measured: a new `array_book` benchmark
  runs an epoch-stamped dense-array book (the bounded-grid alternative an
  order-book design discussion starts from) against `L2Book` on identical
  workloads, and ADR-0002 records the results — the array wins the write path
  10–19x, ties top-of-book — together with why BTreeMap is retained (book ops
  are not the pipeline bottleneck; no grid coupling; proportional memory) and
  the crossover condition for switching. Incorrect "O(1) best bid/ask" claims
  in the README, architecture overview, and latency doc now cite the measured
  ~3 ns figure instead.
- Published performance report: `docs/PERFORMANCE.md` carries measured,
  machine/toolchain/commit-stamped Criterion medians for every hot-path stage
  (wire deserialize, dispatcher normalize + shadow-book, WAL encode/append,
  book operations, read model), regenerated with `just bench-report`
  (`scripts/bench-report.py`). The README benchmarks section now leads with
  the measured numbers.
- Engineering-narrative documentation: a root-level `TESTING.md` mapping each
  failure mode to the layer that catches it (with verified counts and run
  commands, plus an explicit not-yet-covered section), and four new ADRs
  (0008–0011) documenting the embedded WAL over a message broker, dual-sink
  Parquet/ClickHouse storage, ingest/serve process separation, and the
  read-only workstation boundary. The ADR index in `docs/architecture.md` now
  lists all eleven ADRs.
- Research access: `research/orderbook_analysis.ipynb`, an executed notebook
  (plots embedded, renders on GitHub) computing microstructure analytics —
  top-of-book/mid/spread series, microprice, Cont–Kukanov–Stoikov order-flow
  imbalance vs mid moves, trade signs and effective/realized spreads, and
  binary-outcome complementarity/pinning — directly over the committed
  `demo/data/` Parquet capture with polars + matplotlib only, plus
  `examples/api-cookbook.md` with one captured request/response example per
  HTTP route and the WebSocket stream, both runnable fully offline
  (`research/README.md` documents the one-line runner).
- Preflight checks: a new `doctor` subcommand prints a pass/warn/fail table
  covering config-key validation (hard failure on unknown keys), Parquet path
  writability, WAL directory state, REST/Gamma reachability, a full WebSocket
  TLS handshake through the feed's own connector, ClickHouse ping (warn-only),
  and port availability — exiting non-zero on failure so it can gate deploys.
  `--skip-network` for offline/CI use. pb-feed gains a public `probe_ws`
  helper for the handshake probe.
- Offline demo: a new `demo` subcommand replays a committed capture of real
  BTC-5-minute market data (`demo/data/`, recorded with `auto-ingest`) as a
  simulated live feed behind the full workstation API — original cadence
  (`--speed` to fast-forward), looping, provenance timestamps shifted to the
  wall clock, every asset pre-seeded from its first captured venue snapshot so
  live routes answer immediately. Replay, integrity, and execution answer from
  the capture with original timestamps, and the startup banner prints
  copy-paste examples with known-good values. `just demo` wraps it.
- One-command local topologies: a top-level `docker-compose.yml` with three
  profiles — `minimal` (one container: live feed + API + web UI),
  `full` (production process separation: ingest + serve over a shared WAL
  volume, plus ClickHouse), and `observability` (Prometheus with the alert
  rules loaded, Alertmanager, and Grafana with the committed dashboard
  provisioned). `just up` / `up-full` / `up-obs` wrap the profiles.
- The API process can now serve the bundled web UI: `api.static_assets_dir`
  (env: `PB__API__STATIC_ASSETS_DIR`) enables a static-file fallback with SPA
  deep-link support on `serve-api` and `serve`. The Docker image enables it
  against its bundled assets and its default command is now the combined live
  workstation (`serve-api --auto-rotate`) instead of a help screen.
- Hosted demo: the workstation SPA now deploys to GitHub Pages in
  backend-free demo mode on every `main` push (`Live demo` link in the
  README). Demo mode gained a client-side market simulator that streams a
  moving order book (random-walk mid, level churn, bounded probability band)
  through the same message shape as the real WebSocket broadcast, replacing
  the frozen fixture book. The stream hook no longer opens a real WebSocket
  in demo mode (previously a permanent amber "Reconnecting" badge), the SQL
  workbench returns fixture results instead of a 500, the replay form
  prefills a known-good timestamp, fixture clocks are anchored to page load,
  and the zero-data orderbook page now explains how to get data. New favicon
  and page metadata; WS staleness is now surfaced as a badge.
- Fixed: all live-feed commands (`ingest`, `auto-ingest`, `serve-api`) panicked
  at WebSocket TLS setup ("Could not automatically determine the process-level
  CryptoProvider") because the dependency graph links two rustls crypto
  providers. The WS connector now pins the `ring` provider explicitly, and the
  binary installs a process-level default at startup for all other TLS users.
- Fixed: `serve-api --tokens` logged spurious "live runtime did not shut down
  within timeout" warnings during healthy operation (and stopped supervising
  the feed tasks after 20s) because the shutdown drain ran at spawn time
  instead of after a shutdown request.
- CLOB V2 ingest compatibility (Polymarket cutover 2026-04-28):
  parse the new `tick_size_change` WebSocket event (no PersistedRecord;
  observed via the `pb_messages_received_total{event_type="tick_size_change"}`
  counter), accept the new optional fields on `GET /book` responses
  (`tick_size`, `min_order_size`, `neg_risk`, `last_trade_price`), and add
  `RestClient::get_clob_market_info(condition_id)` for the V2
  `/clob-markets/{condition_id}` metadata endpoint. WebSocket URL,
  subscription payload, and existing `book` / `price_change` /
  `last_trade_price` event shapes are unchanged in V2.
- Repository hardening for public open-source collaboration:
  README rewrite, operations docs split-out, community health files, and
  package metadata cleanup.
- Second-pass GitHub maintainer improvements:
  Dependabot, cargo-deny workflow, release workflow, CODEOWNERS, and
  release process documentation.
