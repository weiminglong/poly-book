# Architecture

## System Data Flow

```text
                        ┌─────────────────────────────────────────────┐
                        │                 Polymarket                  │
                        │            (venue WebSocket + REST)         │
                        └──────┬──────────────────┬──────────────────┘
                               │                  │
                          WebSocket            REST API
                               │                  │
                               ▼                  ▼
                        ┌────────────┐     ┌────────────┐
                        │  WsClient  │     │ RestClient │
                        │  (pb-feed) │     │  (pb-feed) │
                        └──────┬─────┘     └──────┬─────┘
                               │                  │
                               ▼                  │
                        ┌──────────────┐          │
                        │  Dispatcher  │◄─────────┘
                        │   (pb-feed)  │
                        └──────┬───────┘
                               │
                    tokio::mpsc channel (PersistedRecord)
                               │
         ┌─────────────────────┼────────────────────┐
         │                     │                    │
         ▼                     ▼                    ▼
  ┌─────────────┐      ┌─────────────┐      ┌──────────────┐
  │ ParquetSink │      │ClickHouse   │      │  WalWriter   │
  │  (pb-store) │      │Sink         │      │  (pb-wal)    │
  │  5-min Zstd │      │(pb-store)   │      │ append+CRC32C│
  └──────┬──────┘      │ 1s batch    │      └──────┬───────┘
         │             └──────┬──────┘             │
         │                    │               WAL segments
         ▼                    ▼                    │
  ┌──────────┐         ┌──────────┐                ▼
  │ Parquet  │         │ClickHouse│         ┌──────────────┐
  │  files   │         │  tables  │         │  WalReader   │
  └────┬─────┘         └────┬─────┘         │  (pb-wal)    │
       │                    │               └──────┬───────┘
       └────────┬───────────┘                      │
                │                                  ▼
                ▼                           ┌──────────────┐
         ┌─────────────┐                    │ LiveReadModel │
         │ EventReader  │                   │   (pb-api)   │
         │ (pb-replay)  │                   │ watch-based  │
         └──────┬───────┘                   └──────┬───────┘
                │                                  │
                ▼                           ┌──────┴───────┐
         ┌──────────────┐                   │              │
         │ ReplayEngine │                REST/WS     per-asset
         │ (pb-replay)  │                   │         broadcast
         └──────┬───────┘                   ▼              ▼
                │                        HTTP          WebSocket
                ▼                       handlers        clients
         ┌──────────────┐
         │ pb-service   │  ReplayService / IntegrityService /
         │  (traits)    │  ExecutionService
         └──────┬───────┘
                │
                ▼
         ┌──────────────┐
         │   pb-api     │  thin HTTP adapters
         │  (handlers)  │
         └──────────────┘
```

## Crate Dependency Graph

```text
pb-bin (CLI entrypoint)
├── pb-api
│   ├── pb-types
│   ├── pb-book
│   │   └── pb-types
│   ├── pb-service
│   │   ├── pb-types
│   │   ├── pb-book
│   │   └── pb-replay
│   ├── pb-replay
│   ├── pb-wal
│   │   └── pb-types
│   ├── pb-metrics
│   └── pb-store (test fixtures)
│       ├── pb-types
│       └── pb-metrics
├── pb-grpc
│   ├── pb-types
│   └── pb-service
├── pb-feed
│   ├── pb-types
│   └── pb-metrics
├── pb-store
├── pb-replay
├── pb-wal
├── pb-service
├── pb-metrics
└── pb-types
```

Leaf crates (no internal dependencies): **pb-types**, **pb-metrics**.

## Persisted Record Model

All data flows through `PersistedRecord` (defined in pb-types), which splits into
six event datasets. Each dataset has its own Parquet schema and ClickHouse table:

| Dataset             | Source          | Content                                |
|---------------------|-----------------|----------------------------------------|
| `BookEvent`         | WS deltas       | L2 orderbook price level changes       |
| `TradeEvent`        | WS trades       | Matched trades with fidelity label     |
| `IngestEvent`       | WS lifecycle    | Connect, disconnect, reconnect markers |
| `BookCheckpoint`    | REST snapshots  | Full book state at a point in time     |
| `ReplayValidation`  | Replay engine   | REST-vs-replay comparison results      |
| `ExecutionEvent`    | External / CLI  | Order lifecycle state changes          |

## Runtime Topology

### Process Separation (ingest + serve)

