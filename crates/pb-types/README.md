# pb-types

Shared fixed-point, wire, and persisted event types for the poly-book workspace.
Every other crate depends on pb-types — it defines the foundational vocabulary
for prices, sizes, identifiers, and the six event datasets that flow through
the system.

## Key Types

| Type | Description |
|------|-------------|
| `FixedPrice(u32)` | Price scaled by 10,000 (4 decimal places). Never use `f64`. See [ADR-0001](../../docs/adr/0001-fixed-point-arithmetic.md). |
| `FixedSize(u64)` | Size scaled by 1,000,000 (6 decimal places). |
| `AssetId` | Typed newtype wrapping a string identifier for a market asset. |
| `Sequence` | Monotonically increasing event sequence number. |
| `SlugRegistry` | Maps condition IDs to human-readable market slugs. |
| `PersistedRecord` | Enum dispatching to the six event datasets below. |

## Persisted Record Model

Six event datasets, each with its own Parquet schema and ClickHouse table:

| Dataset | Purpose |
|---------|---------|
| `BookEvent` | L2 orderbook deltas and snapshots from the venue WebSocket. |
| `TradeEvent` | Matched trades with a `TradeFidelity` label. |
| `IngestEvent` | Feed lifecycle events: connect, disconnect, reconnect. |
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

- Wire types borrow from raw buffers (`&'a str`) for zero-copy deserialization
  on the ingest hot path. See [ADR-0004](../../docs/adr/0004-zero-copy-deserialization.md).
- `PersistedRecord` is the single enum that splits into all six datasets. Adding
  a new dataset means adding a variant here, plus corresponding schema and writer
  in pb-store and reader in pb-replay.
- `proptest` suites verify fixed-point roundtrip, ordering, and serde consistency.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New event dataset added | `pb-store` schema + writer, `pb-replay` reader, `docs/architecture.md` persisted record table |
| `FixedPrice`/`FixedSize` scaling changed | [ADR-0001](../../docs/adr/0001-fixed-point-arithmetic.md), `CLAUDE.md` conventions |
| New newtype added | `CLAUDE.md` crate summary |
| Wire type shape changed | `pb-feed` dispatcher deserialization |
| `PersistedRecord` variant added | `pb-store` `schema_for_record` + `records_to_record_batch`, `pb-replay` reader |
| Changes affect API surface or storage schema | Check the active OpenSpec change under `openspec/changes/` |
