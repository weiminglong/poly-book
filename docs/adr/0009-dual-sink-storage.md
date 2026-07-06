# ADR-0009: Dual-Sink Storage — Parquet Cold, ClickHouse Warm

## Status
Accepted

## Date
2026-07-06 (records a decision implemented incrementally through March 2026)

## Context
Persisted market data serves two read patterns with conflicting needs:

1. **Replay truth and audit**: reconstructing the exact book state at an
   arbitrary timestamp, validating replay against venue snapshots, and
   rebuilding derived state after a crash. This needs an immutable, portable,
   self-contained format whose correctness does not depend on a running
   server.
2. **Interactive inspection**: the workstation's replay/integrity/execution
   routes and the SQL query workbench, where an analyst expects sub-second
   answers over time ranges and aggregations. Scanning raw files is O(total
   data) per query.

A single store optimizes one pattern at the expense of the other.

## Decision
Run two storage sinks side by side off the same `PersistedRecord` channel,
with an explicit division of authority:

- **`ParquetSink` — cold, source of truth.** 5-minute flush windows, Zstd
  level-3 compression, `DELTA_BINARY_PACKED` on timestamp/price/size/sequence
  columns. Written through the `object_store` trait, so local disk, S3, and
  GCS are interchangeable without code changes. Object names embed a content
  hash + byte length, making retries idempotent and silent overwrites
  impossible. Replay correctness (`pb-replay`) and crash recovery
  (`reconcile`) read from Parquet.
- **`ClickHouseSink` — warm, interactive.** 1-second batches (or 10,000
  rows), `MergeTree` tables partitioned by date with composite ORDER BY keys.
  Per-batch deduplication tokens make re-inserts after partial failures
  idempotent. Serves the interactive historical routes and the SQL workbench
  when `api.historical_backend = "clickhouse"`.

Both sinks share the same Arrow schema functions and the same
`PB_SCHEMA_VERSION` constant, so the two representations of a dataset cannot
drift structurally without a deliberate version bump.

ClickHouse is deliberately optional: if it is configured but unreachable at
startup, the serving layer probes with a 3-second timeout and falls back to
the Parquet backend with a warning.

## Alternatives Considered
- **ClickHouse only**: couples replay correctness and audit to a running
  server and its storage format; loses object-store portability and the
  ability to inspect history with nothing but files and DuckDB. Rejected —
  replay truth should not depend on a database being up.
- **Parquet only**: interactive queries degrade to file scans; the query
  workbench and sub-second integrity/execution lookups become impractical as
  data grows. DuckDB over Parquet works for local, single-user inspection but
  is not the serving path. Rejected for the interactive surface.
- **One store with tiering (e.g. ClickHouse with S3-backed cold parts)**:
  keeps one write path, but the cold tier is then in a proprietary part
  format, and the failure domain is still the single server. Rejected for the
  same source-of-truth reason as ClickHouse-only.

## Consequences
- **Each read pattern gets the right engine**: replay and reconciliation work
  from immutable portable files; the workstation gets indexed columnar SQL.
- **Dual write paths are a real cost**: two flush cadences, two retry paths,
  and two schema representations to keep in step. Mitigated by shared schema
  functions, a single version constant, and both sinks flushing with bounded
  exponential-backoff retries.
- **The two stores can diverge** (a sink outage, a partial batch). This is
  accepted and managed rather than prevented: Parquet is authoritative, three
  cross-backend equivalence tests (replay, integrity, execution in
  `tests/integration/cross_backend_service.rs`) verify both backends give the
  same answers over the same records, and `reconcile` rebuilds a lost Parquet
  window from the WAL. The equivalence tests are Docker-backed and
  `#[ignore]`d — they run locally, not in CI (TESTING.md row 7).
- **Storage lag never blocks durability**: sinks consume from their own
  bounded fan-out channels and may fall behind; the WAL (ADR-0008) remains
  the unconditionally-blocking consumer, so a slow sink degrades freshness,
  not capture.
- **Operational footprint**: ClickHouse is one more service to run — but only
  for deployments that want the warm path, since the Parquet fallback keeps
  every route functional without it.
