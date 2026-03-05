# Storage Sinks Spec

## Schema Conversion

### Converts events to Arrow RecordBatch

**Given** a non-empty slice of `OrderbookEvent` values
**When** `events_to_record_batch` is called
**Then** it returns a `RecordBatch` with columns matching `orderbook_schema()`,
where `event_type` is encoded as `u8` (1/2/3), `side` as nullable `u8`
(1/2/None), and `price`/`size` as raw fixed-point integers

### Encodes event types correctly

**Given** events with types Snapshot, Delta, and Trade
**When** converted to a RecordBatch
**Then** the `event_type` column contains values 1, 2, and 3 respectively

### Handles nullable side

**Given** an event with `side: None` (e.g., a Trade)
**When** converted to a RecordBatch
**Then** the `side` column contains a null value for that row

## Parquet Sink

### Flushes on interval

**Given** a `ParquetSink` with buffered events and default 300s flush interval
**When** the interval timer fires
**Then** all buffered events are written as Zstd-compressed Parquet files
grouped by `asset_id` and the buffer is cleared

### Skips empty flushes

**Given** a `ParquetSink` with an empty buffer
**When** the interval timer fires
**Then** no file is written

### Partitions files by time and asset

**Given** buffered events for asset `ABC`
**When** a flush occurs
**Then** the output path follows the pattern
`{base_path}/{YYYY}/{MM}/{DD}/{HH}/events_ABC_{timestamp_micros}.parquet`

### Groups events by asset ID

**Given** buffered events for assets `A` and `B`
**When** a flush occurs
**Then** two separate Parquet files are written, one per asset

### Flushes remaining events on channel close

**Given** a `ParquetSink` with buffered events
**When** the input channel is closed (sender dropped)
**Then** remaining events are flushed before the task returns `Ok(())`

### Uses Zstd compression with 64K row groups

**Given** any Parquet flush
**When** the `ArrowWriter` is configured
**Then** compression is set to Zstd and max row group size is 65,536

## ClickHouse Sink

### Creates table on startup

**Given** a `ClickHouseSink` with a connected client
**When** `ensure_table()` is called
**Then** the `orderbook_events` table is created if it does not exist, using
`ReplacingMergeTree` engine with date partitioning and 90-day TTL

### Flushes on batch size threshold

**Given** a `ClickHouseSink` with default batch size of 10,000
**When** the buffer reaches 10,000 events
**Then** all buffered events are inserted into ClickHouse and the buffer is
cleared

### Flushes on interval

**Given** a `ClickHouseSink` with default 1-second interval
**When** the interval timer fires and the buffer is non-empty
**Then** buffered events are batch-inserted into ClickHouse

### Converts events to ClickHouseRow

**Given** an `OrderbookEvent`
**When** converted to `ClickHouseRow`
**Then** `event_type` becomes the string `"Snapshot"`, `"Delta"`, or `"Trade"`;
`side` becomes `Some("Bid")`, `Some("Ask")`, or `None`; and `price`/`size`
use raw integer values

### Flushes remaining events on channel close

**Given** a `ClickHouseSink` with buffered events
**When** the input channel is closed
**Then** remaining events are flushed before the task returns `Ok(())`
