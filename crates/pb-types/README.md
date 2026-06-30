# pb-types

Shared fixed-point, wire, and persisted event types for the poly-book workspace.
Every other crate depends on pb-types — it defines the foundational vocabulary
for prices, sizes, identifiers, and the six event datasets that flow through
the system.

## Key Types

| Type | Description |
|------|-------------|
| `FixedPrice` | Price scaled by 10,000 (4 decimal places); private `u32` field. `const fn` on `raw()`, `is_zero()`. Never use `f64`. See [ADR-0001](../../docs/adr/0001-fixed-point-arithmetic.md). |
| `FixedSize` | Size scaled by 1,000,000 (6 decimal places); private `u64` field. `const fn` on `new()`, `raw()`, `is_zero()`. |
| `AssetId` | Typed newtype wrapping a string identifier for a market asset. Provides `storage_key()` for safe object-store filenames. |
| `Sequence` | Monotonically increasing event sequence number. `const fn` constructors and accessors. |
| `SlugRegistry` | Maps condition IDs to human-readable market slugs. |
| `PersistedRecord` | Enum dispatching to the six event datasets below. |
| `time::normalize_to_micros` / `parse_to_micros` | The single timestamp-unit converter: classifies a raw value as s/ms/µs/ns by magnitude and returns microseconds (0 preserved as the unknown sentinel). Used by both the dispatcher and REST backfill so they never diverge. |

## Persisted Record Model

Six event datasets, each with its own Parquet schema and ClickHouse table:

| Dataset | Purpose |
|---------|---------|
| `BookEvent` | L2 orderbook deltas and snapshots from the venue WebSocket. |
| `TradeEvent` | Matched trades with a `TradeFidelity` label. |
| `IngestEvent` | Feed lifecycle events plus continuity boundaries such as `source_reset`. |
| `BookCheckpoint` | Full book state captured via periodic REST snapshots. |
| `ReplayValidation` | REST-vs-replay comparison results from the replay engine. |
| `ExecutionEvent` | Order lifecycle state changes (fill, cancel, etc.). |

## Dependents

pb-types is a leaf crate with no internal workspace dependencies. All other
crates depend on it:

```text
pb-types ◄── pb-feed    (wire deserialization)
         ◄── pb-book    (FixedPrice, FixedSize, Side)
         ◄── pb-store   (Arrow schemas, record batches)
         ◄── pb-replay  (event reading, replay results)
         ◄── pb-wal     (codec serialization of PersistedRecord)
         ◄── pb-service (domain service types)
         ◄── pb-api     (DTOs, response types)
         ◄── pb-grpc    (gRPC message conversion)
         ◄── pb-bin     (CLI arg types)
```

## Design Notes

- **Zero-alloc serde**: `FixedPrice` and `FixedSize` serialize via `itoa` +
  stack buffers and deserialize via a custom `Visitor`, avoiding heap allocation
  on the hot path. Dependency: `itoa = "1"`.
- **Exact decimal parsing**: `TryFrom<&str>` (used by serde and the WAL codec)
  parses with integer arithmetic, not `f64`, so it is exact across the full
  `u64` size range — no precision loss above 2^53 and no silent saturation.
  Excess fractional precision (more than 4 price / 6 size decimals) and overflow
  are rejected, not rounded or clamped.
- **Invariant safety**: the `FixedPrice`/`FixedSize` inner fields are private;
  construct via `new`, `from_f64`, or `TryFrom<&str>` so the price range
  invariant cannot be bypassed (an out-of-range value would serialize but fail
  to deserialize).
- `#[inline]` on all hot-path accessors for `FixedPrice`, `FixedSize`, and
  `Sequence`.
- Wire types borrow from raw buffers (`&'a str`) for zero-copy deserialization
  on the ingest hot path. See [ADR-0004](../../docs/adr/0004-zero-copy-deserialization.md).
- `PersistedRecord` is the single enum that splits into all six datasets. Adding
  a new dataset means adding a variant here, plus corresponding schema and writer
  in pb-store and reader in pb-replay.
- `storage_key_for()` percent-encodes asset partitions for Parquet object names,
  preserving common safe ASCII and encoding `/`, `\`, `%`, control bytes, and
  non-ASCII as `%XX`.
- `storage_file_asset_key()` / `storage_file_matches_asset()` parse the current
  `{asset}_{first_ts}_{content_hash}_{len}.parquet` suffix from the right, so
  callers compare the exact asset component even when the encoded asset key
  contains underscores.
- `proptest` suites verify fixed-point roundtrip, ordering, and serde consistency.
- 150 tests covering boundary conditions, serde round-trips, proptest invariants,
  and all persisted record types.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New event dataset added | `pb-store` schema + writer, `pb-replay` reader, `docs/architecture.md` persisted record table |
| `FixedPrice`/`FixedSize` scaling changed | [ADR-0001](../../docs/adr/0001-fixed-point-arithmetic.md), `CLAUDE.md` conventions |
| New newtype added | `CLAUDE.md` crate summary |
| Wire type shape changed | `pb-feed` dispatcher deserialization |
| `PersistedRecord` variant added | `pb-store` `schema_for_record` + `records_to_record_batch`, `pb-replay` reader |
| Changes affect API surface or storage schema | Check the active OpenSpec change under `openspec/changes/` |
