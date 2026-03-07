# ADR-0001: Fixed-Point Arithmetic Over Floating Point

## Status
Accepted

## Context
Polymarket prices are decimals in [0.0, 1.0] and order sizes are non-negative
decimals. Financial systems require exact arithmetic — floating-point rounding
errors are unacceptable for order book state, sequence gap detection, and trade
reconciliation.

## Decision
All prices and sizes use fixed-point integer representations:

- **FixedPrice(u32)**: scaled by 10,000. Range 0–10,000 maps to 0.0000–1.0000.
  Fits in 4 bytes, `Copy`, trivial `Ord`.
- **FixedSize(u64)**: scaled by 1,000,000. Fits in 8 bytes, `Copy`, trivial `Ord`.

Floating-point is used only at serialization boundaries (JSON display, f64
conversion for analytics). The hot path never touches `f64`.

## Consequences
- **Correctness**: Equality, ordering, and hashing are exact. No epsilon
  comparisons needed.
- **Performance**: Integer comparison is a single CPU instruction. No NaN/Inf
  guard on the hot path. `BTreeMap` ordering is trivially correct.
- **Trade-off**: Precision is limited by the scale factor. FixedPrice has
  0.0001 resolution. This is sufficient for Polymarket's price tick size.
- **Validation**: Property-based tests (proptest) verify roundtrip,
  ordering, and serde invariants across the full value range.
