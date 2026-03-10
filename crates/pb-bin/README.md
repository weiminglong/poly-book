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

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New subcommand added | `README.md` workflows section, `docs/operations.md` |
| Config key added or removed | `config/default.toml`, `docs/operations.md` |
| CLI flag changed | `README.md` workflows section |
| Startup or shutdown behavior changed | `docs/operations.md` |
| New subcommand affects workstation scope | Update the active OpenSpec change under `openspec/changes/` |
