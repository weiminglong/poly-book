# ADR-0004: Zero-Copy Deserialization for Wire Protocol

## Status
Accepted

## Context
WebSocket messages arrive as raw text. Each message must be deserialized,
normalized, and forwarded. Allocating owned Strings for every field (asset_id,
price, size, hash) creates significant allocation pressure at high message
rates.

## Decision
Wire types (`WsMessage<'a>`, `BookMessage<'a>`, `OrderEntry<'a>`) borrow from
the raw buffer using `&'a str` and `#[serde(borrow)]`. The raw buffer lives
until the Dispatcher has finished processing the message.

## Consequences
- **Allocation reduction**: Price, size, asset_id, and hash fields are
  borrowed slices into the original JSON buffer. No heap allocation for these
  fields during deserialization.
- **Throughput**: ~2–5x fewer allocations per message compared to owned
  `String` fields. Measured via wire_deser benchmarks.
- **Lifetime constraint**: Wire types cannot outlive the raw buffer. The
  Dispatcher must convert to owned types (FixedPrice, AssetId) before sending
  downstream. This is intentional — it forces explicit materialization at the
  normalization boundary.
- **Interning**: The Dispatcher caches `AssetId` values to avoid repeated
  Arc allocation for the same token ID. Combined with zero-copy deser, this
  means the typical hot path allocates only when seeing a new asset for the
  first time.
