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
| `guard_sql` | Shared query-guard entrypoint that validates read-only SQL and injects `LIMIT`. |
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
- `QueryGuard` enforces a single read-only SQL statement rooted at
  `SELECT`/`WITH`/`SHOW`/`DESCRIBE`/`EXPLAIN`, strips comments and quoted
  literals before keyword checks, injects `LIMIT` if missing, and applies a
  configurable timeout.
- `guard_sql` is reusable outside the ClickHouse adapter, so tests and fuzz
  targets exercise the same sanitizer and normalization path the runtime uses.
- The guard rejects an identifier blocklist of I/O table functions (`file`,
  `url`, `s3`, `remote`, `mysql`, …), the `system` database, and exfiltration
  clauses (`INTO OUTFILE`, `SETTINGS`) so a SELECT-rooted query cannot be an
  SSRF / arbitrary-file-read primitive.
- `ClickHouseQueryService` uses the ClickHouse HTTP API with `JSONCompact` format
  for dynamic SQL execution, and enforces `readonly=2`, `max_result_rows`, and
  `max_execution_time` server-side as defense-in-depth; the whole request
  (send + body download) is bounded by one timeout. The API clamps the
  client-supplied row cap to the configured ceiling.
- Shared helpers are extracted into `lib.rs` to eliminate duplicated business
  logic between Parquet and ClickHouse backends: `map_replay_error`,
  `ingest_to_continuity`, `build_replay_result`, `build_integrity_summary`,
  `assemble_integrity_summary`, `build_execution_timeline`.
- `ClickHouseIntegrityService::summary` computes its counts server-side: it calls
  `ClickHouseReader::read_integrity_aggregates` (`count()`/`countIf` over
  `book_events` and `replay_validations`) plus `read_ingest_events` (the bounded
  continuity list), then `assemble_integrity_summary`. It never materializes the
  full book/trade window just to count it (audit A.42). The Parquet backend, which
  already holds the full window, still uses `build_integrity_summary`.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New service trait or method | `pb-api` handler, `docs/api.md` route docs |
| New domain type | `pb-api` DTO mapping, `docs/api.md` response schemas |
| New backend implementation | Enum dispatch variant, `pb-bin` `build_services()` |
| Error variant added | `pb-api` `ServiceError → ApiError` mapping |
| Changes affect API contracts | `docs/api.md`, `docs/serve-api.md` |
| New query guard rule or type | `pb-api` query handler, `docs/api.md` query routes |

## Tests

51 tests covering shared helper functions (error mapping, continuity gap detection,
replay result construction, integrity summary building, execution timeline ordering),
query guard edge cases, and backend-specific service logic.
