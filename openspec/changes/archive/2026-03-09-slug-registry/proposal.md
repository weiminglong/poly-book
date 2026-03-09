## Why

Polymarket token IDs are 70+ digit CTF position integers (e.g. `21742633143463801764263866138596936600980228888098934498299596572218858267895`). They are unusable in CLI commands, URL paths, log output, and frontend display. Every user-facing surface — API routes, `--tokens` CLI args, log lines, Parquet partition browsing — requires copy-pasting opaque numeric strings. The system already fetches market metadata from the Gamma API during discovery and auto-ingest, but discards the human-readable slug and question text immediately after extracting raw token IDs.

## What Changes

- Introduce a `SlugRegistry` that maps human-readable slugs to full token IDs (bidirectional)
- Populate the registry during market discovery and auto-ingest from Gamma API metadata already being fetched
- Accept either slug or full token ID in all user-facing inputs (API path/query params, CLI `--tokens` args)
- Enrich API response DTOs with an optional `slug` field alongside the existing `asset_id`
- Add a `/api/v1/assets/resolve` endpoint for explicit slug-to-token-ID lookup
- Log slugs alongside token IDs in tracing output for debuggability

## Capabilities

### New Capabilities
- `slug-resolution`: Bidirectional slug-to-token-ID registry with transparent resolution in API and CLI inputs
- `asset-metadata-enrichment`: Enrich API responses and active-asset listings with slug and market context from discovery

### Modified Capabilities

## Impact

- **pb-types**: New `SlugRegistry` struct (or new `pb-slug` crate if isolation preferred)
- **pb-feed**: Gamma API discovery returns richer metadata; `extract_token_ids` returns slug mappings
- **pb-api**: API path/query extractors resolve slugs transparently; response DTOs gain `slug` field
- **pb-bin**: CLI `--tokens` accepts slugs; `auto-ingest` and `ingest` populate registry; `discover` prints slug mappings
- **pb-store / pb-replay**: No schema changes — storage continues to use raw token IDs as partition keys
- **No breaking changes**: All existing full-token-ID inputs continue to work; slug is additive
