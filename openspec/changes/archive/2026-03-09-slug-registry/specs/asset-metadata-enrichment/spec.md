## ADDED Requirements

### Requirement: API responses include slug field
All API response DTOs that contain an `asset_id` field SHALL also include an optional `slug` field. The slug SHALL be populated by looking up the asset ID in the `SlugRegistry`.

#### Scenario: Live snapshot response includes slug
- **WHEN** `GET /api/v1/orderbooks/{asset_id}/snapshot` returns successfully
- **THEN** the `LiveOrderBookSnapshot` response SHALL include `"slug": "btc-updown-5m-1741500000-yes"` if the asset has a registered slug, or `"slug": null` if not

#### Scenario: Replay response includes slug
- **WHEN** `GET /api/v1/replay/reconstruct` returns successfully
- **THEN** the `ReplayReconstructionResponse` SHALL include the `slug` field

#### Scenario: Integrity summary includes slug
- **WHEN** `GET /api/v1/integrity/summary` returns successfully
- **THEN** the `IntegritySummaryResponse` SHALL include the `slug` field

### Requirement: Active asset listing includes slug and label
The `ActiveAssetSummary` DTO SHALL include optional `slug` and `label` fields. The label SHALL be a human-readable market description derived from Gamma API metadata (e.g. `"BTC 5m UP 2026-03-09 14:00"`).

#### Scenario: Active assets list with metadata
- **WHEN** `GET /api/v1/assets/active` is requested
- **THEN** each `ActiveAssetSummary` SHALL include `slug` (from registry) and `label` (from stored metadata) if available

#### Scenario: Active assets without discovery metadata
- **WHEN** `serve-api` is started with `--tokens` (no discovery) and no slug registry is populated
- **THEN** the `slug` and `label` fields SHALL be `null` and the response SHALL still include the full `asset_id`

### Requirement: Feed status response includes slug-enriched asset list
The `FeedStatusResponse` `active_assets` field SHALL include slug information alongside raw token IDs.

#### Scenario: Feed status shows slugs
- **WHEN** `GET /api/v1/feed/status` is requested and the registry has slug mappings
- **THEN** the `active_assets` list SHALL include objects with both `asset_id` and `slug` fields instead of plain strings

### Requirement: Log output includes slug context
Structured log lines that reference an asset SHALL include the slug as a tracing span field when available, alongside the token ID.

#### Scenario: Auto-ingest rotation log includes slug
- **WHEN** auto-ingest logs a market rotation event
- **THEN** the tracing output SHALL include both `slug` and `tokens` fields (e.g. `slug="btc-updown-5m-1741500000" tokens=["2174..."]`)

#### Scenario: API request log includes slug
- **WHEN** an API request references an asset by slug or token ID
- **THEN** the request tracing span SHALL include both the resolved `asset_id` and the `slug` if known

### Requirement: WebSocket streaming messages include slug
The `BookUpdateMessage` sent via the WebSocket streaming endpoint SHALL include an optional `slug` field.

#### Scenario: WebSocket update with slug
- **WHEN** a book update is broadcast to WebSocket subscribers
- **THEN** the `BookUpdateMessage` SHALL include `"slug": "btc-updown-5m-1741500000-yes"` if the asset has a registered slug
