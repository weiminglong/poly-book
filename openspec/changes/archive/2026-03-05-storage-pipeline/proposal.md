# Storage Pipeline

## Why

Raw orderbook events must be persisted for offline analysis, backtesting, and
operational monitoring. A dual-sink architecture serves two access patterns:

- **Cold storage (Parquet)**: Compressed columnar files flushed every 5 minutes,
  partitioned by date and hour, stored via `object_store` for cheap bulk reads.
- **Warm storage (ClickHouse)**: Sub-second batch inserts for near-real-time
  queries, dashboards, and alerting with automatic 90-day TTL.

## What Changes

- Add `orderbook_schema()` returning an Arrow `Schema` for all event fields.
- Add `events_to_record_batch` converting `&[OrderbookEvent]` to Arrow
  `RecordBatch`.
- Add `ParquetSink` that buffers events and flushes Zstd-compressed Parquet
  files to any `ObjectStore` backend every 5 minutes, partitioned by
  `YYYY/MM/DD/HH`.
- Add `ClickHouseSink` that batch-inserts events into a
  `ReplacingMergeTree` table every 1 second or 10,000 rows.
- Add `StoreError` enum covering Parquet, Arrow, ClickHouse, IO, and
  object-store failures.

## Capabilities

### New Capabilities

- `parquet-cold-storage`: Time-partitioned Parquet file writer with
  Zstd compression, 64K row groups, 5-minute flush interval, and
  `object_store` abstraction for local/S3/GCS backends.
- `clickhouse-warm-storage`: ClickHouse batch inserter with 1-second / 10K-row
  flush triggers, `ReplacingMergeTree` engine, date partitioning, and 90-day
  TTL.
- `arrow-schema`: Canonical Arrow schema and `OrderbookEvent` to `RecordBatch`
  conversion for columnar processing.

### Modified Capabilities

(none)

## Impact

- New crate `pb-store` with modules: `schema`, `parquet_sink`,
  `clickhouse_sink`, `error`.
- Depends on `pb-types` for `OrderbookEvent`, `Side`, `EventType`.
- Receives events via `tokio::sync::mpsc` channel from the feed pipeline.
