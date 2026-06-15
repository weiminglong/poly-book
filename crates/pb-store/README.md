# pb-store

Parquet and ClickHouse storage sinks. Receives `PersistedRecord` events from the
ingest channel and writes them to durable storage in split-dataset format.

## Key Types

| Type | Description |
|------|-------------|
| `ParquetSink` | Batches events and flushes to Parquet files every 5 minutes with Zstd level-3 compression and `DELTA_BINARY_PACKED` encoding for timestamp, price, size, and sequence columns. Pre-allocates a 256 KB byte buffer to avoid repeated heap growth. Uses `object_store` (local FS / S3 / GCS). |
| `ClickHouseSink` | Batches events and inserts to ClickHouse every 1 second (or when batch reaches 10,000 rows). Creates insert handles conditionally — only for record types that have data in the current batch. Uses `MergeTree` engine partitioned by date. |
| `ParquetRecordWriter` | Low-level writer for individual Parquet files. |
| `ClickHouseRecordWriter` | Low-level writer for ClickHouse batch inserts. |
| `StoreError` | Error type for storage operations. |

## Schema Functions

Each event dataset has a pair of functions:

- `*_schema()` — returns the Arrow schema (e.g., `book_event_schema()`)
- `*_refs_to_record_batch()` — converts event slices to Arrow `RecordBatch`

Also: `schema_for_record()` dispatches by `PersistedRecord` variant, and
`records_to_record_batch()` converts mixed record slices.

## Data Flow

```text
PersistedRecord channel
    │
    ├──▶ ParquetSink ──▶ Parquet files (5-min flush, Zstd)
    │                         │
    │                         ▼
    │                    pb-replay (ParquetReader)
    │
    └──▶ ClickHouseSink ──▶ ClickHouse tables (1s batch)
                              │
                              ▼
                         pb-replay (ClickHouseReader)
```

## Design Notes

- Storage uses the `object_store` trait for filesystem abstraction, supporting
  local disk, S3, and GCS without code changes.
- Parquet files are partitioned by event type and time window. The flush interval
  (5 minutes) balances write amplification against data freshness for replay.
- Parquet encoding uses explicit Zstd compression at level 3 and `DELTA_BINARY_PACKED`
  encoding on timestamp, price, size, and sequence columns for better compression ratios.
- A pre-allocated 256 KB byte buffer avoids repeated heap allocation during Parquet writes.
- Date formatting uses `Datelike`/`Timelike` trait methods instead of `strftime` for efficiency.
- ClickHouse uses `MergeTree` engine with date partitioning and composite ORDER BY
  keys for efficient range queries. Tables are created via `ensure_tables()`.
- ClickHouse insert handles are created conditionally — only for record types that
  actually have data in the current batch, avoiding empty inserts.
- Both sinks flush with bounded exponential-backoff retries (5 attempts), keeping
  the buffer intact across retries, so a single transient insert/write failure no
  longer drops the batch or instantly tears down the ingest pipeline. (Full sink
  isolation, non-zero exit on terminal failure, and WAL→storage reconciliation
  are tracked under remaining P1-STORE work.)

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New event dataset schema | `pb-types` event struct, `pb-replay` reader, `docs/architecture.md` persisted record table |
| Schema column added/removed | `pb-replay` reader, `pb-api` if column is exposed in routes |
| Flush interval or compression changed | `docs/operations.md`, `docs/latency.md` |
| Storage backend added | `docs/operations.md` config section |
| Storage config keys changed | `config/default.toml` `[storage]` section, `docs/operations.md` |
| Changes affect storage layout or schema | Check the active OpenSpec change under `openspec/changes/` |

## Tests

29 tests covering schema validation, record batch conversion, `ParquetRecordWriter`
lifecycle, and `ParquetSink` lifecycle.
