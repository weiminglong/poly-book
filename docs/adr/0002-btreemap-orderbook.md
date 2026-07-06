# ADR-0002: BTreeMap for L2 Order Book

## Status
Accepted (re-examined against a measured dense-array alternative; see below)

## Context
The L2 order book needs a data structure that maintains price-level ordering
and supports efficient insert/remove/lookup by price. Polymarket prices are
`FixedPrice` values on a bounded grid (raw ticks 0..=10,000), which admits a
design most order-book implementations on bounded grids use: a dense array
indexed by tick. Alternatives considered:

1. **Vec + sort**: O(n) insert, O(n log n) sort. Poor for incremental updates.
2. **HashMap + sort on read**: O(1) insert but O(n log n) for ordered iteration.
3. **BTreeMap**: O(log n) insert/remove/lookup, O(n) in-order iteration.
4. **Dense array indexed by tick + best-price cursor**: O(1) level writes,
   O(1) top-of-book, O(gap width) cursor rescan when the best level empties,
   O(scan distance) top-k iteration; fixed memory per side.
5. **Custom skip list / radix tree**: Lower constant factors than BTreeMap but
   higher implementation complexity; strictly dominated by the dense array on
   a bounded grid.

## Decision
Use `BTreeMap<Reverse<FixedPrice>, FixedSize>` for bids (best-first = highest
first) and `BTreeMap<FixedPrice, FixedSize>` for asks (best-first = lowest
first).

The dense array was implemented at full strength (epoch-stamped cells so a
snapshot rebuild costs O(levels), not an 80 KB clear) and measured against
`L2Book` on identical workloads — the comparison lives in
`crates/pb-book/benches/array_book.rs` and stays runnable:

| Workload (50 levels, 10-tick spacing, Apple M3) | BTreeMap | Dense array |
|---|---|---|
| Snapshot rebuild (50 levels) | 1.11 µs | 59 ns |
| Delta apply (resting level) | 7.7 ns | 0.73 ns |
| Best bid + ask read | 2.8 ns | 2.6 ns |
| Top-10 depth iteration | 31 ns | 39 ns |
| Best-level churn (remove + restore) | 26 ns | 3.8 ns |

The array book wins the write path by 10–19x and ties top-of-book reads.
BTreeMap is retained anyway, deliberately:

- **Book operations are not the bottleneck.** The slowest book operation is
  ~1.1 µs against a pipeline whose per-event cost is dominated by wire
  deserialization (~0.5 µs), WAL append (~0.5 µs), and millisecond-scale
  venue latency (see docs/PERFORMANCE.md). Cutting 7 ns to 0.7 ns is not
  observable end to end.
- **No grid coupling.** BTreeMap works for any `FixedPrice` distribution —
  a future venue without a bounded 10,001-tick grid needs no book rewrite.
- **Memory proportional to occupancy.** The epoch-stamped array pins ~320 KB
  per asset (sizes + stamps, both sides) regardless of book depth; the
  auto-rotating feed touches many assets per hour.
- **Sparse-book behavior.** Array top-k iteration costs the scan distance
  between occupied ticks, which degrades on the near-empty books that
  dominate around market resolution; BTreeMap's top-k is O(k) regardless.

**Crossover condition:** if book operations ever show up in the end-to-end
p99 profile (for example, a much deeper book, a much hotter replay loop, or a
per-message multi-level workload), switch the hot side to the dense array —
the benchmark proves the design and keeps its numbers current.

## Consequences
- **Best bid/ask**: first-element access via `iter().next()` — O(log n) tree
  descent in principle, measured ~2.8 ns at realistic depths (10–200 levels),
  effectively flat.
- **Delta application**: O(log n) insert/remove per level (~8 ns measured).
- **Depth iteration**: O(k) for top-k levels, already in correct order.
- **Cache locality**: BTreeMap nodes are heap-allocated but B-tree fan-out
  provides reasonable cache behavior for typical book depths (10–200 levels).
- **Spread/mid**: derived from best_bid + best_ask at top-of-book cost.

## Benchmarks
Current measured medians live in [docs/PERFORMANCE.md](../PERFORMANCE.md)
(regenerated with `just bench-report`); the BTreeMap-vs-array comparison is
`cargo bench -p pb-book --bench array_book`. Throughput headroom: ~9 ns per
delta apply against a feed that produces ~1,000–2,000 updates/second.
