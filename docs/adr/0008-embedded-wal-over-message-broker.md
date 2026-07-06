# ADR-0008: Embedded Single-Writer WAL Over an External Message Broker

## Status
Accepted

## Date
2026-07-06 (records a decision implemented with the ingest/serve split in
March 2026)

## Context
The ingest and serve runtimes are separate processes (ADR-0010) that need a
durable, ordered handoff for `PersistedRecord` events. The handoff sits on the
durability-critical path: the ingest loop appends every record to it *before*
fanning out to storage sinks, and it is the only consumer allowed to apply
backpressure to the feed. Requirements:

- append latency small enough to sit inline on the ingest hot path
- crash recovery: a torn write must never corrupt or desync the log
- multiple independent consumers with their own resume positions
- bounded disk usage on a single workstation host
- exactly one writer at a time, with safe failover to a standby

The deployment target is a single host. There is no requirement for
cross-host replication or a networked consumer.

## Decision
Build a purpose-fit embedded WAL (`pb-wal`) instead of running Kafka,
Redpanda, NATS JetStream, or Redis Streams:

- Append-only segment files with length-prefix + CRC32C framing; the CRC
  covers the length field as well as the payload, so a corrupted length is a
  CRC mismatch, not a misparse of the rest of the segment.
- `BufWriter`-wrapped standard file I/O. The original design sketch used mmap,
  but the shipped implementation uses plain file operations (`memmap2` was
  removed): simpler crash semantics, no cross-OS mmap behavior to reason about.
- Explicit durability cadence: `flush()` every `wal.flush_interval_ms`
  (default 20 ms, for tail-reader visibility) and `fdatasync` every
  `wal.sync_interval_ms` (default 200 ms, bounding the OS-crash loss window).
- Single-writer mutual exclusion via an exclusive advisory `flock` on
  `<base>/.wal.lock`. A second writer fails fast with `WalError::WriterLocked`;
  the lock auto-releases on process exit, so the same mechanism is the
  writer-failover path (covered by the
  `standby_writer_takes_over_shared_wal_after_primary_exit` test).
- Per-consumer position files with atomic temp-file + fsync + rename commits.
- Retention bounded by `prune`/`prune_with_backpressure`: sealed segments are
  reclaimed once all consumers pass them, with a byte budget for lagging
  replicas and a hard `max_segments` cap so the WAL cannot fill the disk.
- A version-byte codec (`pb_wal::codec`) in front of bincode, so format
  evolution fails closed instead of misreading bytes.

## Alternatives Considered
- **Kafka / Redpanda / NATS JetStream**: durable, replicated, multi-consumer —
  and disproportionate for a single-host system. Each adds a broker to deploy,
  monitor, upgrade, and secure; a network hop plus protocol serialization on
  the hot append path; and a second failure domain between ingest and serve.
  Rejected because the system gains none of the multi-host benefits it would
  be paying for.
- **`redb` (embedded KV store)**: ACID transactions are heavier machinery than
  an append-only log needs, and it has no native segment rotation or consumer
  tailing model. Rejected.
- **`commitlog` crate**: closest fit conceptually, but unmaintained (last
  release 2021). An owned implementation is small enough to audit and gives
  full control over framing, fsync cadence, and pruning policy. Rejected.
- **mmap-based segments** (the original design): zero-copy reads, but
  platform-dependent semantics around truncation and flushing, and harder
  torn-write reasoning. Superseded by buffered file I/O during implementation.

## Consequences
- **Latency**: the Criterion harness (`pb-wal/benches/wal_append.rs`) measures
  `codec::encode` at ~80 ns and steady-state append+flush at ~260 ns/record
  with fsync amortized — comfortably inline on the ingest path. Per-record
  `fdatasync` is ms-scale, which is exactly why syncing is batched on an
  interval rather than per append.
- **No broker operations**: nothing extra to deploy or page on; the WAL is a
  directory. Backup is file copy; inspection is a CLI away.
- **Crash safety is owned code**: torn-tail truncation on reopen, CRC skipping,
  and conservative pruning on unparseable position files all had to be built
  and tested here rather than inherited from a broker. This is the largest
  cost of the decision, paid for with fuzzing (`fuzz_wal_corruption`,
  `fuzz_codec_decode`), property tests, and unit tests (see TESTING.md rows
  4–5).
- **No multi-host replication**: consumers must share a filesystem with the
  writer. Scaling reads across hosts would require a network transport layer —
  an explicitly deferred, separate change.
- **Filesystem coupling**: ingest and serve are pinned to the same host (or a
  shared volume), and both binaries must agree on the codec version
  (`pb_wal::codec::CURRENT_VERSION`); the mismatch procedure is documented in
  the runbook and docs/operations.md.
- **Bounded retention means the WAL is a tail, not a store**: long-term truth
  lives in Parquet (ADR-0009). A consumer that lags past the retention window
  must re-hydrate from a checkpoint (`needs_resync()`).
