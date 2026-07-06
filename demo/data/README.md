# Demo capture

A real capture of Polymarket BTC 5-minute market data, recorded with
`poly-book auto-ingest` on 2026-07-06 (UTC). It contains the complete life of
two outcome tokens — every venue book snapshot, delta, and trade, the ingest
continuity events, and the periodic REST checkpoints — in the same split
Parquet layout the live pipeline writes.

Used by `poly-book demo` (see `docs/operations.md`), which replays it as a
simulated live feed behind the full workstation API. Any directory produced by
`ingest`/`auto-ingest` has the same shape; regenerate with:

```bash
PB__STORAGE__PARQUET_BASE_PATH=/tmp/capture poly-book auto-ingest
# stop with Ctrl-C after the desired window, then copy the asset files you want
```

The data is also directly readable with any Parquet tooling
(`just parquet-stats`, DuckDB, polars) — see `docs/operations.md` for the
schema and partition layout.
