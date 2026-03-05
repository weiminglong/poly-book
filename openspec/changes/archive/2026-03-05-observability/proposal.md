# Proposal: Observability

## Why

A production orderbook system requires visibility into its runtime behavior. Without metrics and structured logging, diagnosing performance bottlenecks, detecting sequence gaps, and monitoring WebSocket connectivity is impractical. Prometheus metrics with an HTTP scrape endpoint provide standard observability, while structured tracing with configurable log levels enables debugging across all subsystems.

## What Changes

- Add `pb-metrics` crate with Prometheus metric definitions and an axum HTTP server
- Register counters for messages, snapshots, deltas, trades, gaps, reconnections, storage flushes, and REST requests
- Register histograms for message processing duration, storage flush duration, and WebSocket latency
- Expose metrics via an axum HTTP endpoint at a configurable address and path
- Configure `tracing-subscriber` with `EnvFilter` for runtime log level control
- Implement layered configuration: TOML file -> environment variables (PB__ prefix) -> CLI flags
- Add CLI subcommands: `discover`, `ingest`, `replay`, `backfill`

## Capabilities

### New Capabilities

- `metrics-recorder`: Register and record Prometheus counters and histograms for all pipeline stages
- `metrics-server`: Serve Prometheus-format metrics over HTTP via axum
- `structured-logging`: Tracing-based structured logging with configurable levels
- `layered-config`: TOML file + environment variable + CLI argument configuration

### Modified Capabilities

None.

## Impact

- New crate: `pb-metrics` (`crates/pb-metrics/`)
  - `src/recorder.rs` -- metric registration and recording functions
  - `src/server.rs` -- axum HTTP server for Prometheus scraping
  - `src/error.rs` -- MetricsError enum
- Modified: `crates/pb-bin/src/main.rs` -- CLI structure, tracing init, config loading
- Modified: `crates/pb-bin/src/commands/ingest.rs` -- metrics server startup integration
- New: `config/default.toml` -- default configuration values
