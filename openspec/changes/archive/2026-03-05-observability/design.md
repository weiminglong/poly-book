# Design: Observability

## Overview

Observability is provided through three subsystems: Prometheus metrics exposed via HTTP, structured logging via `tracing`, and layered configuration for runtime flexibility.

## Metrics Recorder Pattern

All metrics are defined in `pb_metrics::recorder` using the `metrics` crate facade:

- **Registration**: `register_metrics()` is called once at startup, using `describe_counter!` and `describe_histogram!` to attach human-readable descriptions
- **Recording**: Standalone functions (`record_message_received`, `record_snapshot_applied`, etc.) wrap `counter!` and `histogram!` calls, providing a typed API for the rest of the codebase
- **Labels**: Dynamic labels are used where needed (e.g., `event_type` on message counter, `sink` on storage flush counter)

### Defined Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `pb_messages_received_total` | Counter | Total WebSocket messages, labeled by event_type |
| `pb_snapshots_applied_total` | Counter | Total book snapshots applied |
| `pb_deltas_applied_total` | Counter | Total book deltas applied |
| `pb_trades_received_total` | Counter | Total trades received |
| `pb_gaps_detected_total` | Counter | Total sequence gaps detected |
| `pb_reconnections_total` | Counter | Total WebSocket reconnections |
| `pb_storage_flushes_total` | Counter | Total storage flushes, labeled by sink |
| `pb_rest_requests_total` | Counter | Total REST API requests |
| `pb_message_processing_duration_us` | Histogram | Message processing time in microseconds |
| `pb_storage_flush_duration_ms` | Histogram | Storage flush time in milliseconds |
| `pb_ws_latency_us` | Histogram | WebSocket latency (recv - exchange timestamp) |

## Axum Metrics Server

- Uses `metrics_exporter_prometheus::PrometheusBuilder` to install a global Prometheus recorder
- Creates an axum `Router` with a single GET route at the configured endpoint (default `/metrics`)
- The route handler calls `handle.render()` to produce Prometheus text format output
- Binds to a configurable `SocketAddr` (default `0.0.0.0:9090`) via `tokio::net::TcpListener`
- Spawned as a background `tokio::spawn` task in the ingest command
- Errors during startup are captured in `MetricsError` (recorder install failure, bind failure)

## Tracing-Subscriber Configuration

- Initialized in `main()` before any other subsystem
- Uses `tracing_subscriber::fmt()` with `EnvFilter`
- Priority: `RUST_LOG` environment variable > CLI `--log-level` flag (default: `info`)
- Supports standard levels: `trace`, `debug`, `info`, `warn`, `error`
- Structured fields (e.g., `token_id`, `error`) are attached at log callsites throughout the codebase

## Layered Configuration

Configuration is assembled using the `config` crate with three layers, in priority order:

1. **TOML file** (`config/default.toml`): Base defaults for all settings
   - `[feed]` -- WebSocket/REST URLs, ping interval, reconnect backoff, rate limits
   - `[storage]` -- Parquet base path, flush intervals, ClickHouse connection
   - `[metrics]` -- listen address, endpoint path
   - `[logging]` -- log level, format
2. **Environment variables**: Prefix `PB__`, double-underscore separator (e.g., `PB__METRICS__LISTEN_ADDR`)
3. **CLI flags**: `--config`, `--log-level`, and subcommand-specific flags (e.g., `--tokens`, `--metrics`)

Settings are accessed via `config::Config::get_string` / `get_int` with fallback defaults in code.

## CLI Subcommands

| Command | Description |
|---------|-------------|
| `discover` | Find active BTC 5-min prediction markets, optional `--filter` |
| `ingest` | Live orderbook ingestion with `--tokens`, `--parquet`, `--clickhouse`, `--metrics` |
| `replay` | Reconstruct book at timestamp with `--token`, `--at`, `--source` |
| `backfill` | REST snapshot collection with `--tokens`, `--interval-secs`, `--duration-mins` |
