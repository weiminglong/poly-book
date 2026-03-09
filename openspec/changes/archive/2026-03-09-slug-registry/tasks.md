## 1. Core SlugRegistry in pb-types

- [x] 1.1 Add `slug` module to `pb-types` with `SlugRegistry` struct: inner `FxHashMap<String, AssetId>` (slug→token) and `FxHashMap<AssetId, String>` (token→slug), wrapped in `Arc<RwLock<_>>`
- [x] 1.2 Implement `register(&self, slug: &str, asset_id: &AssetId)`, `resolve(&self, input: &str) -> Option<AssetId>`, `slug_for(&self, asset_id: &AssetId) -> Option<String>`, `register_market(&self, base_slug: &str, token_ids: &[String])` (handles YES/NO suffix)
- [x] 1.3 Add unit tests: register + resolve round-trip, raw token ID passthrough (>40 digits), unknown slug returns None, YES/NO suffix generation for two-token markets

## 2. Gamma API Metadata Extraction in pb-feed

- [x] 2.1 Add `pub slug: Option<String>` field to `GammaMarket` wire type in `pb-types/src/wire.rs`
- [x] 2.2 Introduce `SlugMapping { slug: String, token_ids: Vec<String> }` return type and refactor `extract_token_ids` in `market_discovery.rs` to also return slug mappings alongside token IDs
- [x] 2.3 Add optional `label` metadata extraction from `GammaMarket` question field for human-readable display

## 3. Registry Population in pb-bin Commands

- [x] 3.1 Thread `Arc<SlugRegistry>` through `auto_ingest::run` — populate after each `discover_with_retry` call using the new slug mappings
- [x] 3.2 Update `discover::run` to populate registry and print slug→token mappings in CLI output
- [x] 3.3 Update `ingest` and `backfill` commands to accept slugs in `--tokens` by resolving through registry (requires a pre-discovery step or passthrough for raw IDs)
- [x] 3.4 Thread registry into `serve-api` command so the API server has access

## 4. API Slug Resolution in pb-api

- [x] 4.1 Add `SlugRegistry` to `AppState` and pass through from `pb-bin` serve-api setup
- [x] 4.2 Update `orderbook_snapshot` handler to resolve `asset_id` path param through registry before lookup
- [x] 4.3 Update `replay_reconstruct` handler to resolve `asset_id` query param through registry
- [x] 4.4 Update `integrity_summary` and `execution_orders` handlers to resolve asset_id through registry
- [x] 4.5 Add `GET /api/v1/assets/resolve?q={input}` endpoint with `AssetResolveResponse` DTO

## 5. Response DTO Enrichment in pb-api

- [x] 5.1 Add `slug: Option<String>` to `LiveOrderBookSnapshot`, `ReplayReconstructionResponse`, `IntegritySummaryResponse`, `ActiveAssetSummary`, and `BookUpdateMessage` DTOs
- [x] 5.2 Add `label: Option<String>` to `ActiveAssetSummary` DTO
- [x] 5.3 Enrich response construction in each handler to look up slug from registry before returning
- [x] 5.4 Update `FeedStatusResponse.active_assets` from `Vec<String>` to `Vec<AssetRef>` with `{ asset_id, slug }` (or keep backward-compatible by adding a parallel `active_assets_detail` field)

## 6. Logging and Observability

- [x] 6.1 Update auto-ingest rotation log line to include `slug` span field alongside `tokens`
- [x] 6.2 Add slug context to API request tracing spans when an asset is resolved

## 7. Tests and Validation

- [x] 7.1 Add API integration tests: snapshot-by-slug, resolve endpoint, unknown slug 404
- [x] 7.2 Verify existing tests pass with raw token IDs (backward compatibility)
- [x] 7.3 Update `docs/api.md` with new `/assets/resolve` route and slug field documentation
- [x] 7.4 Update `docs/serve-api.md` to document slug resolution behavior
