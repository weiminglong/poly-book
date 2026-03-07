# poly-book

[![CI](https://github.com/weiminglong/poly-book/actions/workflows/ci.yml/badge.svg)](https://github.com/weiminglong/poly-book/actions/workflows/ci.yml)
[![Supply Chain](https://github.com/weiminglong/poly-book/actions/workflows/supply-chain.yml/badge.svg)](https://github.com/weiminglong/poly-book/actions/workflows/supply-chain.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

`poly-book` is a Rust workspace for collecting Polymarket order book data, storing
it in replay-friendly formats, and reconstructing market state later.

It is aimed at people building or studying low-latency market-data systems:

- live WebSocket and REST ingestion
- split Parquet and ClickHouse storage
- checkpoint-based historical replay
- read-only workstation API for live and historical inspection
- metrics and operational hooks

## Project Status

This is an active personal project that is open to outside contributions.

Current expectations:

- the core ingestion and replay path is working and covered by tests
- APIs and storage layout may still evolve
- the repository is optimized for contributor readability, not long-term API stability

If you want to contribute, start with [CONTRIBUTING.md](CONTRIBUTING.md).

## Quickstart

### Prerequisites

- [Rust](https://rustup.rs/) 1.94+
- Docker for the full integration suite
- `just` is optional
- `duckdb` is optional for Parquet inspection
- ClickHouse is optional unless you want warm-storage testing

### Build

```bash
cargo build
```

### Fast Local Validation

```bash
cargo test --workspace --exclude pb-integration-tests
```

### Full Validation

```bash
just ci
```

This runs formatting, Clippy, and the full test suite. The integration package
includes ClickHouse round-trip coverage through `testcontainers`, so Docker must
be available.

### Copy-Paste Demo

```bash
cargo run -- discover --filter btc --limit 5
```

Expected output shape:

```text
--- Live BTC 5-minute market ---
Event: <event title>
  Market: <market question>
    Token IDs: <yes_token_id>,<no_token_id>
    Active: true
```

The exact market title and token IDs change over time.

## Common Workflows

### Discover markets

```bash
just discover
# or
cargo run -- discover --filter btc --limit 100
```

### Auto-discover and ingest

```bash
just auto-ingest
# or
cargo run -- auto-ingest
```

Continuously rotates to the live BTC 5-minute market and ingests data.

### Ingest known token IDs

```bash
just ingest <TOKEN_ID>
# or
cargo run -- ingest --tokens <TOKEN_ID> --parquet --metrics
```

### Replay historical state

```bash
just replay <TOKEN_ID> <TIMESTAMP_US>
# or
cargo run -- replay --token <TOKEN_ID> --at <TIMESTAMP_US> --source parquet --mode recv_time
```

Replay modes:

- `recv_time` reproduces local observation order
- `exchange_time` reorders by venue timestamp, then receive time, then sequence

Add `--validate` to compare replay output against the next checkpoint.

### Serve the workstation API

```bash
cargo run -- serve-api --tokens <TOKEN_ID>
# or
cargo run -- serve-api --auto-rotate
```

This starts the read-only API layer for the quant workstation backend. The
first release serves live feed status, active assets, live book snapshots, and
Parquet-backed replay reconstruction.

### Replay execution history

```bash
cargo run -- execution-replay --start <START_US> --end <END_US> --source parquet
```

### Backfill checkpoints

```bash
just backfill <TOKEN_ID>
# or
cargo run -- backfill --tokens <TOKEN_ID> --interval-secs 60
```

### Inspect local Parquet output

```bash
just parquet-stats
just parquet-peek
just parquet-schema
```

## Performance & Correctness

### Benchmarks

```bash
cargo bench                 # all benchmarks
cargo bench -p pb-types     # fixed-point ops + wire deserialization throughput
cargo bench -p pb-book      # book operations, depth iteration, mixed workloads
```

### Property-Based Testing

The `pb-types` and `pb-book` crates include `proptest` suites that verify
invariants across randomized inputs:

- fixed-point roundtrip, ordering, and serde consistency
- bid/ask ordering preserved after arbitrary snapshots and deltas
- spread never negative for non-crossed books
- mid price and weighted mid price bounded by best bid/ask
- sequence monotonicity and gap detection soundness
- snapshot idempotency
- crossed-book detection via `check_integrity()`
- total size consistency across all levels

### Fuzzing

Fuzz targets under `fuzz/` cover deserialization and book mutation:

```bash
cargo +nightly fuzz run fuzz_ws_deser       # wire protocol parsing
cargo +nightly fuzz run fuzz_fixed_price     # FixedPrice/FixedSize parsing
cargo +nightly fuzz run fuzz_book_delta      # book delta application invariants
```

### Latency Design

Key design choices documented in [docs/latency.md](docs/latency.md) and
[docs/adr/](docs/adr/):

- Fixed-point arithmetic eliminates FPU stalls and NaN guards (ADR-0001)
- BTreeMap gives O(1) best bid/ask via sorted iteration (ADR-0002)
- Channel-based message passing avoids locks on the hot path (ADR-0003)
- Zero-copy wire deserialization reduces allocations (ADR-0004)
- mimalloc global allocator for lower p99 latency (ADR-0005)
- FxHashMap in Dispatcher for faster internal lookups (ADR-0006)

## Why This Repo Exists

The project demonstrates a specific style of trading-infrastructure design:

- fixed-point numeric types instead of heap-heavy decimal math
- single-threaded book mutation on the hot path
- bounded channels and backpressure between components
- typed, split datasets instead of one overloaded event stream
- replay and validation as first-class storage consumers

## Architecture

```text
Polymarket WS -> pb-feed -> pb-store -> Parquet / ClickHouse
                 |                     ^
                 v                     |
              pb-api <---- pb-replay --+
                 |
                 +-> workstation clients
                 |
                 +-> pb-metrics
```

Workspace crates:

| Crate | Responsibility |
|-------|----------------|
| `pb-api` | Read-only HTTP API and live read model for workstation clients |
| `pb-types` | Fixed-point types, wire formats, and persisted records |
| `pb-book` | In-memory L2 order book engine with analytics (weighted mid, top-N, integrity checks) |
| `pb-feed` | WebSocket ingest, REST discovery, dispatcher, rate limiting |
| `pb-store` | Parquet and ClickHouse sinks |
| `pb-replay` | Historical replay, checkpoint reconstruction, validation, backfill |
| `pb-metrics` | Prometheus metrics endpoint |
| `pb-bin` | CLI entrypoint |

## Configuration

Runtime config is layered:

1. `config/default.toml`
2. environment variables prefixed with `PB__`
3. CLI flags

Example:

```bash
PB__LOGGING__LEVEL=debug cargo run -- auto-ingest
```

Operational details, deployment notes, infrastructure setup, and data layout live
in [docs/operations.md](docs/operations.md).

Current workstation API routes and runtime notes live in:

- [docs/api.md](docs/api.md)
- [docs/serve-api.md](docs/serve-api.md)

## Roadmap

Near-term work that would make strong public contributions:

- broader replay validation and determinism coverage
- workstation integrity, execution, and query API expansion beyond the current
  read-only Phase 3 slice
- better local sample-data workflows for offline development
- more explicit schema/versioning guarantees for persisted datasets
- clearer operational docs for ClickHouse and cloud object storage
- examples for strategy hooks and execution-event workflows

## Development Workflow

This repository uses [OpenSpec](https://github.com/Fission-AI/OpenSpec) for larger
feature work. Archived proposals, designs, specs, and tasks live under
`openspec/changes/archive/`.

For contribution standards and PR expectations, see:

- [CONTRIBUTING.md](CONTRIBUTING.md)
- [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)
- [SECURITY.md](SECURITY.md)
- [CHANGELOG.md](CHANGELOG.md)
- [docs/releasing.md](docs/releasing.md)

## License

[MIT](LICENSE)
