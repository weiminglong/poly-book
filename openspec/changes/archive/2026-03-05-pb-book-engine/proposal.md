# pb-book-engine

## Why

The system needs an in-memory Level-2 orderbook that can efficiently apply snapshots and deltas from the Polymarket WebSocket feed, provide best bid/ask lookups, and detect sequence gaps -- all with minimal allocation and correct price ordering.

## What Changes

- Introduce `L2Book` struct with `BTreeMap`-based bid and ask sides
- Bids keyed by `Reverse<FixedPrice>` for highest-first iteration; asks keyed by `FixedPrice` for lowest-first
- `apply_snapshot` replaces entire book state; `apply_delta` inserts/updates/removes individual levels
- Query methods: `best_bid`, `best_ask`, `mid_price`, `spread`, `bid_depth`, `ask_depth`
- Sorted export: `bids_sorted()`, `asks_sorted()`
- Sequence gap detection via `check_sequence`
- `BookError` enum for error reporting

## Capabilities

### New Capabilities

- `l2-book-engine`: Full Level-2 orderbook with BTreeMap storage, snapshot/delta application, best-price queries, mid/spread calculation, depth inspection, and sequence gap detection

### Modified Capabilities

(none)

## Impact

- **Depends on**: `pb-types` (FixedPrice, FixedSize, AssetId, Sequence, Side)
- **Consumed by**: `pb-feed` (applies WebSocket events to book), `pb-store` (persists book state), `pb-metrics` (reads spreads/depths)
