## Context

The current `serve-api` runtime is a monolithic process that owns venue
WebSocket connections, message normalization, book state management, broadcast
fan-out, and browser-facing HTTP/WS serving. This was shipped intentionally as
Phase 3 — fast path to a working workstation. The architecture has concrete
limitations:

- `Arc<RwLock<LiveState>>` causes writer-blocks-readers contention on every
  book delta
- A single `broadcast::Sender<BookUpdateMessage>` copies every asset's update
  to every WS subscriber, filtered per-subscriber in a loop
- No durable state: process restart means seconds of empty books while the feed
  rebuilds from the exchange
- No crash recovery: `mpsc` channels are ephemeral, fan-out is ad-hoc
  forwarding tasks
- Venue connectivity and browser serving are coupled in one process
- Domain logic is interleaved with axum extractors, making future gRPC
  support require duplication

The workstation platform design doc already defines the target clean-slate
topology (ingest runtime, serve runtime, durable update bus). This change
implements that topology.

## Goals / Non-Goals

**Goals:**

- Durable ordered event log as the spine between ingest and serving, enabling
  crash recovery, multi-consumer tailing, and process separation
- Lock-free read path: serving HTTP/WS requests without contending with the
  book update writer
- Sub-100ms serve replica cold start via checkpoint hydration + log tail replay
- Per-asset broadcast partitioning to eliminate O(assets x subscribers) fan-out
- Independently deployable ingest and serve runtimes communicating through the
  event log
- Transport-neutral service layer that can back HTTP/WS and future gRPC without
  duplicating domain logic
- ClickHouse as the interactive serving backend for historical queries, Parquet
  retained as audit and replay-truth source

**Non-Goals:**

- gRPC implementation (service layer enables it; shipping it is a later change)
- Multi-node distributed deployment (event log is local, not networked)
- Live order routing, risk controls, or any mutating actions
- Frontend changes (all API contracts remain identical)
- Replacing Parquet as the source of truth for replay correctness

## Decisions

### D1: Embedded mmap WAL over external message broker

**Decision**: Build a `pb-wal` crate with mmap'd segment files, CRC32 framing,
and per-consumer position tracking. Do not use Kafka, NATS, or Redis Streams.

**Alternatives considered**:
- **Kafka/NATS JetStream**: Adds operational dependency, network hop on the hot
  path, and deployment complexity disproportionate to a single-node system
- **`redb` embedded KV**: ACID transactions are heavier than needed for an
  append-only log; no native segment rotation or consumer tailing
- **`commitlog` crate**: Viable but unmaintained (last release 2021); building
  a purpose-fit WAL is ~500 lines and gives full control over framing and
  pruning

**Rationale**: The system is single-node. An embedded log gives sub-microsecond
append latency, zero-copy reads via mmap, and no external dependencies. The WAL
is append-only with fixed-size segments, making it simple to implement correctly.

**Design**:

```text
pb-wal/
  src/
    segment.rs     -- mmap'd fixed-size segment (default 64 MB)
    writer.rs      -- append with CRC32C + length-prefix framing
    reader.rs      -- consumer tailing with position tracking
    pruner.rs      -- segment cleanup after all consumers advance
    lib.rs         -- WalWriter, WalReader, WalConfig
```

Record framing:

```text
┌──────────┬──────────┬──────────────────┬──────────┐
│ len: u32 │ crc: u32 │ payload: [u8]    │ padding  │
└──────────┴──────────┴──────────────────┘──────────┘
```

Segment rotation: when the active segment exceeds `segment_size`, seal it and
open a new one. Sealed segments are read-only mmap'd. Pruning removes segments
where all registered consumers have advanced past the segment's end offset.

Payload serialization: `PersistedRecord` serialized via `bincode` (compact,
zero-copy-friendly, already in the ecosystem). JSON fallback for debugging.

### D2: watch-based read model over seqlock/double-buffer

**Decision**: Replace `Arc<RwLock<LiveState>>` with a single-writer task that
publishes `Arc<AssetBookState>` per asset via `tokio::sync::watch` channels.

**Alternatives considered**:
- **Seqlock + double-buffer**: True zero-allocation reads, sub-microsecond
  latency. Requires unsafe code, manual memory management, and careful
  alignment. Meaningful only at nanosecond-scale latency requirements.
