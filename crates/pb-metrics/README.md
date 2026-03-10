# pb-metrics

Prometheus metrics helpers and HTTP scrape endpoint. Provides a shared metrics
recorder and a standalone HTTP server that exposes `/metrics` for Prometheus
scraping.

## Key Types and Functions

| Item | Description |
|------|-------------|
| `install_recorder()` | Installs the Prometheus metrics recorder globally. Returns a `PrometheusHandle`. Call once at startup. |
| `serve_metrics(handle, addr, endpoint)` | Starts an axum HTTP server serving metrics at the given address and endpoint path. |
| `serve_metrics_on_listener(handle, listener, endpoint)` | Like `serve_metrics` but accepts a pre-bound `TcpListener`. |
| `start_metrics_server(addr, endpoint)` | Convenience: calls `install_recorder()` then `serve_metrics()`. |
| `register_metrics()` | Registers all metric descriptions (counters and histograms). Call once at startup. |
| `record_*()` helpers | Typed helper functions for recording specific metrics (e.g. `record_message_received`, `record_snapshot_applied`, `record_delta_applied`, `record_trade_received`, `record_gap_detected`, `record_reconnection`, `record_storage_flush`, `record_rest_request`, `record_processing_duration_us`, `record_flush_duration_ms`, `record_ws_latency_us`, `record_api_request_duration_ms`, `record_rotation`, `record_discovery_failure`). |
| `MetricsError` | Error type for recorder installation and server startup failures. |

## Usage Pattern

```text
1. Call install_recorder() at startup (pb-bin does this)
2. Call register_metrics() to register metric descriptions
3. Use record_*() helpers or metrics crate macros: counter!(), histogram!()
4. serve_metrics() / start_metrics_server() serves /metrics on the configured addr
```

Any crate can record metrics via the `metrics` crate macros (`counter!`,
`histogram!`, `gauge!`) or via the typed `record_*()` helpers after the
recorder is installed. pb-metrics itself is a leaf crate with no internal
workspace dependencies.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New metric registered | `docs/operations.md` metrics section |
| Metrics port default changed | `config/default.toml` `[metrics]` section, `docs/operations.md` |
| Recorder setup pattern changed | `pb-bin` startup code, `docs/operations.md` |
