# pb-types-foundation Design

## Overview

`pb-types` provides the foundational type system for the poly-book orderbook pipeline. Every crate in the workspace imports these types to ensure a single, consistent representation of prices, sizes, events, and wire formats. The crate is organized into five modules: `fixed`, `newtype`, `wire`, `event`, and `error`.

## Architecture

```
pb-types/src/
  fixed.rs    -- FixedPrice(u32), FixedSize(u64)
  newtype.rs  -- AssetId(String), Sequence(u64)
  wire.rs     -- WsMessage<'a>, RestBookResponse, GammaEvent
  event.rs    -- OrderbookEvent, Side, EventType, PriceLevel
  error.rs    -- TypesError
  lib.rs      -- Re-exports
```

All types derive standard traits (`Debug`, `Clone`, `Serialize`, `Deserialize`) and the fixed-point types additionally derive `Copy`, `Eq`, `Ord`, and `Hash` for use as BTreeMap keys.

## Key Decisions

### Fixed-point over Decimal

Polymarket prices are always in the range [0.00, 1.00] with at most 4 decimal places. Using `rust_decimal` or `f64` would introduce unnecessary overhead or precision issues:

- **`FixedPrice(u32)`** with scale 10,000 maps the full price range to `0..=10_000`. This fits in 4 bytes, is `Copy`, and supports trivial `Ord` -- critical for BTreeMap-based orderbook sides.
- **`FixedSize(u64)`** with scale 1,000,000 provides 6 decimal places of precision for order sizes without floating-point drift.
- Both types serialize as decimal strings (e.g., `"0.5000"`, `"10.500000"`) for JSON compatibility with Polymarket's API.

### Zero-copy WebSocket deserialization

The `WsMessage<'a>` enum uses `#[serde(borrow)]` and `&'a str` fields so that high-frequency WebSocket messages can be deserialized without allocating strings. The borrowed lifetime ties parsed fields directly to the input buffer. This matters because the feed processes thousands of messages per second.

Owned types (`RestBookResponse`, `GammaEvent`) are used for REST endpoints where zero-copy is impractical (responses are buffered and the data outlives the buffer).

### Custom serde for fixed-point types

`FixedPrice` and `FixedSize` implement `Serialize`/`Deserialize` manually to serialize as decimal strings rather than raw integers. This ensures JSON output matches Polymarket's format and allows direct deserialization from API responses.

### Error design

`TypesError` uses `thiserror` with variants for invalid prices (`InvalidPrice`), parse failures (`PriceParse`, `SizeParse`), invalid sides (`InvalidSide`), and deserialization errors (wrapping `serde_json::Error`). The `From<serde_json::Error>` impl enables `?` propagation from wire parsing.

## Implementation Details

### FixedPrice boundaries

`FixedPrice::new(raw)` rejects values above 10,000 with `TypesError::InvalidPrice`. `FixedPrice::from_f64(v)` rounds to the nearest integer after scaling, then delegates to `new()`. The `TryFrom<&str>` impl parses the string as `f64` first, then calls `from_f64`.

### Sequence gap detection

`Sequence(u64)` is a monotonically increasing counter. The `next()` method returns `self + 1`. Gap detection logic lives in consumers (e.g., `L2Book::check_sequence`) rather than in the type itself, keeping the newtype minimal.

### Wire message discrimination

`WsMessage<'a>` uses `#[serde(tag = "event_type")]` for internally-tagged enum deserialization. Variants map to `"book"`, `"price_change"`, and `"last_trade_price"` strings from the Polymarket WebSocket protocol.
