# pb-wal

Embedded write-ahead log with append-only segments for durable event streaming
between the `ingest` and `serve` processes.

## Key Types

| Type | Description |
|------|-------------|
| `WalWriter` | Appends records to the active segment, rotates on size threshold, seals completed segments. |
| `WalReader` | Tails across segments with independent consumer position tracking. Resumes from committed offset on restart. |
| `WalConfig` | Segment size, base directory, max retained segments, max consumer lag bytes (for backpressure pruning). |
| `WalError` | Error type for WAL operations (IO, CRC, codec). |
| `codec::encode` / `codec::decode` | Version-prefixed bincode serialization for `PersistedRecord`. |

## Segment Layout

```text
┌───────────┬───────────┬──────────────────┐
│ len: u32  │ crc: u32  │ payload: [u8]    │  ← record frame (repeated)
└───────────┴───────────┴──────────────────┘

segment_00000000000000000000.wal   ← active segment
segment_00000000000000000001.wal   ← sealed segment
consumer_serve-live.pos    ← reader position file
```

Each record is framed with a 4-byte little-endian length and a 4-byte CRC32C
checksum. The reader skips corrupt records (CRC mismatch) and advances to the
next valid frame.

## Codec

The `codec` module adds a version byte prefix before bincode serialization:

```text
┌──────────────┬───────────────────────┐
│ version: u8  │ bincode payload       │
└──────────────┴───────────────────────┘
```

Currently version 1. Unknown versions produce a `WalError::Codec` error,
allowing forward-compatible schema evolution.

## Data Flow

```text
Dispatcher (pb-feed)
    │
    ▼
WalWriter ──▶ segment files ──▶ WalReader ──▶ LiveReadModel (pb-api)
```

In the `ingest` process, the dispatcher writes `PersistedRecord` events to the
WAL via `codec::encode`. In the `serve` process, a `WalReader` tails the WAL
and feeds decoded records into the live read model.

## Design Notes

- Segments are fixed-size append-only files. The writer rotates to a new segment
  when the active segment reaches the configured `segment_size` threshold.
- Multiple consumers can independently tail the same WAL with separate position
  files (`consumer_{name}.pos`). Each consumer commits its read position to disk
  via `WalReader::commit_position()` and resumes from there on restart.
- **Pruning**: `WalWriter::prune()` removes sealed segments that all registered
  consumers have advanced past. The active segment is never pruned.
- **Backpressure pruning**: `WalWriter::prune_with_backpressure()` retains at
  least `max_consumer_lag_bytes` worth of segments so new replicas have a window
  to hydrate before old segments disappear.
- **Gap detection**: `WalReader::needs_resync()` returns `true` if the reader's
  committed position references a segment that has been pruned, indicating the
  consumer should re-hydrate from a checkpoint.
- **Lag tracking**: `WalReader::lag_bytes()` returns the byte distance between
  the reader's current position and the latest data on disk.
- The codec version byte allows future record format changes without breaking
  existing segments.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| Frame format or CRC algorithm | `docs/architecture.md` WAL section |
| Codec version or serialization format | `pb-api` hydration, `pb-bin` ingest/serve commands |
| Config keys changed | `config/default.toml` `[wal]` section, `docs/operations.md` |
| Segment naming or layout | `docs/operations.md` data layout section |
| Changes affect WAL semantics | Check the active OpenSpec change under `openspec/changes/` |
