# Tasks: Replay Engine and REST Backfill

## EventReader Trait and Implementations

- [x] Define `EventReader` trait with `read_events` async method
- [x] Implement `ParquetReader` with time-partitioned directory layout scanning
- [x] Implement `hour_paths` helper to generate hourly directory paths from timestamp range
- [x] Implement `read_parquet_file` with Arrow async streaming
- [x] Implement `extract_events` to deserialize record batches into `OrderbookEvent`
- [x] Handle missing hourly directories gracefully in `ParquetReader`
- [x] Sort ParquetReader results by `recv_timestamp_us`
- [x] Implement `ClickHouseReader` with parameterized SQL queries
- [x] Define `EventRow` struct with `clickhouse::Row` derive for deserialization
- [x] Implement `row_to_event` conversion from string-typed fields to domain enums

## Replay Engine

- [x] Implement `ReplayEngine` struct generic over `EventReader`
- [x] Implement `reconstruct_at` with snapshot search and delta application
- [x] Use `rposition` to find the most recent snapshot within lookback window
- [x] Collect all snapshot events at the same timestamp for full book state
- [x] Apply `L2Book::apply_snapshot` from aggregated bids/asks
- [x] Apply deltas between snapshot and target timestamp
- [x] Implement `replay_range` for raw event retrieval
- [x] Add configurable lookback window with `with_lookback_us` builder method

## Backfill

- [x] Define `BackfillConfig` with REST URL, token IDs, interval, rate limit pause
- [x] Implement `run_backfill` loop with periodic REST snapshot fetching
- [x] Implement `fetch_snapshot` HTTP GET to `/book?token_id={id}`
- [x] Convert `RestBookResponse` bid/ask entries to `OrderbookEvent` with `EventType::Snapshot`
- [x] Assign monotonically increasing sequence numbers
- [x] Send events via `mpsc::Sender` with clean shutdown on channel close
- [x] Add rate limiting pause between consecutive HTTP requests
- [x] Implement `parse_price` and `parse_size` helpers

## Error Handling

- [x] Define `ReplayError` enum with IO, Parquet, Arrow, ClickHouse, HTTP variants
- [x] Add `NoSnapshotFound` error with asset_id and timestamp context
- [x] Add `InvalidEventType` and `InvalidSide` errors for stored data validation
- [x] Derive `thiserror::Error` with `#[from]` conversions

## CLI Integration

- [x] Add `Replay` subcommand with `--token`, `--at`, `--source` flags
- [x] Add `Backfill` subcommand with `--tokens`, `--interval-secs`, `--duration-mins` flags
- [x] Implement `commands::replay::run` handler
- [x] Implement `commands::backfill::run` handler