- **`left-right` crate**: Lock-free reads via epoch-based reclamation.
  Adds complexity and a dependency for marginal benefit over `watch`.
- **Keep `RwLock`**: Simple, but readers queue behind writers under load.

**Rationale**: `watch` gives zero reader contention (readers borrow a reference
to the latest value without locking), consistent snapshots, and natural
batching (if the writer commits multiple deltas between reader polls, readers
see only the latest state). The implementation is ~50 lines of safe Rust.

**Structure**:

```text
BookProjector (single task)
  ├── tails event log
  ├── maintains per-asset L2Book (owned, no sharing)
  ├── on each delta: updates book, publishes Arc<AssetSnapshot> to watch
  └── on each snapshot: replaces book, publishes

Per-asset watch channels:
  FxHashMap<AssetId, watch::Sender<Arc<AssetSnapshot>>>

HTTP/WS handlers:
  watch::Receiver::borrow() → Arc<AssetSnapshot> (no lock, no copy)
```

### D3: Per-asset broadcast over global broadcast

**Decision**: Replace the single `broadcast::Sender<BookUpdateMessage>` with
`FxHashMap<AssetId, broadcast::Sender<BookUpdateMessage>>`.

**Rationale**: With N assets and M total subscribers, the current design does
N x M message clones. Per-asset partitioning reduces this to sum of per-asset
subscriber counts. WS handler subscribes to exactly one asset's channel,
eliminating the filter loop entirely.

The `BookProjector` creates broadcast channels lazily when a new asset becomes
active, and drops them when assets rotate out.

### D4: Checkpoint hydration with log position coordination

**Decision**: Extend `BookCheckpoint` records with the WAL offset at which the
checkpoint was taken. On serve startup, load the latest checkpoint per asset,
seek the WAL to that offset, and replay forward to head.

**Sequence**:

```text
1. Read latest checkpoint from Parquet (per-asset full book state + WAL offset)
2. Seek WAL reader to checkpoint's WAL offset
3. Replay events from offset → current head (apply deltas to hydrated books)
4. Switch to live tailing mode
5. Report readiness
```

**Rationale**: Checkpoints are already written periodically by `pb-store`. Adding
a WAL offset field is a single `u64`. The hydration window (events between last
checkpoint and current head) is bounded by checkpoint interval (currently 5 min),
so replay is fast.

### D5: Process separation via shared WAL directory

**Decision**: Ingest and serve runtimes run as separate processes sharing a WAL
directory on the filesystem. Ingest writes, serve reads. No network protocol
between them.

**Alternatives considered**:
- **Unix domain socket**: Adds a protocol layer, serialization overhead, and
  connection management
- **Shared memory ring buffer**: Maximum performance but requires careful
  synchronization primitives and limits to same-host deployment
- **Network protocol (gRPC/HTTP)**: Adds latency and operational complexity

**Rationale**: Filesystem-based WAL sharing is the simplest IPC mechanism that
provides durability. Both processes mmap the same files. The writer uses
`flock` or atomic rename for segment coordination. This naturally supports the
single-node deployment model without precluding a future network transport.

**Binary structure**:

```text
poly-book ingest   -- owns venue connectivity, writes to WAL + storage
poly-book serve    -- tails WAL, serves HTTP/WS, reads from storage
poly-book all      -- current monolith mode (backward compatible)
```

### D6: Service layer extraction

**Decision**: Create `pb-service` crate with transport-neutral service traits.
`pb-api` handlers become thin adapters that parse HTTP input, call the service,
and format HTTP output.

```rust
// pb-service/src/lib.rs
pub trait BookService: Send + Sync {
    async fn snapshot(&self, asset_id: &str, depth: usize)
        -> Result<BookSnapshot, ServiceError>;
    async fn feed_status(&self) -> Result<FeedStatus, ServiceError>;
    async fn active_assets(&self) -> Result<Vec<AssetSummary>, ServiceError>;
}

pub trait ReplayService: Send + Sync {
    async fn reconstruct(&self, params: ReconstructParams)
        -> Result<ReplayResult, ServiceError>;
}

pub trait IntegrityService: Send + Sync {
    async fn summary(&self, params: IntegrityParams)
        -> Result<IntegritySummary, ServiceError>;
}
```

