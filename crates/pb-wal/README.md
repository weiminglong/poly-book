# pb-wal

Embedded write-ahead log with append-only segments for durable event streaming
between the `ingest` and `serve` processes.

## Key Types

| Type | Description |
|------|-------------|
| `WalWriter` | Appends records to the active segment, rotates on size threshold, seals completed segments. |
| `WalReader` | Tails across segments with independent consumer position tracking. Resumes from committed offset on restart. |
| `WalPosition` | Typed `(segment_id, offset)` handoff point for resuming a reader without consulting a consumer position file. |
| `WalMaintenanceGuard` | Exclusive writer lease for offline maintenance; refuses acquisition while ingest owns the WAL. |
| `WalConfig` | Segment size, base directory, max retained segments, max consumer lag bytes, and live reader position commit interval. |
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
checksum. The CRC covers **both the length field and the payload**, so a
corrupted length is detected as a CRC mismatch rather than being trusted and
misparsing the rest of the segment. The reader skips corrupt records (CRC
mismatch) and advances to the next valid frame.

## Codec

The `codec` module adds a version byte prefix before bincode serialization:

```text
┌──────────────┬───────────────────────┐
│ version: u8  │ bincode payload       │
└──────────────┴───────────────────────┘
```

Currently version 2 (`codec::CURRENT_VERSION`). Unknown versions produce a
`WalError::Codec` error; version 1 (which predates the provenance ingest
ordinal) is explicitly rejected with a drain-the-WAL hint, because positional
bincode would silently misparse the changed field count.

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

- **Append-only file I/O** — segments use standard file I/O (not mmap).
  `WalWriter` wraps the active file in a `BufWriter` with a 64 KB buffer,
  reducing write syscalls by ~3x.
- **Efficient framing**: the 8-byte frame header (4-byte length + 4-byte CRC32C)
  is stack-assembled and written in a single call instead of two separate writes.
- **Steady-state durability cadence**: the `ingest`/`auto-ingest` event loop
  drives `WalWriter::flush()` on `wal.flush_interval_ms` (default 20 ms, for
  tail-reader visibility) and `WalWriter::sync()` (`fdatasync`) on
  `wal.sync_interval_ms` (default 200 ms, bounding the OS-crash data-loss
  window). A failed open/append/flush/sync is fatal: ingest exits non-zero so a
  supervisor restarts it, rather than silently running without durability.
- **Durable rotation**: rotating to a new segment first `fdatasync`s the sealed
  segment, then `fsync`s the directory so the new segment's directory entry
  survives a power loss. The directory is also fsynced after the first segment is
  created.
- **Crash recovery on reopen**: `WalWriter::open` scans the last segment
  frame-by-frame and truncates a torn (partial) or zero-filled tail back to the
  last valid frame before resuming appends, so post-restart records are never
  desynced or silently lost.
- **No permanent reader stall**: a `WalReader` opened on an empty directory
  (serve started before ingest) or racing a prune reloads and recovers instead
  of returning `None` forever.
- **Incremental tailing**: a caught-up reader stats the active segment and reads
  only newly appended bytes, never re-reading the whole segment per poll. If the
  active segment is observed *shorter* than the reader's cached copy — a writer's
  torn-tail recovery truncating it in a multi-process setup — the reader reloads
  the whole segment instead of splicing the new suffix onto a now-stale prefix,
  and a reader parked at an incomplete trailing frame keeps its offset at that
  frame's start so it resumes correctly once the writer completes or rewrites it.
- **Conservative prune on a corrupt position file**: if a consumer position file
  exists but cannot be parsed (e.g. a partial write), pruning keeps every segment
  (as it does for a missing file) and logs a warning, rather than treating the
  consumer as fully caught up and deleting segments it still needs.
- **Single-buffer codec encode**: `codec::encode` uses `serialize_into` to write
  directly into the frame buffer, avoiding an intermediate allocation.
