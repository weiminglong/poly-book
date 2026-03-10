# pb-service

Transport-neutral domain service layer. Defines service traits that decouple
business logic from HTTP transport. The `pb-api` crate uses these as thin
adapters (parse HTTP → call service → format response).

## Service Traits

| Trait | Methods | Description |
|-------|---------|-------------|
| `BookService` | `feed_status`, `active_assets`, `is_asset_active`, `snapshot` | Live book queries against the watch-based read model. |
| `ReplayService` | `reconstruct` | Historical order book reconstruction at a specific timestamp. |
| `IntegrityService` | `summary` | Data integrity and completeness assessment over a time range. |
| `ExecutionService` | `timeline` | Execution event timeline queries with asset/order filters. |
| `QueryService` | `execute_sql`, `list_datasets` | Guarded read-only SQL execution and dataset schema discovery. |

## Concrete Implementations

| Backend | Services |
|---------|----------|
| Parquet | `ParquetReplayService`, `ParquetIntegrityService`, `ParquetExecutionService` |
| ClickHouse | `ClickHouseReplayService`, `ClickHouseIntegrityService`, `ClickHouseExecutionService`, `ClickHouseQueryService` |

## Enum Dispatch

Service traits use `impl Future` return types, making them not dyn-compatible.
Backend polymorphism uses enum dispatch instead:

```rust
pub enum AnyReplayService {
    Parquet(ParquetReplayService),
    ClickHouse(ClickHouseReplayService),
}
impl ReplayService for AnyReplayService { ... }
```

Same pattern for `AnyIntegrityService`, `AnyExecutionService`, and `AnyQueryService`.

## Domain Types

| Type | Description |
|------|-------------|
| `FeedStatus` | Feed connection status, active assets, rotation info. |
| `AssetSummary` | Per-asset staleness and book availability. |
| `BookSnapshot` | Full order book snapshot with spread, depth, levels. |
| `ReplayResult` | Historical reconstruction result with continuity events. |
| `IntegritySummary` | Event counts, completeness level, continuity events. |
| `ExecutionTimeline` | Execution events with total count. |
| `ContinuityEvent` | Structured reconnect/gap/stale event from the data layer. |
| `QueryGuard` | Guard rails for query execution (max rows, timeout). |
| `QueryResult` | Query result with columns, rows, truncation flag, execution time. |
| `QueryColumnInfo` | Column metadata (name and data type). |
| `DatasetSchema` | Dataset schema with name, description, and columns. |
| `ServiceError` | Domain error with variants: NotFound, InvalidParams, Unavailable, Internal. |

## Data Flow

```text
HTTP request
    │
    ▼
pb-api handler (thin adapter)
    │
    ▼
pb-service trait method
    │
    ├──▶ ParquetReader (pb-replay) ──▶ Parquet files
    │
    ├──▶ ClickHouseReader (pb-replay) ──▶ ClickHouse tables
    │
    └──▶ ClickHouseQueryService ──▶ ClickHouse HTTP API (read-only SQL)
```

## Design Notes

- Service traits define the domain contract; HTTP concerns stay in `pb-api`.
- `ServiceError` maps cleanly to HTTP status codes at the `pb-api` boundary.
- Backend selection is configured via `api.historical_backend` in config.
- If ClickHouse is unavailable at startup, the system falls back to Parquet.
- `QueryGuard` enforces read-only SQL (rejects write keywords), injects `LIMIT` if missing, and applies a configurable timeout.
- `ClickHouseQueryService` uses the ClickHouse HTTP API with `JSONCompact` format for dynamic SQL execution.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New service trait or method | `pb-api` handler, `docs/api.md` route docs |
| New domain type | `pb-api` DTO mapping, `docs/api.md` response schemas |
| New backend implementation | Enum dispatch variant, `pb-bin` `build_services()` |
| Error variant added | `pb-api` `ServiceError → ApiError` mapping |
| Changes affect API contracts | `docs/api.md`, `docs/serve-api.md` |
| New query guard rule or type | `pb-api` query handler, `docs/api.md` query routes |