**Rationale**: Current handlers mix HTTP parsing, domain logic, and response
formatting. Extracting the domain layer means gRPC handlers (future) share the
same logic path. It also makes services testable without HTTP.

### D7: ClickHouse for interactive historical reads

**Decision**: Route interactive replay, integrity, and execution queries through
ClickHouse. Parquet remains the audit and replay-truth source.

**Rationale**: Parquet scans are O(total data) for interactive queries.
ClickHouse provides indexed, columnar, concurrent-safe reads with sub-second
latency for the workstation's query patterns. The data is already being written
to ClickHouse via `ClickHouseSink`.

**Serving split**:

```text
Interactive workstation queries → ClickHouse
Audit, validation, recovery    → Parquet (canonical truth)
Local development / offline    → DuckDB over Parquet
```

## Risks / Trade-offs

**[Risk]** Custom WAL has bugs (data corruption, lost writes)
→ **Mitigation**: CRC32C on every record. Checksums verified on read. Fuzz test
the WAL with `cargo fuzz`. Property tests for crash-recovery scenarios. The WAL
is append-only, which eliminates most corruption vectors.

**[Risk]** mmap behavior differs across OS (Linux vs macOS)
→ **Mitigation**: Use `memmap2` crate which abstracts platform differences.
Segment size defaults to 64 MB (well within OS page cache). CI runs on both
Linux and macOS.

**[Risk]** Process separation adds operational complexity
→ **Mitigation**: Keep `poly-book all` as the monolith mode for development and
simple deployments. Process separation is opt-in for production.

**[Risk]** ClickHouse becomes a hard dependency for serving
→ **Mitigation**: Service layer uses trait-based readers. Parquet fallback
remains available. DuckDB adapter for local dev. The serving backend is
configurable, not hardcoded.

**[Risk]** `watch` channel drops intermediate states (reader sees only latest)
→ **Mitigation**: This is intentional. HTTP readers want the latest state, not
every intermediate delta. WS subscribers get per-update broadcasts from the
separate broadcast channel, not from the watch.

**[Trade-off]** Bincode serialization is not self-describing
→ **Acceptance**: WAL records are internal, not cross-service. Schema evolution
is handled by versioning the record format with a version byte prefix. Breaking
schema changes require WAL truncation (acceptable — WAL is ephemeral cache,
Parquet is truth).

**[Trade-off]** Filesystem-shared WAL limits to single-host deployment
→ **Acceptance**: This system is designed for single-node deployment. Multi-node
would require a network transport layer, which is a separate future change.

## Migration Plan

The migration is incremental. Each phase can be shipped and validated
independently.

1. **Phase 6.0**: Per-asset broadcast partitioning (small diff, no arch change)
2. **Phase 6.1**: `pb-wal` crate with embedded event log
3. **Phase 6.2**: `watch`-based read model replacing `RwLock` in `LiveReadModel`
4. **Phase 6.3**: Checkpoint hydration (WAL offset in checkpoints, startup replay)
5. **Phase 6.4**: Process separation (ingest/serve binaries, `all` compat mode)
6. **Phase 6.5**: `pb-service` extraction from `pb-api` handlers
7. **Phase 6.6**: ClickHouse interactive reads for historical queries
8. **Phase 6.7**: gRPC internal surface (deferred, enabled by 6.5)

Critical path: **6.0 → 6.1 → 6.3 → 6.4**. Phases 6.2 and 6.5 are
independently landable at any point. Phase 6.6 depends on ClickHouse sink
already writing data (it does). Phase 6.7 is deferred beyond this change.

Rollback: Each phase is backward compatible. The `all` mode preserves the
current monolith behavior. No API contract changes.

## Open Questions

- **WAL segment size**: 64 MB default is a reasonable starting point for BTC
  5-min books. Should this be tunable per deployment, or is a fixed size
  sufficient?
- **WAL retention policy**: Prune after all consumers advance, or also enforce
  a time-based TTL? Time-based prevents unbounded disk usage if a consumer
  stalls.
- **Checkpoint interval tuning**: Current 5-min checkpoint interval means
  hydration replays at most 5 minutes of events. Is this acceptable, or should
  checkpoints be more frequent for faster cold start?
- **bincode versioning**: Version byte prefix is simple but requires manual
  migration code. Should we use a self-describing format like `rkyv` instead?