- Segments are fixed-size append-only files. The writer rotates to a new segment
  when the active segment reaches the configured `segment_size` threshold.
- Multiple consumers can independently tail the same WAL with separate position
  files (`consumer_{name}.pos`). Each consumer commits its read position to disk
  via `WalReader::commit_position()` and resumes from there on restart.
- `WalReader::open_at()` allows a runtime to hand off directly from hydration to
  live tailing without replaying WAL records that were already applied during
  startup.
- `WalReader::open_from_start()` plus `next_strict()` provides a fail-closed
  recovery scan over every retained segment. Unlike the live reader, it returns
  CRC, truncation, and internal segment-gap errors instead of skipping damage.
- `WalMaintenanceGuard` reuses the writer's advisory lease so destructive
  maintenance cannot race a live ingest writer.
- **Atomic position writes**: position files are written to a temp file first, then
  fsynced and renamed into place, and the parent directory is fsynced afterward,
  preventing partial reads or lost renames on crash.
- **Cached segment list**: `WalReader` caches the segment directory listing,
  avoiding repeated directory re-scans during tailing.
- **Pruning**: `WalWriter::prune()` removes sealed segments that all registered
  consumers have advanced past. The active segment is never pruned.
- **Backpressure pruning**: `WalWriter::prune_with_backpressure()` retains at
  least `max_consumer_lag_bytes` worth of segments so new replicas have a window
  to hydrate before old segments disappear, while also enforcing a hard
  `max_segments` count cap so the WAL cannot grow without bound when the byte
  budget is generous. When a lagging consumer blocks pruning below the cap, a
  needs-resync warning is logged instead of letting the disk fill.
- **Single-writer mutual exclusion / writer leasing**: `WalWriter::open` acquires
  an exclusive advisory `flock` on `<base>/.wal.lock`; a second writer on the same
  directory fails fast with `WalError::WriterLocked` rather than interleaving
  appends. The lock auto-releases on process exit (crash-safe, no stale lock file).
  This doubles as the writer-failover mechanism: after the primary exits, a standby
  acquires the released lease, reads everything the primary durably synced, and
  resumes appending with no data loss across the handoff
  (`standby_writer_takes_over_shared_wal_after_primary_exit`). RTO/RPO targets per
  failure mode are documented in `docs/operations.md` ("Failover & Recovery").
- **Gap detection**: `WalReader::needs_resync()` returns `true` if the reader's
  committed position references a segment that has been pruned, indicating the
  consumer should re-hydrate from a checkpoint.
- **Lag tracking**: `WalReader::lag_bytes()` returns the byte distance between
  the reader's current position and the latest data on disk.
- **Tunable commit cadence**: serve-side live readers commit their position on a
  configurable interval via `wal.position_commit_interval_ms`, letting operators
  trade off fsync frequency against restart replay distance.
- The codec version byte allows future record format changes without breaking
  existing segments.
- `memmap2` dependency removed — all I/O is via standard file operations.
- Tests cover CRC/length corruption detection, truncated and zero-filled tail
  recovery on reopen, incremental tailing, empty-dir reader recovery, codec
  round-trips, segment rotation, reader position persistence, and pruner safety.
- **Benchmarks** (`cargo bench -p pb-wal`, `benches/wal_append.rs`) measure the
  durability hot path: `codec::encode` (~80 ns), steady-state `append+flush`
  (~260 ns/record, fsync amortized), and per-record `fdatasync` (ms-scale —
  ~10⁴× the amortized append, which is why fsync is batched on
  `sync_interval_ms`). See `docs/latency.md` for the full pipeline budget.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| Frame format or CRC algorithm | `docs/architecture.md` WAL section |
| Codec version or serialization format | `pb-api` hydration, `pb-bin` ingest/serve commands |
| Config keys changed | `config/default.toml` `[wal]` section, `docs/operations.md` |
| Segment naming or layout | `docs/operations.md` data layout section |
| Changes affect WAL semantics | Check the active OpenSpec change under `openspec/changes/` |
