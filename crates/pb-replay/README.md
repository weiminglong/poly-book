# pb-replay

Historical replay and orderbook reconstruction engine. Reads stored events from
Parquet or ClickHouse, rebuilds the L2 order book at any microsecond timestamp,
and supports periodic REST snapshot backfill for checkpoint seeding.

## Key Types

| Type | Description |
|------|-------------|
| `ReplayEngine` | Reconstructs `L2Book` at a target timestamp by reading checkpoints and applying events forward. |
| `ReplayResult` | Output of reconstruction: the rebuilt `L2Book`, replay mode, whether a checkpoint was used, and continuity (ingest) events encountered. |
| `EventReader` | Trait abstracting storage backends for reading events by time range and asset. |
| `ParquetReader` | `EventReader` implementation for local/S3 Parquet files. |
| `ClickHouseReader` | `EventReader` implementation for ClickHouse tables. |
| `BackfillConfig` | Configuration for periodic REST snapshot fetching. |
| `ReplayError` | Error type for replay operations. |

## Data Flow

```text
Parquet files / ClickHouse tables
        │
        ▼
  EventReader (ParquetReader or ClickHouseReader)
        │
        ▼
  ReplayEngine::reconstruct_at(asset_id, target_us)
        │
        ▼
  ReplayResult { book, mode, used_checkpoint, continuity_events }
        │
        ▼
  pb-api (replay/reconstruct, integrity/summary, execution/orders)
```

Also: `run_backfill` periodically fetches REST snapshots and writes them as
`BookCheckpoint` events to seed future replay reconstructions.

## Design Notes

- Reconstruction starts from the nearest checkpoint before the target timestamp,
  then only reads market data from that checkpoint timestamp forward instead of
  replaying the full lookback window.
- `SourceReset` is treated as a hard continuity boundary during replay. If the
  latest reset precedes the target, replay ignores older checkpoints/snapshots
  and requires a fresh post-reset snapshot before applying later deltas.
- The `EventReader` trait has methods for reading each dataset type:
  `read_market_data`, `read_checkpoints`, `read_latest_checkpoint`,
  `read_validations`, `read_execution_events`.
- Both readers support time-range filtering and asset filtering at the storage
  layer to minimize I/O. ClickHouseReader pushes `WHERE asset_id` into the query
  for server-side filtering and adds `ORDER BY` clauses on all queries.
- ClickHouseReader uses `tokio::try_join!` to run independent queries concurrently
  (e.g., checkpoints + market data) instead of sequential awaits.
- For integrity summaries, `ClickHouseReader::read_integrity_aggregates` pushes the
  heavy counts to the server (`count()` on `book_events`, `count()`/`countIf(matched)`
  on `replay_validations`) and `read_ingest_events` fetches only the bounded ingest
  list. This avoids streaming every book/trade row back just to call `.len()` on
  it; the two aggregate queries run concurrently via `try_join!`. The
  returned `IntegrityAggregates` carries `book_event_count`/`validation_count`/
  `validation_match_count`.
- Unbounded ClickHouse reads (`read_ingest_events`, `read_execution_events`) go
  through `bounded_client()`, which sets `max_result_rows = MAX_READ_ROWS` (5M) +
  `result_overflow_mode = 'throw'`, so a pathological window ERRORS loudly instead
  of materializing millions of rows and OOM-ing the serve process. Verified
  against a live server (TOO_MANY_ROWS_OR_BYTES on overflow).
- Uses `std::mem::take` instead of `clone` for ingest events to avoid unnecessary
  heap allocation during reconstruction.
- Date formatting uses `Datelike`/`Timelike` trait methods instead of `strftime`
  for efficiency in Parquet hour-path generation.
- Replay validation seeds reconstruction from the checkpoint *strictly before*
  the reference checkpoint and replays deltas forward to it, then compares
  against the independent reference. It must never seed from the reference
  itself, or `matched` is trivially always true.
- **Deterministic ordering**: `sort_book_events` imposes a *total* order —
  timestamp (clock-domain), then `ingest_ordinal` (the authoritative
  arrival-order tiebreaker stamped at ingest), then `sequence`, then content
  tiebreakers (side, price, size, source event id). Parquet files are read
  concurrently and may arrive out of order (`buffer_unordered`), so this total
  order is what makes two replays of the same window byte-identical.
  Because `ingest_ordinal` is monotonic in true arrival order (unlike `sequence`,
  which resets to 0 on each snapshot), a same-microsecond *pre-snapshot* delta
  now sorts before its snapshot. Legacy rows without an ordinal fall back
  to `sequence` + content tiebreakers.
- Replay never mutates live observability: a sequence gap found during
  reconstruction is recorded in the returned continuity events, not pushed to the
  live `pb_gaps_detected_total` recorder.
- **Single clock domain at the checkpoint boundary**: `checkpoint_timestamp_us`
  is an exchange-clock value, so the post-checkpoint cutoff projects the
  checkpoint into the active replay clock (`checkpoint_ordering_ts`) before
  comparing it against events — in RecvTime mode it uses the checkpoint's recv
  timestamp. Without this, recv-vs-exchange skew could skip or double-apply deltas
  straddling the boundary.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New `EventReader` method | Both `ParquetReader` and `ClickHouseReader` must implement it |
| Reconstruction logic changed | `docs/serve-api.md` replay semantics section |
| New storage backend reader | `docs/operations.md`, `pb-api` if the reader is used in routes |
| Backfill config shape changed | `config/default.toml`, `docs/operations.md` |
| Replay semantics affect API responses | `docs/api.md`, check the active OpenSpec change under `openspec/changes/` |

## Tests

27 tests covering `ReplayEngine` mock-based reconstruction, `hour_paths` generation,
`ParquetReader` integration, end-to-end write-then-reconstruct round-trips, and
backfill REST response parsing.
