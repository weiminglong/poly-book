# pb-types-foundation

## Why

All crates in the poly-book workspace need a shared vocabulary of fixed-point numeric types, wire-format deserialization structs, and normalized orderbook events. Establishing these foundational types first ensures consistency and eliminates floating-point drift across the entire pipeline.

## What Changes

- Introduce `FixedPrice(u32)` with scale factor 10,000 for Polymarket's 0.00-1.00 price range
- Introduce `FixedSize(u64)` with scale factor 1,000,000 for order sizes
- Add `AssetId` and `Sequence` newtypes for domain clarity and type safety
- Add zero-copy `WsMessage<'a>` enum for WebSocket deserialization (book, price_change, last_trade_price)
- Add owned REST/Gamma API response types (`RestBookResponse`, `GammaEvent`)
- Define `OrderbookEvent` struct and `Side`/`EventType` enums for normalized event representation
- Define `TypesError` enum via `thiserror` for parse and validation errors

## Capabilities

### New Capabilities

- `fixed-point-arithmetic`: `FixedPrice(u32)` and `FixedSize(u64)` with scaled integer representation, f64 conversion, string parsing, serde roundtrip, and `Ord` derivation
- `wire-deserialization`: Zero-copy serde types (`WsMessage<'a>`, `BookMessage<'a>`, `PriceChangeMessage<'a>`) for Polymarket WebSocket messages, plus owned types for REST and Gamma APIs
- `orderbook-events`: Normalized `OrderbookEvent` struct with `Side`, `EventType`, `FixedPrice`, `FixedSize`, `AssetId`, and `Sequence` tracking
- `domain-newtypes`: `AssetId(String)` and `Sequence(u64)` for type-safe identifiers and gap detection

### Modified Capabilities

(none)

## Impact

- **Downstream crates**: `pb-book`, `pb-feed`, `pb-store`, `pb-replay`, `pb-metrics`, and `pb-bin` all depend on `pb-types`
- **No external API surface**: This is an internal library crate only
- **Zero runtime dependencies beyond serde/thiserror**: Keeps the dependency tree minimal
