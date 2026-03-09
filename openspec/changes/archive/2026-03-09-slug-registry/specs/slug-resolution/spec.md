## ADDED Requirements

### Requirement: SlugRegistry provides bidirectional slug-token mapping
The system SHALL maintain an in-memory `SlugRegistry` that maps human-readable slugs to `AssetId` values and vice versa. The registry SHALL support concurrent reads and infrequent writes via `Arc<RwLock<_>>`.

#### Scenario: Register a slug-token pair
- **WHEN** a slug `"btc-updown-5m-1741500000-yes"` is registered with token ID `"2174263314346380...7895"`
- **THEN** `registry.resolve("btc-updown-5m-1741500000-yes")` SHALL return the corresponding `AssetId`
- **AND** `registry.slug_for(&asset_id)` SHALL return `Some("btc-updown-5m-1741500000-yes")`

#### Scenario: Register multiple tokens from one market
- **WHEN** a market with slug `"btc-updown-5m-1741500000"` has two CLOB token IDs (YES and NO)
- **THEN** the registry SHALL contain entries for both `"btc-updown-5m-1741500000-yes"` and `"btc-updown-5m-1741500000-no"`

### Requirement: Resolve function accepts both slugs and raw token IDs
The `SlugRegistry::resolve` method SHALL transparently handle both slug strings and full token ID strings. If the input is longer than 40 characters and consists entirely of digits, it SHALL be treated as a raw token ID. Otherwise, it SHALL be looked up as a slug.

#### Scenario: Resolve a slug
- **WHEN** `resolve("btc-updown-5m-1741500000-yes")` is called and the slug is registered
- **THEN** the method SHALL return `Some(AssetId)` with the corresponding full token ID

#### Scenario: Resolve a raw token ID
- **WHEN** `resolve("21742633143463801764263866138596936600980228888098934498299596572218858267895")` is called
- **THEN** the method SHALL return `Some(AssetId)` wrapping the input directly, regardless of registry contents

#### Scenario: Resolve an unknown slug
- **WHEN** `resolve("unknown-slug")` is called and no such slug is registered
- **THEN** the method SHALL return `None`

### Requirement: CLI --tokens flag accepts slugs
All CLI commands that accept `--tokens` (ingest, backfill, replay, serve-api) SHALL resolve each comma-separated value through `SlugRegistry::resolve` before use. Raw token IDs SHALL continue to work unchanged.

#### Scenario: Ingest with slug
- **WHEN** `pb ingest --tokens btc-updown-5m-1741500000-yes` is run after discovery has populated the registry
- **THEN** the ingest command SHALL subscribe to the WebSocket using the resolved full token ID

#### Scenario: Ingest with raw token ID
- **WHEN** `pb ingest --tokens 21742633143463801764263866138596936600980228888098934498299596572218858267895` is run
- **THEN** the command SHALL work exactly as it does today

### Requirement: API routes accept slugs in asset_id parameters
The API server SHALL resolve `asset_id` path parameters and query parameters through `SlugRegistry::resolve`. Both slugs and full token IDs SHALL be accepted.

#### Scenario: Snapshot by slug
- **WHEN** `GET /api/v1/orderbooks/btc-updown-5m-1741500000-yes/snapshot` is requested
- **THEN** the server SHALL resolve the slug to the full token ID and return the orderbook snapshot

#### Scenario: Replay reconstruct by slug
- **WHEN** `GET /api/v1/replay/reconstruct?asset_id=btc-updown-5m-1741500000-yes&at_us=...&mode=recv_time` is requested
- **THEN** the server SHALL resolve the slug and reconstruct against the correct token ID's Parquet data

#### Scenario: API returns 404 for unknown slug
- **WHEN** `GET /api/v1/orderbooks/unknown-slug/snapshot` is requested and the slug is not registered
- **THEN** the server SHALL return 404 with error message indicating the asset was not found

### Requirement: Registry populated during discovery and auto-ingest
The `discover` and `auto-ingest` commands SHALL populate the `SlugRegistry` with slug-token mappings extracted from Gamma API responses. The `GammaMarket` wire type SHALL include a `slug` field deserialized from the API response.

#### Scenario: Auto-ingest rotation populates registry
- **WHEN** auto-ingest rotates to a new 5-minute market via `discover_with_retry`
- **THEN** the registry SHALL contain slug entries for all token IDs discovered in that rotation

#### Scenario: Discover command populates registry
- **WHEN** `pb discover` is run
- **THEN** the registry SHALL be populated with all slug-token mappings from the discovery results
- **AND** the CLI output SHALL display slugs alongside token IDs

### Requirement: Asset resolve endpoint
The API SHALL expose `GET /api/v1/assets/resolve?q={slug_or_token_id}` that returns the resolution result.

#### Scenario: Resolve known slug
- **WHEN** `GET /api/v1/assets/resolve?q=btc-updown-5m-1741500000-yes` is requested
- **THEN** the response SHALL include `{ "asset_id": "<full_token_id>", "slug": "btc-updown-5m-1741500000-yes", "found": true }`

#### Scenario: Resolve unknown input
- **WHEN** `GET /api/v1/assets/resolve?q=nonexistent` is requested
- **THEN** the response SHALL include `{ "found": false }`
