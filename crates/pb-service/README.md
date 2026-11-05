# pb-service

Transport-neutral domain service layer. `HistoricalService` wraps the Parquet
reader from `pb-replay` and answers the historical questions the workstation
asks, decoupling business logic from HTTP transport. The `pb-api` crate uses it
as a thin adapter (parse HTTP → call service → format response).

## Key Types

| Type | Description |
|------|-------------|
| `HistoricalService` | Parquet-backed replay reconstruction and integrity summaries. Constructors: `new(base_path)` (local-path convenience) and `from_reader(ParquetReader)` (share one configured local/S3 object-store reader). |
| `ReplayResult` | Historical reconstruction result: top-of-book, top-N levels, depth counts, whether a checkpoint was used, and continuity events. |
| `IntegritySummary` | Event counts, reconnect/gap/stale-skip counts, validation counts, completeness level, continuity events. |
| `CompletenessLevel` | `Empty`, `Partial`, or `Full`; the API maps these to `complete`/`best_effort` labels. |
| `ContinuityEvent` | Structured reconnect/gap/stale event from the data layer. |
| `ServiceError` | Domain error with variants: `NotFound`, `InvalidParams`, `Unavailable`, `Internal`. |

## Methods

| Method | Description |
|--------|-------------|
| `reconstruct(asset_id, at_us, mode, depth)` | Rebuilds the book at `at_us` via `ReplayEngine` and trims to `depth` levels per side. |
| `integrity_summary(asset_id, start_us, end_us)` | Counts events and validations over the window and labels completeness. Enforces the 24-hour window cap via `validate_time_window` (`MAX_QUERY_WINDOW_US`). |

## Data Flow

```text
HTTP request
    │
    ▼
pb-api handler (thin adapter)
    │
    ▼
HistoricalService::{reconstruct, integrity_summary}
    │
    ▼
ParquetReader (pb-replay) ──▶ Parquet files (local FS / S3)
```

`pb-bin` builds the service once via `pipeline::build_historical_service`
and stores it in `pb_api::AppState::historical`.

## Design Notes

- The service defines the domain contract; HTTP concerns stay in `pb-api`.
- `ServiceError` maps cleanly to HTTP status codes at the `pb-api` boundary
  (`map_replay_error` translates `ReplayError` variants).
- Shared helpers in `lib.rs` (`ingest_to_continuity`, `build_replay_result`,
  `build_integrity_summary`) turn raw `pb-replay` output into the domain types.
- Local files and cloud object stores share the same reader path; recovered
  partitions are selected through recovery manifests below this layer.

## Tests

Unit tests cover error mapping, continuity gap detection, replay result
construction, integrity summary building, and time-window validation.
