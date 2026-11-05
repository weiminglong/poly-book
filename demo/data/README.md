# Demo capture

A real capture of Polymarket BTC 5-minute market data, recorded with
`poly-book ingest --auto-rotate` on 2026-07-06 (UTC). It contains the complete life of
two outcome tokens — every venue book snapshot, delta, and trade, the ingest
continuity events, and the periodic REST checkpoints — in the same split
Parquet layout the live pipeline writes.

Used by `poly-book demo` (see `docs/operations.md`), which replays it as a
simulated live feed behind the full workstation API. Any directory produced by
`ingest` has the same shape; regenerate with:

```bash
PB__STORAGE__PARQUET_BASE_PATH=/tmp/capture poly-book ingest --auto-rotate
# stop with Ctrl-C after the desired window, then copy the asset files you want
```

The `ingest_events` dataset was rewritten once after capture to drop
`BookMismatch` events that an earlier dispatcher bug raised spuriously (it
compared mid-message and treated the venue's empty-ask marker `"1"` as a real
level). The venue book data itself (`book_events`, `trade_events`,
`book_checkpoints`) is untouched; replaying its deltas matches 1,683 of the
1,732 venue snapshots it contains exactly, with top-of-book differing once.

The data is also directly readable with any Parquet tooling
(`just parquet-stats`, DuckDB, polars) — see `docs/operations.md` for the
schema and partition layout.
