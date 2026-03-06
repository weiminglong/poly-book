# Market Data Upgrades -- Tasks

## Phase 1: Research-Grade Integrity

- [x] Define persisted ingest lifecycle event types for reconnects, gap markers, stale snapshots, and source resets
- [x] Introduce distinct `book_events`, `trade_events`, and `ingest_events` storage schemas
- [x] Add provenance fields needed to distinguish receive-time and exchange-time ordering
- [x] Update feed normalization to emit lifecycle events instead of only logging them
- [x] Update storage sinks and readers to persist and read the new datasets

## Phase 2: Backtest-Credible Replay

- [x] Add explicit replay mode selection for `recv_time` versus `exchange_time`
- [x] Extend trade-event storage to include real trade size and side when available from the venue
- [x] Implement periodic `book_checkpoints` persistence
- [x] Generate live checkpoints in `ingest` and `auto-ingest` through a shared producer
- [x] Add replay validation that compares reconstructed books against later snapshots or checkpoints
- [x] Route replay validation persistence through reusable `pb-store` writer helpers
- [x] Update CLI and replay APIs to expose timeline mode, validation state, and checkpoint usage

## Phase 3: Live Execution Readiness

- [x] Define append-only `execution_events` schema for orders, cancels, rejects, and fills
- [x] Persist end-to-end latency timestamps spanning ingest, strategy, order submit, acknowledgement, and fill
- [x] Add readers and analytics views for execution replay
- [x] Add a concrete CLI append path for `execution_events`
- [x] Keep execution storage physically and logically separate from public market-data storage
- [x] Restore ClickHouse runtime coverage for market data, checkpoints/validation, and execution events
- [x] Document the boundary between market-data replay and execution replay in the CLI and design docs
