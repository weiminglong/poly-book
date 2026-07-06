# API cookbook

One working example per route, with the response shape captured from a real
run. Everything here runs fully offline against the committed sample capture.

## Setup

Build and start the offline demo (replays `demo/data/` as a simulated live
feed behind the full API):

```bash
cargo build --release -p pb-bin
./target/release/poly-book demo --metrics=false
```

The startup banner prints the capture's asset IDs, its time window in
microseconds, and copy-paste `curl` examples with known-good timestamps. The
examples below use the committed capture's values:

```bash
BASE=http://127.0.0.1:3000
ASSET=67249148562634303510812091719968200582025293007866552361275929928039192336914
START_US=1783331992041540   # capture window (printed by the banner)
END_US=1783332300338425
AT_US=1783332112508603      # a mid-capture instant
```

Two encoding rules apply everywhere: prices and sizes are fixed-point decimal
strings (4 and 6 decimal places), and timestamps are microseconds since the
Unix epoch. Live surfaces (snapshot, feed status, streams) reflect the looping
playback clock; historical surfaces (replay, integrity, execution) answer from
the capture's original timestamps. Full contract: [`docs/api.md`](../docs/api.md).

## Health

```bash
curl "$BASE/health"
```

```json
{"ready":true,"hydrated":true,"wal_lag_bytes":0,"needs_resync":false}
```

## Feed status

```bash
curl "$BASE/api/v1/feed/status"
```

```json
{
  "mode": "fixed_tokens",
  "session_status": "starting",
  "current_session_id": null,
  "active_asset_count": 2,
  "active_assets": [
    {"asset_id": "67249148…336914"},
    {"asset_id": "74422331…963673"}
  ],
  "last_rotation_us": null,
  "latest_global_warning": null
}
```

## Active assets

```bash
curl "$BASE/api/v1/assets/active"
```

```json
[
  {
    "asset_id": "67249148…336914",
    "last_recv_timestamp_us": 1783335032276557,
    "last_exchange_timestamp_us": 1783335032330788,
    "stale": false,
    "has_book": true
  },
  {"asset_id": "74422331…963673", "…": "…"}
]
```

`stale` flips to `true` when an asset has not updated within
`api.stale_after_secs` (default 15) — in the demo this happens between loop
passes of the capture.

## Resolve a slug or token ID

```bash
curl "$BASE/api/v1/assets/resolve?q=$ASSET"
```

```json
{"found":true,"asset_id":"67249148…336914"}
```

Always returns 200; check `found`. A registered slug (e.g.
`btc-updown-5m-…-yes`) resolves the same way and adds a `slug` field.

## Live orderbook snapshot

```bash
curl "$BASE/api/v1/orderbooks/$ASSET/snapshot?depth=3"
```

```json
{
  "asset_id": "67249148…336914",
  "sequence": 99,
  "last_update_us": 1783335032276557,
  "best_bid": {"price": "0.5400", "size": "24.820000"},
  "best_ask": {"price": "0.5500", "size": "127.290000"},
  "mid_price": 0.545,
  "spread": 0.01,
  "bid_depth": 54,
  "ask_depth": 45,
  "bids": [
    {"price": "0.5400", "size": "24.820000"},
    {"price": "0.5300", "size": "70.970000"},
    {"price": "0.5200", "size": "37.000000"}
  ],
  "asks": [
    {"price": "0.5500", "size": "127.290000"},
    {"price": "0.5600", "size": "552.630000"},
    {"price": "0.5700", "size": "647.270000"}
  ],
  "stale": false,
  "latest_warning": null
}
```

## Replay: reconstruct the book at an instant

```bash
curl "$BASE/api/v1/replay/reconstruct?asset_id=$ASSET&at_us=$AT_US&mode=recv_time&depth=3"
```

