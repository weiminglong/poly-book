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
  then applies deltas forward. No checkpoint means replaying from the beginning
  of the Parquet partition.
- The `EventReader` trait has methods for reading each dataset type:
  `read_market_data`, `read_checkpoints`, `read_latest_checkpoint`,
  `read_validations`, `read_execution_events`.
- Both readers support time-range filtering and asset filtering at the storage
  layer to minimize I/O.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New `EventReader` method | Both `ParquetReader` and `ClickHouseReader` must implement it |
| Reconstruction logic changed | `docs/serve-api.md` replay semantics section |
| New storage backend reader | `docs/operations.md`, `pb-api` if the reader is used in routes |
| Backfill config shape changed | `config/default.toml`, `docs/operations.md` |
| Replay semantics affect API responses | `docs/api.md`, check the active OpenSpec change under `openspec/changes/` |
