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
  │  5-min Zstd │      │(pb-store)   │      │ mmap+CRC32C  │
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
│   ├── pb-wal
│   │   └── pb-types
│   ├── pb-metrics
│   └── pb-store (test fixtures)
│       ├── pb-types
│       └── pb-metrics
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
│  Metrics server on :9090                              │
└───────────────────────────────────────────────────────┘
```

### Combined Mode (serve-api / all)

```text
┌─────────────────────────────────────────────────────┐
│                 serve-api / all process              │
│                                                     │
│  WsClient ──▶ Dispatcher ──▶ LiveReadModel          │
│                    │              │                  │
│                    ▼              ├─▶ REST handlers  │
│              ParquetSink          └─▶ WS streaming   │
│                                                     │
│  pb-service ──▶ ReplayEngine ──▶ replay handlers    │
│                                                     │
│  Metrics server on :9090                            │
│  API server on :3000                                │
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

## Key Design Decisions

| ADR | Decision | Rationale |
|-----|----------|-----------|
| [ADR-0001](adr/0001-fixed-point-arithmetic.md) | Fixed-point over floating-point | Eliminate FPU stalls and NaN guards |
| [ADR-0002](adr/0002-btreemap-orderbook.md) | BTreeMap for L2 book | O(1) best bid/ask via sorted iteration |
| [ADR-0003](adr/0003-channel-message-passing.md) | Channel-based message passing | No locks on hot path |
| [ADR-0004](adr/0004-zero-copy-deserialization.md) | Zero-copy wire deserialization | Reduce allocations on ingest path |
| [ADR-0005](adr/0005-mimalloc-allocator.md) | mimalloc global allocator | Lower p99 latency under tokio |
| [ADR-0006](adr/0006-fxhashmap-dispatcher.md) | FxHashMap in Dispatcher | Faster non-cryptographic lookups on trusted data |
