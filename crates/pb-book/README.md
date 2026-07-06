# pb-book

In-memory L2 order book engine. Maintains bid and ask sides as sorted maps,
applies snapshots and deltas, and provides analytics (mid price, spread,
weighted mid, depth queries, integrity checks).

## Key Types

| Type | Description |
|------|-------------|
| `L2Book` | The order book. Bids: `BTreeMap<Reverse<FixedPrice>, FixedSize>` (best-first). Asks: `BTreeMap<FixedPrice, FixedSize>` (best-first). |
| `BookSide` | Type alias: `Vec<(FixedPrice, FixedSize)>` for one side of the book. |
| `BookError` | Error type for book operations (sequence gaps, integrity violations, aggregate overflow). |

## Methods

`apply_snapshot`, `try_apply_snapshot`, `apply_delta`, `try_apply_delta`,
`best_bid`, `best_ask`, `mid_price`, `spread`, `weighted_mid_price`,
`total_bid_size`, `total_ask_size`, `top_bids`, `top_asks`, `bids_sorted`,
`asks_sorted`, `bid_depth`, `ask_depth`, `check_integrity`, `check_sequence`.

## Data Flow

```text
pb-feed (Dispatcher)
    │
    ▼ BookEvent / BookCheckpoint
pb-book (L2Book)
    │
    ├──▶ pb-replay (ReplayEngine reconstructs book)
    └──▶ pb-api (LiveReadModel maintains live book)
```

## Design Notes

- `Reverse<FixedPrice>` on the bid side ensures `BTreeMap` iteration yields
  best bid first without extra sorting. See [ADR-0002](../../docs/adr/0002-btreemap-orderbook.md).
- **O(1) total sizes**: `total_bid_size` and `total_ask_size` are maintained via
  checked running sums (`total_bid_raw` / `total_ask_raw` fields) updated during
  `try_apply_snapshot` and `try_apply_delta`, avoiding full-tree walks without
  panic-prone integer overflow. If a total would overflow, the checked method
  returns `BookError::AggregateOverflow` and leaves the book unchanged.
- `#[inline]` on hot-path methods: `apply_delta`, `best_bid`, `best_ask`,
  `mid_price`, `spread`, `weighted_mid_price`.
- `check_integrity` detects crossed books (best bid >= best ask). Zero-size
  levels are prevented at insertion time by `apply_snapshot` and `apply_delta`.
- `check_sequence` enforces monotonic sequence numbers and detects gaps.
- `proptest` suites verify ordering, spread, snapshot idempotency, and crossed-book
  detection.
- 69 tests covering empty book, single level, snapshot duplicates, delta sequences,
  and proptest invariants.

## Benchmarks

`benches/book_ops.rs` and `benches/book_depth.rs` cover snapshot/delta/read
workloads; `benches/array_book.rs` keeps the ADR-0002 comparison honest by
running an epoch-stamped dense-array book (the bounded-tick-grid alternative)
against `L2Book` on identical workloads. Measured results and the retention
rationale live in `docs/adr/0002-btreemap-orderbook.md`.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New `L2Book` method added | `pb-api` `dto.rs` if exposed to clients, `docs/api.md` if it creates a new route |
| Book integrity semantics changed | `docs/serve-api.md`, integrity capability spec |
| Snapshot/delta format changed | `pb-store` schema, `pb-replay` reconstruction logic |
| Changes affect the API surface | Check the active OpenSpec change under `openspec/changes/` |
