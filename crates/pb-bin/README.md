# pb-bin

CLI entrypoint for the poly-book workspace. Parses commands, loads layered
configuration, initializes tracing and the global allocator, and dispatches
to the appropriate subsystem.

## Subcommands

| Command | Purpose |
|---------|---------|
| `discover` | Find active BTC 5-minute prediction markets (with keyword filter). |
| `ingest` | Start live orderbook ingestion with Parquet/ClickHouse/metrics toggles. |
| `auto-ingest` | Continuously discover and ingest, rotating to the live market automatically. |
| `replay` | Reconstruct historical orderbook state at a specific timestamp. |
| `execution-replay` | Replay stored execution history independently of market-data replay. |
| `execution-append` | Append execution events to storage from flags or JSON input. |
| `backfill` | Periodic REST API snapshot backfill for checkpoint seeding. |
| `serve-api` | Start the read-only API server with live feed and replay access. |
| `serve` | Start the read-only serve runtime (WAL reader + checkpoint hydration + HTTP/WS). |
| `reconcile` | Offline recovery: rebuild Parquet partitions from the durable WAL after a crash lost a buffered window. Idempotent (per-partition replace). |

## Config Layering

```text
config/default.toml  →  env vars (PB__ prefix)  →  CLI flags
```

Sections: `[feed]`, `[storage]`, `[metrics]`, `[wal]`, `[api]`, `[grpc]`, `[logging]`.

Example: `PB__STORAGE__CLICKHOUSE_URL=http://localhost:8123`

## Design Notes

- Uses `mimalloc` as the global allocator for lower p99 latency under tokio.
  See [ADR-0005](../../docs/adr/0005-mimalloc-allocator.md).
- `anyhow` is used for error handling here (the only crate that uses it);
  library crates use `thiserror`.
- Graceful shutdown via `CancellationToken` propagated to all subsystems.
  Both SIGINT and SIGTERM are handled for container/production environments.
- In separated `serve` mode, startup hydrates from checkpoints + WAL, then
  resumes live WAL tailing from the exact post-hydration position rather than
  re-reading the entire log. The live consumer also commits its read position
  periodically so restarts do not roll back far under normal operation. That
  commit cadence is tunable via `wal.position_commit_interval_ms`.
- WAL writer is owned directly on the single-threaded event loop (no
  `Arc<Mutex<_>>` overhead).
- `fanout_event()` helper in `pipeline.rs` deduplicates event routing logic
  shared between `ingest` and `auto_ingest`.
- Forwarder tasks use idiomatic `while let` receive loops instead of
  `tokio::select!` patterns.
- **Task supervision**: long-lived background tasks (feed, dispatcher, storage
  sinks, fan-out forwarders, WAL drain, checkpoint producer) are registered with
  a `pipeline::Supervisor` (a tagged `JoinSet`). If any exits unexpectedly —
  returns, errors, or panics — before a coordinated shutdown, `ingest`/
  `auto-ingest` cancel the shutdown token and return a non-zero error rather than
  continuing with a dead component or exiting 0. In `auto_ingest` the rotating
  per-market feed generations are deliberately *not* supervised this way (their
  cycling is the expected steady state and is managed via `Generation`).
- **WAL→storage reconciliation**: `reconcile` reads the durable WAL and rebuilds
  the Parquet partitions it covers via `ParquetRecordWriter::write_batch_replacing`
  (per-`(dataset, asset, hour)` delete-then-write), so a storage window lost when
  a crash dropped the in-memory Parquet buffer is recoverable from the WAL (A.27).
  Run it offline (ingest stopped); it is idempotent.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New subcommand added | `README.md` workflows section, `docs/operations.md` |
| Config key added or removed | `config/default.toml`, `docs/operations.md` |
| CLI flag changed | `README.md` workflows section |
| Startup or shutdown behavior changed | `docs/operations.md` |
| New subcommand affects workstation scope | Update the active OpenSpec change under `openspec/changes/` |
