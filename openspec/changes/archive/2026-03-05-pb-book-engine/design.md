# pb-book-engine Design

## Overview

`pb-book` provides the `L2Book` struct: an in-memory Level-2 orderbook that maintains aggregate size at each price level for a single asset. It is designed for high-frequency updates from the Polymarket WebSocket feed.

## Architecture

```
pb-book/src/
  book.rs   -- L2Book struct and all orderbook logic
  error.rs  -- BookError enum
  lib.rs    -- Re-exports
```

### Data structure

```rust
pub struct L2Book {
    pub asset_id: AssetId,
    pub bids: BTreeMap<Reverse<FixedPrice>, FixedSize>,
    pub asks: BTreeMap<FixedPrice, FixedSize>,
    pub sequence: Sequence,
    pub last_update_us: u64,
}
```

- **Bids**: `BTreeMap<Reverse<FixedPrice>, FixedSize>` -- wrapping the key in `std::cmp::Reverse` inverts the ordering so that `iter().next()` yields the highest bid (best bid) first.
- **Asks**: `BTreeMap<FixedPrice, FixedSize>` -- natural ordering means `iter().next()` yields the lowest ask (best ask) first.

This design gives O(log n) insert/remove/lookup and O(1) best-price access via `iter().next()`, with no custom comparator needed.

## Key Decisions

### BTreeMap over Vec or HashMap

- **vs Vec**: A sorted Vec would require O(n) insertion. BTreeMap gives O(log n) for all mutations.
- **vs HashMap**: HashMap has no ordering, so finding best bid/ask would require O(n) scan. BTreeMap iteration is ordered by key.
- **Cache friendliness**: BTreeMap nodes are contiguous, which is acceptable for the typical 20-50 price levels in a Polymarket book.

### Reverse wrapper for bids

Using `Reverse<FixedPrice>` as the key type leverages the standard library's `Reverse` wrapper to flip ordering. This avoids a custom `Ord` impl or a separate `BidPrice` newtype. The tradeoff is that callers must wrap/unwrap with `Reverse`, but this is contained within `L2Book` methods.

### Snapshot replaces entire state

`apply_snapshot` calls `clear()` on both sides before inserting new levels. This ensures stale levels from a previous snapshot do not persist. Zero-size entries in the snapshot input are silently skipped.

### Delta semantics

`apply_delta` follows Polymarket's convention: size=0 means remove the level, size>0 means insert or replace. This is handled with a simple branch per side.

### Sequence gap detection

`check_sequence` compares the incoming sequence against `self.sequence + 1`. If the current sequence is 0 (initial state), any incoming sequence is accepted. This allows the first snapshot to set the baseline. Gaps return `BookError::SequenceGap { expected, got }`.

## Implementation Details

### Query methods

- `best_bid()` / `best_ask()`: `iter().next()` on the respective BTreeMap, O(1) amortized.
- `mid_price()`: Average of best bid and best ask as f64. Returns `None` if either side is empty.
- `spread()`: `best_ask - best_bid` as f64. Returns `None` if either side is empty.
- `bid_depth()` / `ask_depth()`: `.len()` on the BTreeMap, O(1).
- `bids_sorted()` / `asks_sorted()`: Collect iterator into `Vec<(FixedPrice, FixedSize)>`, unwrapping `Reverse` for bids.

### Error types

`BookError` has three variants: `InvalidPrice` (string context), `UnknownSide` (string context), and `SequenceGap { expected, got }` (both u64). All use `thiserror` for `Display`/`Error` derives.
