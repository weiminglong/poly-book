# Market Data Upgrades -- Design

## Overview

The upgrade path is intentionally phased because the standards for:

- signal research
- historical backtesting
- live market making

are materially different.

The current system stores a normalized stream of `OrderbookEvent` values and
reconstructs an L2 book by finding the latest stored snapshot and applying
subsequent deltas. That design remains the base layer, but it needs additional
datasets and stricter semantics.

## Phase 1: Research-Grade Integrity

Phase 1 upgrades stored market data so replay consumers can reason about data
quality instead of inferring it from logs.

### Objectives

- Persist ingest lifecycle events such as reconnects, stale snapshots, and
  sequence gaps.
- Preserve enough provenance to distinguish exchange-time ordering from
  receive-time ordering.
- Separate book events from trade events so downstream readers do not need to
  infer semantics from overloaded rows.

### Data Model

Phase 1 introduces:

- `book_events`: snapshot and delta rows for book reconstruction
- `trade_events`: trade-price rows with source provenance, even if full fill
  detail is not yet available
- `ingest_events`: reconnects, source resets, gap markers, stale snapshot
  skips, and related lifecycle conditions

Each dataset must keep `recv_timestamp_us` and `exchange_timestamp_us`, plus
source metadata needed to explain event origin and ordering.

### Replay Semantics

Readers must surface continuity metadata alongside event streams. A replay
consumer may still choose best-effort reconstruction, but gaps can no longer be
only warning logs.

## Phase 2: Backtest-Credible Replay

Phase 2 upgrades historical replay so a strategy can use stored data for
meaningful backtests rather than only rough book-state reconstruction.

### Objectives

- Support replay keyed by either exchange time or receive time.
- Store richer trade events when the venue exposes them.
- Add periodic full-book checkpoints to reduce recovery cost and bound drift.
- Validate reconstructed books against later snapshots and persist drift
  findings.

### Data Model

Phase 2 extends the persisted datasets with:

- `book_checkpoints`: periodic full L2 snapshots keyed by asset and timestamp
- richer `trade_events` fields such as size, side, and stable trade identity
  when available
- replay validation outputs that record whether a reconstructed book matches a
  later checkpoint or snapshot

### Replay Modes

Two replay timelines are explicitly supported:

- `recv_time`: reproduces what the local process observed
- `exchange_time`: approximates venue ordering for historical analysis

Consumers must select a mode explicitly to avoid silent ambiguity.

## Phase 3: Live Execution Readiness

Phase 3 adds the strategy-state data required for production market making.

### Objectives

- Persist the full lifecycle of local orders and fills.
- Measure end-to-end latencies from market-data receipt through order
  acknowledgement and execution.
- Keep market-data truth separate from execution truth.

### Execution Journal

Phase 3 introduces an append-only `execution_events` dataset that records:

- order submission intent
- exchange acknowledgement
- cancel requests and acknowledgements
- rejects
- partial fills
- full fills
- local terminal state transitions

This dataset is distinct from public market data and is the source of truth for
execution replay.

In this repository, Phase 3 stops at append and replay surfaces for execution
state. Live order routing and exchange connectivity remain out of scope.

### Latency Telemetry

Execution readiness requires persisted timestamps for:

- websocket receive
- normalization completion
- strategy decision
- order submit
- exchange acknowledgement
- exchange fill

This supports both live observability and latency-aware simulation.

## Dataset Boundaries

The upgraded system separates persisted concerns into independent datasets:

- `book_events`
- `trade_events`
- `ingest_events`
- `book_checkpoints`
- `execution_events`

This keeps replay, analytics, and execution logic from sharing one overloaded
schema.

## Rollout Strategy

The rollout order is:

1. Phase 1 integrity events and dual-clock provenance
2. Phase 2 replay modes, checkpoints, and validation
3. Phase 3 execution journal and latency telemetry

This order ensures data quality is established before backtests depend on it,
and execution-state storage is added only after market-data replay semantics are
stable.
