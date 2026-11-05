# pb-store

Parquet storage sink and writer. Receives `PersistedRecord` events from the
ingest channel and writes them to durable storage in split-dataset format.

## Key Types

| Type | Description |
|------|-------------|
| `ParquetSink` | Batches events and flushes to Parquet files every 5 minutes with Zstd level-3 compression and `DELTA_BINARY_PACKED` encoding for timestamp, price, size, and sequence columns. Pre-allocates a 256 KB byte buffer to avoid repeated heap growth. Uses `object_store` (local FS / S3). |
| `ParquetRecordWriter` | Low-level writer for individual Parquet files. |
| `RecoveryCoverage` | Validated inclusive WAL timestamp span used to prove that an hourly receive-time partition is complete. |
| `RecoveryReport` | Published partition/record counts plus any post-publication cleanup failures. |
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
    └──▶ ParquetSink ──▶ Parquet files (5-min flush, Zstd)
                              │
                              ▼
                         pb-replay (ParquetReader)
```

## Design Notes

- Storage uses the `object_store` trait for filesystem abstraction. The current
  workspace enables local disk and S3 (including S3-compatible endpoints such as
  MinIO); GCS support is not compiled in.
- Parquet files are partitioned by event type and time window (`YYYY/MM/DD/HH`).
  The flush interval (5 minutes) balances write amplification against data
  freshness for replay. Records whose timestamp is outside a wide plausible band
  (or not representable as a datetime) are routed to a dedicated
  `invalid_timestamp` partition with a warning, instead of being silently misfiled
  into the 1970-01-01 partition by `unwrap_or_default()`.
- Parquet object names are `{asset_key}_{first_ts}_{content_hash}_{len}.parquet`.
  `asset_key` is a percent-encoded asset partition, so malformed asset IDs cannot
  inject path separators or object-key delimiters. The content-hash +
  byte-length suffix means two batches that land in the same
  (asset, hour) bucket with the same first-record timestamp cannot silently
  overwrite each other; identical content maps to the same name (idempotent
  retry). The byte length is appended so a (vanishingly unlikely) 64-bit hash
  collision between two *different* batches would also need identical lengths to
  collide — at no extra cost, since identical content has identical length. The
  asset component is parsed from the right-hand fixed suffix fields when reading
  or deleting, so an asset key containing `_` cannot be mistaken for another
  asset's prefix.
- `write_batch_replacing` accepts only receive-time-partitioned book, trade, and
  ingest records whose UTC hour is fully contained in an explicit, validated WAL
  coverage span. It first writes an immutable object under `_recovery_objects`
  and publishes a manifest pointing at that staged view. It then promotes the
  same bytes into the normal dataset/hour tree, publishes the final manifest, and
  cleans superseded normal/staged objects. Manifest-aware readers therefore see
  a complete partition across crashes at either publication phase. A clean run
  leaves the active object in the normal tree for direct Parquet consumers;
  unresolved cleanup is reported and direct scans remain unsafe until a retry.
  Boundary-hour and unprovable dataset replacement is refused. It is the storage
  half of the offline `reconcile` command; every process that writes the same
  Parquet prefix must be stopped during the manifest cut.
- Parquet encoding uses explicit Zstd compression at level 3 and `DELTA_BINARY_PACKED`
  encoding on timestamp, price, size, and sequence columns for better compression ratios.
- A pre-allocated 256 KB byte buffer avoids repeated heap allocation during Parquet writes.
- Date formatting uses `Datelike`/`Timelike` trait methods instead of `strftime` for efficiency.
- The sink flushes with bounded exponential-backoff retries (5 attempts), keeping
  the buffer intact across retries, so a single transient write failure no
  longer drops the batch or instantly tears down the ingest pipeline.
- On graceful shutdown (cancellation) the sink drains any records still queued in
  its mpsc channel — bounded by a 10s deadline — before the final flush, so a
  clean stop does not abandon records the upstream already enqueued.
- WAL→storage reconciliation is provided by `write_batch_replacing` / the
  `reconcile` command with strict full-hour coverage and manifest publication.

## Tests

Unit tests cover schema validation, record batch conversion,
`ParquetRecordWriter` lifecycle, and `ParquetSink` lifecycle.
