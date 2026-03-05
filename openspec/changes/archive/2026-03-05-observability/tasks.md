# Tasks: Observability

## Metrics Recorder

- [x] Define `register_metrics()` function with `describe_counter!` and `describe_histogram!` for all metrics
- [x] Implement `record_message_received` with `event_type` label
- [x] Implement `record_snapshot_applied`, `record_delta_applied`, `record_trade_received` counters
- [x] Implement `record_gap_detected` and `record_reconnection` counters
- [x] Implement `record_storage_flush` counter with `sink` label
- [x] Implement `record_rest_request` counter
- [x] Implement `record_processing_duration_us` histogram
- [x] Implement `record_flush_duration_ms` histogram
- [x] Implement `record_ws_latency_us` histogram

## Metrics Server

- [x] Implement `start_metrics_server` with `PrometheusBuilder` recorder installation
- [x] Create axum `Router` with GET route at configurable endpoint
- [x] Bind to configurable `SocketAddr` via `tokio::net::TcpListener`
- [x] Define `MetricsError` enum with `RecorderInstall` and `ServerStart` variants

## Structured Logging

- [x] Initialize `tracing_subscriber::fmt` with `EnvFilter` in `main()`
- [x] Support `RUST_LOG` env var with fallback to `--log-level` CLI flag
- [x] Default log level set to `info`

## Layered Configuration

- [x] Create `config/default.toml` with `[feed]`, `[storage]`, `[metrics]`, `[logging]` sections
- [x] Load TOML config file via `config::File` (optional, non-fatal if missing)
- [x] Layer environment variables with `PB__` prefix and `__` separator
- [x] Access settings with `get_string` / `get_int` with in-code fallback defaults

## CLI Subcommands

- [x] Implement `discover` subcommand with optional `--filter` flag
- [x] Implement `ingest` subcommand with `--tokens`, `--parquet`, `--clickhouse`, `--metrics` flags
- [x] Implement `replay` subcommand with `--token`, `--at`, `--source` flags
- [x] Implement `backfill` subcommand with `--tokens`, `--interval-secs`, `--duration-mins` flags
- [x] Wire all subcommands to their respective command handlers in `main()`

## Integration

- [x] Call `pb_metrics::register_metrics()` in ingest command before pipeline starts
- [x] Spawn metrics server as background task via `tokio::spawn` in ingest command
- [x] Read `metrics.listen_addr` and `metrics.endpoint` from config with defaults
