# pb-metrics

Prometheus metrics helpers and HTTP scrape endpoint. Provides a shared metrics
recorder and a standalone HTTP server that exposes `/metrics` for Prometheus
scraping.

## Key Functions

| Function | Description |
|----------|-------------|
| `install_recorder()` | Installs the Prometheus metrics recorder. Call once at startup. |
| `start_metrics_server(port)` | Spawns an axum server serving `/metrics` on the given port. |
| `serve_metrics()` | Handler that returns the current metrics snapshot as text. |
| `serve_metrics_on_listener()` | Variant that binds to a provided `TcpListener`. |

## Usage Pattern

```text
1. Call install_recorder() at startup (pb-bin does this)
2. Use metrics crate macros anywhere: counter!(), histogram!()
3. start_metrics_server() serves /metrics on :9090
```

Any crate can record metrics via the `metrics` crate macros (`counter!`,
`histogram!`, `gauge!`) after the recorder is installed. pb-metrics itself
is a leaf crate with no internal workspace dependencies.

## Docs to Update After Changes

| What changed | Update |
|---|---|
| New metric registered | `docs/operations.md` metrics section |
| Metrics port default changed | `config/default.toml` `[metrics]` section, `docs/operations.md` |
| Recorder setup pattern changed | `pb-bin` startup code, `docs/operations.md` |
