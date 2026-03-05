# Proposal: Replay Engine and REST Backfill

## Why

The ingestion pipeline captures live orderbook events, but there is no way to reconstruct historical book state or fill gaps when the system is offline. A replay engine is needed to rebuild the L2 orderbook at any past timestamp from stored snapshots and deltas. Additionally, a REST-based backfill loop provides periodic snapshot collection independent of the WebSocket feed, ensuring data continuity and enabling historical data seeding.

## What Changes

- Add `EventReader` trait abstracting over event storage backends (Parquet, ClickHouse)
- Implement `ParquetReader` with time-partitioned directory scanning (`YYYY/MM/DD/HH`)
- Implement `ClickHouseReader` with parameterized SQL queries
- Add `ReplayEngine` that finds the nearest snapshot and applies deltas to reconstruct `L2Book` at any timestamp
- Add configurable lookback window (default 1 hour) for snapshot search
- Add `run_backfill` loop that periodically fetches REST API snapshots and emits them as `OrderbookEvent`s via an mpsc channel
- Define `ReplayError` covering IO, Parquet, Arrow, ClickHouse, HTTP, and domain errors
- Add `replay` and `backfill` CLI subcommands in `pb-bin`

## Capabilities

### New Capabilities

- `replay-engine`: Reconstruct L2Book at any historical timestamp from stored events
- `event-reader`: Unified trait for reading events from Parquet and ClickHouse
- `backfill`: Periodic REST snapshot fetching for historical data collection

### Modified Capabilities

None.

## Impact

- New crate: `pb-replay` (`crates/pb-replay/`)
  - `src/reader.rs` -- EventReader trait, ParquetReader, ClickHouseReader
  - `src/engine.rs` -- ReplayEngine with reconstruct_at and replay_range
  - `src/backfill.rs` -- BackfillConfig and run_backfill loop
  - `src/error.rs` -- ReplayError enum
- Modified: `crates/pb-bin/src/main.rs` -- added Replay and Backfill subcommands
- Modified: `crates/pb-bin/src/commands/` -- added replay.rs and backfill.rs command handlers
