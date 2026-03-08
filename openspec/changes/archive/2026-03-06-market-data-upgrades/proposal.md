# Market Data Upgrades

## Why

The current pipeline stores normalized L2 orderbook events and can reconstruct
book state from snapshots plus deltas. That is sufficient for basic replay and
feature research, but it is not sufficient for higher-confidence backtesting or
production market making:

- replay tolerates sequence gaps with warnings instead of treating them as a
  first-class data-quality condition
- trade events are currently last-trade-price updates without trade size or
  aggressor side
- replay is implicitly ordered by local receive time only
- no execution journal exists for the strategy's own orders, acknowledgements,
  cancels, rejects, and fills

The system needs a phased upgrade path that separates research-grade data
integrity from execution-grade trading support.

## What Changes

- Define a phased market-data evolution plan with one capability package per
  phase.
- Phase 1 upgrades the stored event model for data integrity and replay safety.
- Phase 2 upgrades replay and storage for credible historical backtesting.
- Phase 3 adds execution-state and latency capture required for live market
  making.

## Capabilities

### New Capabilities

- `research-grade-integrity`: persistent ingest lifecycle events, explicit gap
  tracking, dual-clock provenance, and separation of book events from trade
  events.
- `backtest-credible-replay`: replay modes keyed by exchange time or receive
  time, richer trade-event storage, periodic book checkpoints, and replay
  validation against later snapshots.
- `live-execution-readiness`: append-only execution journal, end-to-end latency
  telemetry, and separation of market-data truth from strategy-state truth.

### Modified Capabilities

- `storage-pipeline`: evolves from one generic orderbook event stream into
  multiple datasets with explicit data-quality and execution semantics.
- `replay-backfill`: evolves from best-effort book reconstruction into
  validated replay with selectable timeline semantics.

## Impact

- Adds a new active OpenSpec change that defines the target state for future
  storage, replay, and execution work.
- Affects `pb-feed`, `pb-store`, `pb-replay`, `pb-book`, and the CLI layer.
- Introduces new persisted datasets beyond `orderbook_events`, including trade,
  ingest-lifecycle, checkpoint, and execution-event records.
