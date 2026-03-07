# ADR-0002: BTreeMap for L2 Order Book

## Status
Accepted

## Context
The L2 order book needs a data structure that maintains price-level ordering
and supports efficient insert/remove/lookup by price. Alternatives considered:

1. **Vec + sort**: O(n) insert, O(n log n) sort. Poor for incremental updates.
2. **HashMap + sort on read**: O(1) insert but O(n log n) for ordered iteration.
3. **BTreeMap**: O(log n) insert/remove/lookup, O(n) in-order iteration.
4. **Custom skip list / radix tree**: Lower constant factors but higher
   implementation complexity.

## Decision
Use `BTreeMap<Reverse<FixedPrice>, FixedSize>` for bids (best-first = highest
first) and `BTreeMap<FixedPrice, FixedSize>` for asks (best-first = lowest
first).

## Consequences
- **Best bid/ask**: O(1) via `iter().next()` — BTreeMap iterates in sorted
  order, so the first element is always the best price.
- **Delta application**: O(log n) insert/remove per level.
- **Depth iteration**: O(k) for top-k levels, already in correct order.
- **Cache locality**: BTreeMap nodes are heap-allocated but B-tree fan-out
  provides reasonable cache behavior for typical book depths (10–200 levels).
- **Spread/mid**: Derived from best_bid + best_ask in O(1).

## Benchmarks
- `apply_snapshot` (50 levels): ~2.5 µs
- `apply_delta` (single level): ~50 ns
- `best_bid + best_ask`: ~5 ns
- `1M delta applies`: ~200 ms

These are well within the latency budget for a market data system processing
~1,000 updates/second.
