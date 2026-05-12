# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/) where it
is practical to do so.

## [Unreleased]

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
