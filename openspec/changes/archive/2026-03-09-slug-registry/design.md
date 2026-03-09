## Context

Polymarket token IDs are 70+ digit CTF position integers derived from ERC-1155 conditional token positions on Polygon. The system currently passes these raw strings through every layer: CLI args, WebSocket subscriptions, Parquet partitions, API routes, and log output.

The Gamma API — already queried during `discover` and `auto-ingest` — returns rich market metadata including slugs (e.g. `btc-updown-5m-1741500000`), question text, and market active status. Today, `extract_token_ids()` in `market_discovery.rs` extracts only the raw token IDs and discards everything else.

The system is single-operator (one workstation user), not multi-tenant. The registry only needs to hold the assets currently active or recently discovered — not a global mapping of all Polymarket markets.

## Goals / Non-Goals

**Goals:**
- Users can reference assets by slug instead of 70-digit token IDs in CLI and API
- All existing full-token-ID inputs continue to work unchanged
- Registry is populated automatically from Gamma API data already being fetched
- API responses include slug alongside token ID for frontend display
- Explicit lookup endpoint for slug resolution

**Non-Goals:**
- Persistent slug storage (registry is in-memory, rebuilt on startup from discovery)
- Supporting arbitrary user-defined aliases
- Changing Parquet partition keys or ClickHouse schema (storage remains token-ID keyed)
- Global registry of all Polymarket markets (only discovered/active assets)
- Slug collision handling across different market types (BTC 5m markets have unique timestamp-based slugs)

## Decisions

### D1: Registry lives in pb-types, not a new crate

**Decision**: Add `SlugRegistry` to `pb-types` as a new module.

**Rationale**: The registry is a simple bidirectional `FxHashMap` with no external dependencies beyond what `pb-types` already has. Creating a `pb-slug` crate adds workspace overhead for ~100 lines of code. Both `pb-feed` (producer) and `pb-api` (consumer) already depend on `pb-types`.

**Alternative considered**: New `pb-slug` crate — rejected for being over-engineered given the scope.

### D2: Slug format uses Gamma API slug field directly

**Decision**: Use the slug from `GammaMarket` as-is (e.g. `btc-updown-5m-1741500000`). For markets with YES/NO token pairs, append `-yes` / `-no` suffix to distinguish the two tokens within a market.

**Rationale**: Gamma slugs are already URL-safe, unique per market, and meaningful. Inventing a custom scheme adds maintenance burden. The `-yes`/`-no` suffix handles the common case where a single market slug maps to two CLOB token IDs.

**Alternative considered**: Blake3 short hash — deterministic but not human-readable, defeats the UX purpose.

### D3: Resolution is transparent via a shared resolve function

**Decision**: A single `SlugRegistry::resolve(&self, input: &str) -> Option<AssetId>` method handles both slugs and raw token IDs. If the input is >40 characters and all digits, treat it as a raw token ID. Otherwise, look up as slug.

**Rationale**: Callers don't need to know whether the user passed a slug or token ID. This makes adoption incremental — existing code paths work unchanged, slug support is additive.

### D4: Registry is shared via `Arc<SlugRegistry>` with interior mutability

**Decision**: `SlugRegistry` uses `RwLock<Inner>` internally so it can be populated during discovery/auto-ingest and read concurrently by the API server. Wrapped in `Arc` for cheap cloning into axum state and CLI contexts.

**Rationale**: The registry is written rarely (on discovery/rotation, every ~5 minutes) and read frequently (every API request). `RwLock` gives zero-contention reads. This is not on the hot data path (book updates), so the lock overhead is negligible.

**Alternative considered**: Immutable registry rebuilt on each rotation — simpler but requires `watch::channel` plumbing and doesn't support the API needing to resolve slugs for historical assets no longer active.

### D5: GammaMarket wire type gains a `slug` field

**Decision**: Add `pub slug: Option<String>` to the existing `GammaMarket` wire type in `pb-types`. The Gamma API already returns this field; we just aren't deserializing it.

**Rationale**: Minimal change — one field addition to an existing struct that's only used during discovery.

### D6: No Parquet/ClickHouse schema changes

**Decision**: Storage layers continue using raw token IDs as partition keys and column values. Slug resolution happens at the API/CLI boundary only.

**Rationale**: Slug-to-token mapping can change (markets expire, new ones appear). Embedding slugs in immutable Parquet files would create stale references. The registry resolves at query time, keeping storage stable.

## Risks / Trade-offs

- **[Stale slugs after market expiry]** → Slugs for expired markets remain in the in-memory registry until process restart. Acceptable for a single-operator workstation; not a correctness issue since the underlying token ID is still valid for historical queries.

- **[Registry lost on restart]** → In-memory only; rebuilt from next discovery or auto-ingest cycle. For `serve-api` with `--tokens` (no discovery), slugs won't be available until an explicit `/resolve` or discovery run populates them. This is acceptable — the primary use case (auto-ingest) always discovers first.

- **[Gamma API slug format changes]** → We depend on Gamma API's slug convention. If they change the format, our slugs change too. Low risk — slug format has been stable and we don't persist them.

- **[Two tokens per market]** → BTC 5m markets have YES and NO tokens. The `-yes`/`-no` suffix convention must be documented and consistent. The resolve function must handle both `btc-updown-5m-1741500000-yes` and the full token ID.
