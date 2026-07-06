# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/) where it
is practical to do so.

## [Unreleased]

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
