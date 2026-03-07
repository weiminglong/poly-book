# Latency Characteristics

This document describes the latency profile of each component in the poly-book
pipeline. All measurements are order-of-magnitude estimates from Criterion
benchmarks on commodity hardware (see `cargo bench`).

## Pipeline Latency Budget

```
WebSocket frame arrival
    │
    ├─ WS recv + TLS decrypt     ~50–200 µs  (kernel + TLS)
    │
    ├─ Deserialization            ~1–5 µs     (zero-copy serde)
    │   └─ Book snapshot (10 lvl) ~2 µs
    │   └─ Price change delta     ~0.5 µs
    │   └─ Last trade price       ~0.3 µs
    │
    ├─ Dispatcher normalize       ~1–3 µs     (parse + intern + sequence)
    │   └─ FixedPrice::try_from   ~30 ns
    │   └─ FxHashMap lookup       ~10 ns
    │   └─ Channel send           ~50–100 ns
    │
    ├─ Book update                ~50–200 ns  (BTreeMap insert/remove)
    │   └─ apply_delta            ~50 ns
    │   └─ apply_snapshot (50)    ~2.5 µs
    │   └─ best_bid + best_ask   ~5 ns
    │   └─ weighted_mid_price     ~10 ns
    │   └─ check_integrity        ~10 ns
    │   └─ top_bids/asks(5)       ~20 ns
    │
    ├─ Storage flush              async, off hot path
    │   └─ Parquet row group      ~10–50 ms  (buffered, 5-min interval)
    │   └─ ClickHouse batch       ~5–20 ms   (1-sec interval)
    │
    └─ API response               ~0.1–5 ms  (depending on endpoint)
        └─ /feed/status           ~0.1 ms
        └─ /orderbooks/snapshot   ~0.5 ms
        └─ /replay/reconstruct    ~1–5 ms    (depends on lookback window)
```

## Key Design Choices for Low Latency

| Decision | Latency Impact |
|----------|---------------|
| Fixed-point arithmetic | Eliminates FPU pipeline stalls and NaN guards |
| BTreeMap for book | O(1) best bid/ask via sorted iteration |
| Zero-copy wire deser | ~2–5x fewer allocations per message |
| FxHashMap in Dispatcher | ~3–5x faster hash for asset ID lookups |
| mimalloc allocator | Lower p99 allocation latency under tokio |
| Bounded channels | No lock contention; backpressure prevents OOM |
| Single-threaded book | No synchronization overhead for updates |
| AssetId interning | One Arc alloc per asset, not per message |

## Measuring Latency

### Benchmarks

```bash
cargo bench                      # all benchmarks
cargo bench -p pb-types          # fixed-point and wire deser
cargo bench -p pb-book           # book operations, depth, and analytics
```

### Prometheus Metrics

When running with `--metrics`, the following histograms are exposed:

- `pb_ws_latency_us` — exchange → recv timestamp delta
- `pb_processing_duration_us` — per-message Dispatcher processing time
- `pb_api_request_duration_ms` — HTTP request latency

### Profiling

```bash
cargo build --release
perf record -g ./target/release/poly-book ingest --tokens <ID>
perf report
```

## Replay Reconstruction Latency

Reconstruction time depends on the lookback window and checkpoint availability:

| Scenario | Typical Latency |
|----------|----------------|
| Checkpoint hit, small window | 1–5 ms |
| No checkpoint, 1-hour window | 50–200 ms |
| Full-day reconstruction | 1–5 s |

The replay engine uses checkpoints to bound reconstruction time. The
`run_backfill` command periodically writes REST snapshots as checkpoints,
reducing worst-case replay latency.