```text
┌───────────────────────────────────────────────────────┐
│                    ingest process                      │
│                                                       │
│  WsClient ──▶ Dispatcher ──┬──▶ WalWriter (pb-wal)   │
│                    ▲        ├──▶ ParquetSink           │
│  RestClient ───────┘        └──▶ ClickHouseSink        │
│                                                       │
│  CheckpointProducer (periodic REST snapshots + WAL    │
│                       offset capture)                  │
│  Metrics server on :9090                              │
└───────────────────────────────────────────────────────┘
          │
          │  WAL segments on shared filesystem
          ▼
┌───────────────────────────────────────────────────────┐
│                    serve process                       │
│                                                       │
│  Checkpoint hydration ──▶ WAL tail ──▶ LiveReadModel  │
│  (cold start: load latest checkpoint,                 │
│   replay WAL from offset, then live tail)             │
│                                                       │
│  LiveReadModel (watch-based, zero-contention reads)   │
│       │                                               │
│       ├─▶ REST handlers (pb-service traits)           │
│       └─▶ per-asset WS broadcasts                     │
│                                                       │
│  pb-service ──▶ Parquet or ClickHouse backend          │
│  (configurable via api.historical_backend)            │
│                                                       │
│  API server on :3000                                  │
│  gRPC server on :50051 (optional)                     │
│  Metrics server on :9090                              │
└───────────────────────────────────────────────────────┘
```

### Combined Mode (serve-api)

```text
┌─────────────────────────────────────────────────────┐
│                  serve-api process                   │
│                                                     │
│  WsClient ──▶ Dispatcher ──▶ LiveReadModel          │
│                                   │                  │
│                                   ├─▶ REST handlers  │
│                                   └─▶ WS streaming   │
│  (in-memory only — no WAL, no storage sinks)        │
│                                                     │
│  pb-service ──▶ ReplayEngine ──▶ replay handlers    │
│  (reads historical Parquet/ClickHouse for replay)   │
│                                                     │
│  Metrics server on :9090                            │
│  API server on :3000                                │
│  gRPC server on :50051 (optional)                   │
└─────────────────────────────────────────────────────┘
```

### Web SPA

```text
┌──────────────────────────┐
│     web SPA (:4173)      │
│  Vite dev server         │
│  proxies /api → :3000    │
└──────────────────────────┘
```

## Flow Control & Backpressure

Components communicate over bounded `tokio::mpsc` channels, so the policy for what
happens when a consumer falls behind is explicit:

- **The WAL is the only unconditionally-blocking consumer.** The ingest loop
  awaits each `event_rx.recv()` and appends to the WAL before fan-out; if the WAL
  cannot keep up the whole pipeline blocks (and ultimately applies backpressure to
  the feed). This is deliberate: durability is never sacrificed for throughput.
- **Storage sinks may lag.** Sinks consume from their own bounded fan-out
  channels and flush with bounded-retry. A sink falling behind does not block
  ingest indefinitely; sustained failure surfaces as `pb_sink_flush_failures_total`
  (alerted) and the WAL remains the source of truth — a lost storage window is
  rebuildable with `reconcile`.
- **Channel depth is observable.** The ingest event-channel depth is exported as
  `pb_channel_depth{channel="ingest_events"}`; rising depth is the leading
  indicator of a downstream stall, before latency (`pb_recv_to_durable_us`)
  degrades.
- **Capacities** are currently fixed (event/raw channels 2048, sink fan-out
  10000). They should be sized from measured rotation-burst depth using the depth
  gauge above; multi-replica writer leasing / feed redundancy (HA failover) is a
  separate, deferred phase.

## Key Design Decisions

| ADR | Decision | Rationale |
|-----|----------|-----------|
| [ADR-0001](adr/0001-fixed-point-arithmetic.md) | Fixed-point over floating-point | Eliminate FPU stalls and NaN guards |
| [ADR-0002](adr/0002-btreemap-orderbook.md) | BTreeMap for L2 book | ~3 ns best bid/ask measured; dense-array alternative benchmarked |
| [ADR-0003](adr/0003-channel-message-passing.md) | Channel-based message passing | No locks on hot path |
| [ADR-0004](adr/0004-zero-copy-deserialization.md) | Zero-copy wire deserialization | Reduce allocations on ingest path |
| [ADR-0005](adr/0005-mimalloc-allocator.md) | mimalloc global allocator | Lower p99 latency under tokio |
| [ADR-0006](adr/0006-fxhashmap-dispatcher.md) | FxHashMap in Dispatcher | Faster non-cryptographic lookups on trusted data |
| [ADR-0007](adr/0007-release-profile.md) | panic=abort release profile with symbolizable backtraces | Fail-stop over limping on, without losing diagnosability |
| [ADR-0008](adr/0008-embedded-wal-over-message-broker.md) | Embedded single-writer WAL over a message broker | Sub-µs appends, no broker ops on a single host |
| [ADR-0009](adr/0009-dual-sink-storage.md) | Dual-sink storage: Parquet cold, ClickHouse warm | Portable replay truth + interactive SQL |
| [ADR-0010](adr/0010-ingest-serve-process-separation.md) | Ingest/serve process separation | Isolate capture from read-path faults; restartable serving |
| [ADR-0011](adr/0011-read-only-workstation-boundary.md) | Read-only workstation boundary | Blast-radius control; no control plane without its prerequisites |
