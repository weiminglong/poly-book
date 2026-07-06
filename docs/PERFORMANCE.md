# Performance

Measured Criterion results for the hot-path operations, regenerated with
`just bench-report` (which runs `cargo bench --workspace` and rewrites this
file). Numbers are medians of Criterion's sampled iterations on the machine
below — single-machine, wall-clock measurements for order-of-magnitude
reasoning, not a controlled lab benchmark. CI compiles every benchmark on
each PR (`bench` job) but does not gate on timings: shared runners are too
noisy for statistical regression detection, so regression checks are run
locally against this file.

## Measurement context

- CPU: Apple M3 (8 cores), RAM: 16 GB
- OS: Darwin 25.5.0 (arm64)
- Toolchain: rustc 1.94.0 (4a4ef493e 2026-03-02), bench profile (inherits release: thin LTO, overflow-checks on)
- Commit: `e9ecd2a`, measured 2026-07-06

## Pipeline hot path

| Stage | Operation | Median | Per item | Rate |
|---|---|---|---|---|
| ingest | WS `price_change` deserialize (zero-copy) | 504 ns | 504 ns | 2.0 M/s |
| ingest | WS book snapshot deserialize (10 levels) | 2.33 µs | 2.33 µs | 429 k/s |
| ingest | Dispatcher normalize + shadow-book cross-check | 483.68 µs | 484 ns | 2.1 M/s |
| durability | WAL codec encode (book delta) | 87 ns | 87 ns | 11.5 M/s |
| durability | WAL append + flush | 508.71 µs | 509 ns | 2.0 M/s |
| durability | WAL append + fdatasync every record | 403.98 ms | 4.04 ms | 248/s |
| book | Book delta apply | 9 ns | 9 ns | 107.9 M/s |
| book | Book snapshot rebuild (50 levels) | 1.10 µs | 1.10 µs | 907 k/s |
| book | Top-of-book read (best bid + ask) | 1 ns | 1 ns | 760.5 M/s |
| serving | Read-model snapshot (50 levels, depth 20) | 154 ns | 154 ns | 6.5 M/s |
| book | Mixed workload: deltas on a 20-level book | 119.68 µs | 12 ns | 83.6 M/s |

Batch benches (dispatcher, WAL, mixed workload) time the whole batch per
iteration; the per-item column divides by the batch size.

## All results

| Benchmark | Median | Mean | Std dev |
|---|---|---|---|
| `1M delta applies` | 10.36 ms | 10.30 ms | 401.80 µs |
| `FixedPrice comparison` | 0 ns | 0 ns | 0 ns |
| `FixedPrice serde roundtrip` | 50 ns | 50 ns | 0 ns |
| `FixedPrice__from_f64` | 2 ns | 2 ns | 0 ns |
| `FixedPrice__try_from str` | 10 ns | 10 ns | 0 ns |
| `FixedSize__from_f64` | 2 ns | 2 ns | 0 ns |
| `L2Book__apply_delta` | 9 ns | 9 ns | 0 ns |
| `L2Book__apply_snapshot (50 levels)` | 1.10 µs | 1.10 µs | 3 ns |
| `L2Book__best_bid + best_ask` | 1 ns | 1 ns | 0 ns |
| `L2Book__mid_price` | 1 ns | 1 ns | 0 ns |
| `LiveReadModel__active_assets` | 102 ns | 102 ns | 1 ns |
| `LiveReadModel__feed_status_raw` | 60 ns | 60 ns | 0 ns |
| `LiveReadModel__is_asset_active` | 23 ns | 23 ns | 0 ns |
| `LiveReadModel__snapshot (50 levels, depth=20)` | 154 ns | 155 ns | 1 ns |
| `analytics/check_integrity` | 3 ns | 3 ns | 0 ns |
| `analytics/top_5_asks` | 26 ns | 26 ns | 0 ns |
| `analytics/top_5_bids` | 26 ns | 26 ns | 0 ns |
| `analytics/total_ask_size` | 0 ns | 0 ns | 0 ns |
| `analytics/total_bid_size` | 0 ns | 0 ns | 0 ns |
| `analytics/weighted_mid_price` | 2 ns | 2 ns | 0 ns |
| `book_depth_iteration/asks_sorted/10` | 30 ns | 30 ns | 0 ns |
| `book_depth_iteration/asks_sorted/100` | 115 ns | 115 ns | 1 ns |
| `book_depth_iteration/asks_sorted/200` | 214 ns | 215 ns | 1 ns |
| `book_depth_iteration/asks_sorted/50` | 72 ns | 72 ns | 1 ns |
| `book_depth_iteration/bids_sorted/10` | 29 ns | 29 ns | 1 ns |
| `book_depth_iteration/bids_sorted/100` | 114 ns | 115 ns | 3 ns |
| `book_depth_iteration/bids_sorted/200` | 214 ns | 215 ns | 1 ns |
| `book_depth_iteration/bids_sorted/50` | 73 ns | 72 ns | 1 ns |
| `codec__encode (book delta)` | 87 ns | 87 ns | 1 ns |
| `dispatcher/price_change normalize+shadow-book (200x5 entries)` | 483.68 µs | 487.26 µs | 11.69 µs |
| `mixed_workload/10k_deltas_on_20_level_book` | 119.68 µs | 120.22 µs | 2.64 µs |
| `snapshot_at_depth/apply_snapshot/10` | 141 ns | 141 ns | 1 ns |
| `snapshot_at_depth/apply_snapshot/100` | 2.55 µs | 2.55 µs | 20 ns |
| `snapshot_at_depth/apply_snapshot/200` | 5.70 µs | 5.71 µs | 35 ns |
| `snapshot_at_depth/apply_snapshot/50` | 1.11 µs | 1.11 µs | 3 ns |
| `spread_at_depth/mid_price/10` | 1 ns | 1 ns | 0 ns |
| `spread_at_depth/mid_price/100` | 1 ns | 1 ns | 0 ns |
| `spread_at_depth/mid_price/200` | 1 ns | 1 ns | 0 ns |
| `spread_at_depth/mid_price/50` | 1 ns | 1 ns | 0 ns |
| `spread_at_depth/spread/10` | 1 ns | 1 ns | 0 ns |
| `spread_at_depth/spread/100` | 1 ns | 1 ns | 0 ns |
| `spread_at_depth/spread/200` | 1 ns | 1 ns | 0 ns |
| `spread_at_depth/spread/50` | 1 ns | 1 ns | 0 ns |
| `wal_append/append+fdatasync-each (100 records)` | 403.98 ms | 404.91 ms | 3.27 ms |
| `wal_append/append+flush (1k records)` | 508.71 µs | 501.22 µs | 73.13 µs |
| `wire_deser/batch_100_mixed_messages` | 303.77 µs | 305.81 µs | 14.63 µs |
| `wire_deser/book_snapshot_10_levels` | 2.33 µs | 2.33 µs | 33 ns |
| `wire_deser/last_trade_price` | 259 ns | 262 ns | 5 ns |
| `wire_deser/price_change_delta` | 504 ns | 509 ns | 11 ns |

The ClickHouse-backed cross-backend comparison bench requires a running
ClickHouse and is not part of this run.
