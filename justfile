set shell := ["bash", "-cu"]

data_dir := "./data"

# ── Build & Test ──────────────────────────────────────────────

# Type-check all crates
check:
    cargo check

# Run all tests
test:
    cargo test

# Run Criterion benchmarks
bench:
    cargo bench

# Measure the durability-layer failover RTO (primary crash -> standby lease
# acquire + tail recovery + resume + re-hydrate). Run on the TARGET hardware for a
# realistic baseline. The full deployment RTO additionally includes container
# scheduling + health-check time, which needs a live cluster to measure.
failover-drill:
    cargo test -p pb-wal --lib measure_failover_handoff_rto -- --ignored --nocapture

# Run clippy with warnings as errors
clippy:
    cargo clippy --workspace -- -D warnings

# Format all code
fmt:
    cargo fmt --all

# Check formatting (CI mode)
fmt-check:
    cargo fmt --all -- --check

# Run full CI pipeline: fmt-check, clippy, test
ci: fmt-check clippy test

# ── Run Commands ──────────────────────────────────────────────

# Discover active BTC 5-minute markets
discover:
    cargo run -- discover

# Start live orderbook ingestion with token IDs
ingest tokens:
    cargo run -- ingest --tokens {{tokens}}

# Auto-discover and ingest BTC 5-min markets continuously
auto-ingest:
    cargo run -- auto-ingest

# Replay orderbook state at a timestamp (microseconds). --mode is required by the
# CLI; default to recv_time (override with `just replay TOKEN AT exchange_time`).
replay token at mode="recv_time":
    cargo run -- replay --token {{token}} --at {{at}} --mode {{mode}}

# Backfill REST snapshots for tokens
backfill tokens:
    cargo run -- backfill --tokens {{tokens}}

# ── Data Inspection (DuckDB) ─────────────────────────────────
# Datasets are split into separate Parquet trees with different schemas
# (book_events, trade_events, ingest_events, book_checkpoints,
# replay_validations, execution_events). Inspect ONE dataset at a time — globbing
# across all of them mixes incompatible schemas. Override the dataset, e.g.
# `just parquet-count trade_events`.

# List Parquet files under data directory
parquet-ls:
    @find {{data_dir}} -name '*.parquet' 2>/dev/null || echo "No parquet files found"

# Count total rows in a dataset
parquet-count dataset="book_events":
    @duckdb -c "SELECT count(*) AS total_rows FROM '{{data_dir}}/{{dataset}}/**/*.parquet'"

# Peek at first 20 rows of a dataset
parquet-peek dataset="book_events":
    @duckdb -c "SELECT * FROM '{{data_dir}}/{{dataset}}/**/*.parquet' LIMIT 20"

# Show schema of a dataset
parquet-schema dataset="book_events":
    @duckdb -c "DESCRIBE SELECT * FROM '{{data_dir}}/{{dataset}}/**/*.parquet'"

# Summary stats for a dataset: row count, recv-timestamp range, distinct assets
parquet-stats dataset="book_events":
    @duckdb -c " \
        SELECT \
            count(*) AS total_rows, \
            min(recv_timestamp_us) AS min_recv_ts, \
            max(recv_timestamp_us) AS max_recv_ts, \
            count(DISTINCT asset_id) AS distinct_assets \
        FROM '{{data_dir}}/{{dataset}}/**/*.parquet' \
    "

# ── Metrics ───────────────────────────────────────────────────

# Fetch Prometheus metrics
metrics:
    @curl -s localhost:9090/metrics

# Grep Prometheus metrics by pattern
metrics-grep pattern:
    @curl -s localhost:9090/metrics | grep '{{pattern}}'

# ── Demo ──────────────────────────────────────────────────────

# Offline demo: replay the committed capture as a simulated live feed behind
# the full API (no network, no venue dependency). Add --speed for fast-forward.
demo:
    cargo run --release -- demo

# ── Docker Compose ────────────────────────────────────────────

# One-container live workstation (feed + API + UI) at http://localhost:3000
up:
    docker compose --profile minimal up --build

# Full topology: ingest + serve (shared WAL) + ClickHouse
up-full:
    docker compose --profile full up --build

# Full topology plus Prometheus/Alertmanager/Grafana (Grafana at :3001)
up-obs:
    docker compose --profile full --profile observability up --build

# Stop and remove all compose services (data volumes are kept)
down:
    docker compose --profile minimal --profile full --profile observability down

# ── Housekeeping ──────────────────────────────────────────────

# Clean build artifacts
clean:
    cargo clean

# Clean only debug build output
clean-debug:
    rm -rf target/debug

# Clean only incremental compiler caches
clean-incremental:
    rm -rf target/debug/incremental target/release/incremental

# Clean Criterion benchmark artifacts
clean-bench:
    rm -rf target/criterion

# Show build artifact sizes under target/
target-size:
    @du -sh target 2>/dev/null || echo "No target directory found"
    @du -sh target/* 2>/dev/null | sort -hr || true

# Remove data directory (with confirmation)
clean-data:
    @echo "This will delete {{data_dir}} and all Parquet files."
    @read -p "Are you sure? [y/N] " confirm && [ "$$confirm" = "y" ] && rm -rf {{data_dir}} && echo "Deleted {{data_dir}}" || echo "Aborted"
