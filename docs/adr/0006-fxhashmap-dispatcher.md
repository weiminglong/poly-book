# ADR-0006: FxHashMap for Dispatcher Lookup Tables

## Status
Accepted

## Context
The Dispatcher maintains several `HashMap` lookup tables:
- `asset_sequences`: per-asset sequence counter
- `last_snapshot_ts`: per-asset last snapshot timestamp
- `asset_id_cache`: interned AssetId values

These maps are keyed by `Arc<str>` (asset IDs). The standard `HashMap` uses
SipHash, which is designed to resist HashDoS attacks at the cost of ~20 ns per
hash. Since the Dispatcher is an internal component processing trusted data
(not exposed to untrusted input), cryptographic hash resistance is unnecessary.

## Decision
Replace `HashMap` with `FxHashMap` (from `rustc-hash`) in the Dispatcher.
FxHashMap uses FxHash, a fast non-cryptographic hash function used inside the
Rust compiler.

## Consequences
- **Performance**: ~3–5x faster hash computation for short string keys.
  Meaningful when hashing on every message in the hot path.
- **Security**: FxHash is not DoS-resistant. Acceptable because the
  Dispatcher processes data from a known upstream (Polymarket WebSocket), not
  arbitrary user input.
- **Scope**: Only used in the Dispatcher's internal lookup tables. External-
  facing data structures (API, storage) continue to use standard HashMap or
  BTreeMap as appropriate.
