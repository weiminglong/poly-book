# Storage Pipeline -- Design

## Architecture Overview

Events flow from the feed dispatcher into two independent sinks via separate
`mpsc` channels (fan-out at the orchestrator level):

```
Dispatcher ──[OrderbookEvent]──> ParquetSink  (cold, 5-min flush)
             ──[OrderbookEvent]──> ClickHouseSink (warm, 1s batch)
```

Each sink runs as an independent Tokio task. Both drain their channel on
shutdown to avoid data loss.

## Arrow Schema

`orderbook_schema()` defines a canonical columnar schema:

| Field                  | Arrow Type | Nullable |
|------------------------|------------|----------|
| recv_timestamp_us      | UInt64     | no       |
| exchange_timestamp_us  | UInt64     | no       |
| asset_id               | Utf8       | no       |
| event_type             | UInt8      | no       |
| side                   | UInt8      | yes      |
| price                  | UInt32     | no       |
| size                   | UInt64     | no       |
| sequence               | UInt64     | no       |

`event_type` encodes as `1=Snapshot, 2=Delta, 3=Trade`.
`side` encodes as `1=Bid, 2=Ask, null=N/A`.
`price` and `size` store raw fixed-point integer representations.

`events_to_record_batch` builds column arrays from a slice of `OrderbookEvent`
and assembles them into a `RecordBatch`.

## Parquet Sink (Cold Storage)

### Flush Strategy

A `tokio::select!` loop reads events into a `Vec` buffer. A
`tokio::time::interval` (default 300 s) triggers flushes. On channel close,
remaining events are flushed before shutdown.

### File Layout

Files are time-partitioned and keyed by asset:

```
{base_path}/{YYYY}/{MM}/{DD}/{HH}/events_{asset_id}_{timestamp_micros}.parquet
```

Events are grouped by `asset_id` before writing so each file contains a
single asset's data.

### Writer Configuration

- Compression: Zstd (default level)
- Max row group size: 65,536 rows
- Backend: any `object_store::ObjectStore` impl (local filesystem, S3, GCS)

The `ArrowWriter` serializes the `RecordBatch` into an in-memory buffer, which
is then PUT to the object store as a single payload.

## ClickHouse Sink (Warm Storage)

### Table Schema

```sql
CREATE TABLE IF NOT EXISTS orderbook_events (
    recv_timestamp_us UInt64,
    exchange_timestamp_us UInt64,
    asset_id String,
    event_type Enum8('Snapshot'=1, 'Delta'=2, 'Trade'=3),
    side Nullable(Enum8('Bid'=1, 'Ask'=2)),
    price UInt32,
    size UInt64,
    sequence UInt64,
    event_date Date MATERIALIZED toDate(fromUnixTimestamp64Micro(exchange_timestamp_us))
) ENGINE = ReplacingMergeTree()
PARTITION BY event_date
ORDER BY (asset_id, exchange_timestamp_us, sequence)
TTL event_date + INTERVAL 90 DAY
```

### Batch Insert Strategy

A `tokio::select!` loop buffers events and flushes when either:

1. Buffer reaches 10,000 rows, or
2. 1-second interval timer fires.

On channel close, remaining events are flushed.

Each flush converts `OrderbookEvent` values to `ClickHouseRow` structs (with
string-typed enum fields), writes them into a ClickHouse `INSERT` block, and
calls `insert.end()`.

### Table Initialization

`ensure_table()` runs the DDL at startup, creating the table if it does not
exist.

## Error Handling

`StoreError` is a `thiserror` enum with `From` impls for
`parquet::errors::ParquetError`, `arrow::error::ArrowError`,
`clickhouse::error::Error`, `std::io::Error`, and `object_store::Error`.
