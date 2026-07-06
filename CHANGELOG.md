# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/) where it
is practical to do so.

## [Unreleased]

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