```json
{
  "asset_id": "67249148…336914",
  "mode": "recv_time",
  "used_checkpoint": true,
  "sequence": 233,
  "last_update_us": 1783332112508603,
  "best_bid": {"price": "0.9100", "size": "380.280000"},
  "best_ask": {"price": "0.9200", "size": "88.650000"},
  "mid_price": 0.915,
  "spread": 0.01,
  "bid_depth": 91,
  "ask_depth": 8,
  "bids": ["…"],
  "asks": ["…"],
  "continuity_events": [
    {
      "kind": "sequence_gap",
      "recv_timestamp_us": 1783332111962774,
      "exchange_timestamp_us": 1783332112012000,
      "details": "orderbook 67249148…: sequence gap 1 -> 137 (dropped 136 updates)"
    }
  ]
}
```

`mode` is required: `recv_time` (local receive clock) or `exchange_time`
(venue clock). Continuity events that affect the reconstructed window are
surfaced rather than hidden.

## Integrity summary

```bash
curl "$BASE/api/v1/integrity/summary?asset_id=$ASSET&start_us=$START_US&end_us=$END_US"
```

```json
{
  "asset_id": "67249148…336914",
  "start_us": 1783331992041540,
  "end_us": 1783332300338425,
  "total_book_events": 258708,
  "total_ingest_events": 16826,
  "reconnect_count": 0,
  "gap_count": 16826,
  "stale_snapshot_skip_count": 0,
  "validation_count": 0,
  "validations_matched": 0,
  "validations_mismatched": 0,
  "completeness": "best_effort",
  "continuity_events": [
    {"kind": "book_mismatch", "recv_timestamp_us": 1783331993027473, "…": "…"}
  ]
}
```

The window may not exceed 24 hours.

## Execution orders

```bash
curl "$BASE/api/v1/execution/orders?start_us=$START_US&end_us=$END_US&limit=5"
```

```json
{"events":[],"total_count":0}
```

The sample capture contains market data only, so the page is empty. When
execution events exist (appended via `poly-book execution-append`), each
event carries order IDs, kind, side, price, size, status, reason, and a
latency trace; `total_count` is the pre-pagination total. Supports
`order_id`, `asset_id`, `limit`, `offset`, and `order=asc|desc` parameters.

## Query workbench

```bash
curl "$BASE/api/v1/query/datasets"
curl -X POST "$BASE/api/v1/query/sql" \
  -H 'content-type: application/json' \
  -d '{"sql": "SELECT count(*) AS n FROM book_events"}'
```

The workbench is ClickHouse-backed and disabled in the offline demo (and by
default), so both routes return 503:

```json
{"error":"service temporarily unavailable"}
```

With `api.query_workbench_enabled = true` and ClickHouse configured (see
`docs/operations.md`), `datasets` lists each queryable dataset with its
columns, and `sql` returns:

```json
{
  "columns": [{"name": "n", "data_type": "UInt64"}],
  "rows": [[517416]],
  "row_count": 1,
  "truncated": false,
  "execution_time_ms": 4
}
```

Only single read-only statements against the advertised datasets are
accepted; a `LIMIT` is injected when missing.

## WebSocket orderbook stream

```bash
websocat "ws://127.0.0.1:3000/api/v1/streams/orderbook?asset_id=$ASSET"
```

Every frame — the initial one on connect and each subsequent update — carries
the full depth-bounded book state, so a consumer never needs to patch deltas
and slow consumers self-heal:

```json
{
  "asset_id": "67249148…336914",
  "sequence": 99,
  "last_update_us": 1783335032276557,
  "bid_depth": 54,
  "ask_depth": 45,
  "bids": [{"price": "0.5400", "size": "24.820000"}, "…"],
  "asks": [{"price": "0.5500", "size": "127.290000"}, "…"],
  "mid_price": 0.545,
  "spread": 0.01
}
```

## Same data, no API

The capture behind all of the above is plain Parquet — see
[`research/orderbook_analysis.ipynb`](../research/orderbook_analysis.ipynb)
for consuming it directly with polars.
