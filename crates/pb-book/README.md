# pb-book

In-memory L2 order book engine. Maintains bid and ask sides as sorted maps,
applies snapshots and deltas, and provides analytics (mid price, spread,
weighted mid, depth queries, integrity checks).

## Key Types

| Type | Description |
|------|-------------|
| `L2Book` | The order book. Bids: `BTreeMap<Reverse<FixedPrice>, FixedSize>` (best-first). Asks: `BTreeMap<FixedPrice, FixedSize>` (best-first). |
| `BookError` | Error type for book operations (sequence gaps, integrity violations). |

## Methods

`apply_snapshot`, `apply_delta`, `best_bid`, `best_ask`, `mid_price`, `spread`,
`weighted_mid_price`, `total_bid_size`, `total_ask_size`, `top_bids`, `top_asks`,
`check_integrity`, `check_sequence`.

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
- `check_integrity` verifies: no negative sizes, no zero-size levels, and flags
  crossed books (best bid >= best ask).
- `check_sequence` enforces monotonic sequence numbers and detects gaps.
- `proptest` suites verify ordering, spread, snapshot idempotency, and crossed-book
  detection.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New `L2Book` method added | `pb-api` `dto.rs` if exposed to clients, `docs/api.md` if it creates a new route |
| Book integrity semantics changed | `docs/serve-api.md`, integrity capability spec |
| Snapshot/delta format changed | `pb-store` schema, `pb-replay` reconstruction logic |
| Changes affect the API surface | Check the active OpenSpec change under `openspec/changes/` |
