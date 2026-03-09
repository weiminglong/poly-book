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
              ┌────────────────┼────────────────┐
              │                │                │
              ▼                ▼                ▼
       ┌─────────────┐ ┌─────────────┐  ┌──────────────┐
       │ ParquetSink │ │ClickHouse   │  │ LiveReadModel│
       │  (pb-store) │ │Sink         │  │   (pb-api)   │
       │  5-min Zstd │ │(pb-store)   │  │              │
       └──────┬──────┘ │ 1s batch    │  └──────┬───────┘
              │        └──────┬──────┘         │
              │               │         ┌──────┴───────┐
              ▼               ▼         │              │
       ┌──────────┐    ┌──────────┐  REST/WS     broadcast
       │ Parquet  │    │ClickHouse│    │              │
       │  files   │    │  tables  │    ▼              ▼
       └────┬─────┘    └────┬─────┘  HTTP          WebSocket
            │               │      handlers        clients
            └───────┬───────┘
                    │
                    ▼
             ┌─────────────┐
             │ EventReader  │  (ParquetReader / ClickHouseReader)
             │ (pb-replay)  │
             └──────┬───────┘
                    │
                    ▼
             ┌──────────────┐
             │ ReplayEngine │  reconstruct_at(asset_id, timestamp)
             │ (pb-replay)  │
             └──────┬───────┘
                    │
                    ▼
             ┌──────────────┐
             │   pb-api     │  /replay/reconstruct, /integrity/summary
             │  (read path) │
             └──────────────┘
```

## Crate Dependency Graph

```text
pb-bin (CLI entrypoint)
├── pb-api
│   ├── pb-types
│   ├── pb-book
│   │   └── pb-types
│   ├── pb-replay
│   │   ├── pb-types
│   │   ├── pb-book
│   │   └── pb-metrics
│   ├── pb-metrics
│   └── pb-store (test fixtures)
│       ├── pb-types
│       └── pb-metrics
├── pb-feed
│   ├── pb-types
│   └── pb-metrics
├── pb-store
├── pb-replay
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

```text
┌─────────────────────────────────────────────────────┐
│                    serve-api process                 │
│                                                     │
│  WsClient ──▶ Dispatcher ──▶ LiveReadModel          │
│                    │              │                  │
│                    ▼              ├─▶ REST handlers  │
│              ParquetSink          └─▶ WS streaming   │
│                                                     │
│  ParquetReader ──▶ ReplayEngine ──▶ replay handlers │
│                                                     │
│  Metrics server on :9090                            │
│  API server on :3000                                │
└─────────────────────────────────────────────────────┘

┌───────────────────────────────────────────────────┐
│              ingest / auto-ingest process          │
│                                                    │
│  WsClient ──▶ Dispatcher ──┬──▶ ParquetSink       │
│                    ▲        └──▶ ClickHouseSink    │
│  RestClient ───────┘                               │
│                                                    │
│  Metrics server on :9090                           │
└───────────────────────────────────────────────────┘

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
