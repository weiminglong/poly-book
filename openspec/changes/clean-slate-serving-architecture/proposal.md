## Why

The current `serve-api` runtime is a monolith: it owns venue WebSocket
connections, normalization, book state, broadcast, and browser-facing HTTP/WS in
a single process. Live state uses `Arc<RwLock<LiveState>>` with writer-blocks-
readers contention on every delta. The broadcast channel copies every asset's
update to every subscriber. There is no crash recovery, no durable state, no
hydration from storage — a process restart means seconds of empty books while the
feed rebuilds from zero. This architecture cannot scale horizontally, cannot
survive restarts gracefully, and couples venue connectivity to browser serving.

Phase 3 shipped this design intentionally as a fast path to a working
workstation. Phase 6 replaces it with a production-grade serving architecture
built around a durable event log, lock-free read models, process separation, and
checkpoint-based hydration.

## What Changes

- Introduce an embedded event log (mmap'd WAL) as the durable, ordered,
  multi-consumer spine between ingest and serving
- Replace `Arc<RwLock<LiveState>>` with a `watch`-based single-writer read model
  that eliminates lock contention on the read path
- Partition the broadcast channel per-asset so WS subscribers only receive
  updates for their subscribed asset
- Add checkpoint-based hydration so serve replicas start serving in <100ms by
  loading the latest checkpoint and replaying the log tail
- Separate ingest and serve into independently deployable runtimes communicating
  through the event log
- Extract domain logic from axum handlers into a transport-neutral service layer
  that can back both HTTP/WS and future gRPC interfaces
- Migrate interactive historical reads (replay, integrity, execution) to
  ClickHouse while keeping Parquet as the audit and replay-truth source

## Capabilities

### New Capabilities

- `event-log`: Durable mmap'd write-ahead log with segment rotation, multi-consumer
  tailing, position tracking, and crash recovery. Replaces ad-hoc mpsc fan-out
  with a replayable ordered event spine.
- `lock-free-read-model`: Single-writer book projector publishing snapshots via
  `watch` channel. Zero reader contention, consistent snapshots, natural batching.
- `checkpoint-hydration`: Serve runtime hydrates from latest stored checkpoint
  plus event log tail on startup. Sub-100ms cold start, stateless replicas.
- `service-layer`: Transport-neutral domain service traits for book, replay,
  integrity, and execution queries. Decouples business logic from HTTP framework.

### Modified Capabilities

- `serving-runtime-platform`: Gains concrete ingest/serve process boundary,
  event log as IPC mechanism, and readiness semantics based on checkpoint
  hydration rather than feed arrival.
- `live-market-observability`: Per-asset broadcast partitioning replaces global
  broadcast fan-out. WS subscribers receive only their asset's updates.

## Impact

- **New crate `pb-wal`**: Embedded event log with mmap segments, CRC framing,
  consumer position tracking, segment pruning
- **New crate `pb-service`**: Transport-neutral service layer extracted from
  `pb-api` handlers
- **pb-api**: Handlers become thin adapters calling `pb-service`. `LiveReadModel`
  rewritten to use `watch`-based publishing. `BookBroadcast` partitioned per-asset.
- **pb-bin**: `serve-api` subcommand gains `--hydrate` flag for checkpoint-based
  startup. Future: separate `ingest` and `serve` binaries.
- **pb-store**: Checkpoint writer gains log position markers for hydration
  coordination
- **pb-replay**: Interactive query paths gain ClickHouse-first routing with
  Parquet fallback for audit
- **Config**: New `[wal]` section for segment size, retention, and path.
  New `[service]` section for hydration and ClickHouse serving backend.
- **No breaking changes to existing API contracts** — all HTTP/WS routes
  maintain their current shape. Changes are internal architectural.
