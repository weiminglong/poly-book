# Design: Replay Engine and REST Backfill

## Overview

The replay system reconstructs historical L2 orderbook state by finding the nearest snapshot event before a target timestamp and applying subsequent delta events. Two storage backends are supported through a common trait. A separate backfill subsystem periodically fetches REST API snapshots for gap-filling.

## Replay Strategy

1. Given a target timestamp `T` and a lookback window (default 1 hour), read all events in `[T - lookback, T]`
2. Find the most recent `Snapshot` event at or before `T` using reverse position search
3. Collect all snapshot events at that exact timestamp to build the full bid/ask state
4. Apply `L2Book::apply_snapshot` with the aggregated bids and asks
5. Iterate forward through remaining events, applying each `Delta` event via `L2Book::apply_delta`
6. Stop when event timestamps exceed `T`
7. Return the reconstructed `L2Book`

If no snapshot is found within the lookback window, return `ReplayError::NoSnapshotFound`.

## EventReader Trait

```rust
pub trait EventReader: Send + Sync {
    fn read_events(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> impl Future<Output = Result<Vec<OrderbookEvent>, ReplayError>> + Send;
}
```

All readers return events sorted by `recv_timestamp_us`.

## ParquetReader

- Scans a time-partitioned directory layout: `{base_path}/{YYYY}/{MM}/{DD}/{HH}/events_*.parquet`
- Computes the set of hourly directory paths covering `[start_us, end_us]` using chrono
- For each hourly directory, reads all `events_*.parquet` files
- Uses Arrow's `ParquetRecordBatchStreamBuilder` for async streaming reads
- Extracts columns: `recv_timestamp_us`, `exchange_timestamp_us`, `asset_id`, `event_type`, `side`, `price`, `size`, `sequence`
- Filters rows by `asset_id` match and timestamp range
- Skips missing hourly directories gracefully (logs at debug level)
- Sorts all collected events by `recv_timestamp_us` before returning

## ClickHouseReader

- Connects via `clickhouse::Client` with configurable URL and database
- Queries the `orderbook_events` table with parameterized WHERE clause on `asset_id` and timestamp range
- Deserializes rows via `clickhouse::Row` derive into an intermediate `EventRow` struct
- Converts string-typed `event_type` and `side` fields to domain enums
- Results are ordered by `recv_timestamp_us` via SQL `ORDER BY`

## Backfill Loop

- Configured via `BackfillConfig`: REST base URL, list of token IDs, fetch interval, rate limit pause
- Uses `reqwest::Client` to GET `/book?token_id={id}` for each token
- Parses response as `RestBookResponse` (bid/ask price-size arrays)
- Converts each bid/ask entry into an `OrderbookEvent` with `EventType::Snapshot`
- Assigns monotonically increasing sequence numbers
- Sends events through an `mpsc::Sender<OrderbookEvent>` channel
- Pauses `rate_limit_pause` between consecutive HTTP requests
- Pauses `interval` between full iteration cycles
- Exits cleanly when the channel receiver is dropped

## Error Handling

`ReplayError` covers:
- IO, Parquet, Arrow, ClickHouse driver errors (via `#[from]`)
- Domain errors: `NoSnapshotFound`, `InvalidEventType`, `InvalidSide`
- HTTP errors from backfill REST calls
- Type conversion errors from `pb_types`
