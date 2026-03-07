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

# Replay orderbook state at a timestamp (microseconds)
replay token at:
    cargo run -- replay --token {{token}} --at {{at}}

# Backfill REST snapshots for tokens
backfill tokens:
    cargo run -- backfill --tokens {{tokens}}

# ── Data Inspection (DuckDB) ─────────────────────────────────

# List Parquet files under data directory
parquet-ls:
    @find {{data_dir}} -name '*.parquet' 2>/dev/null || echo "No parquet files found"

# Count total rows across all Parquet files
parquet-count:
    @duckdb -c "SELECT count(*) AS total_rows FROM '{{data_dir}}/**/*.parquet'"

# Peek at first 20 rows
parquet-peek:
    @duckdb -c "SELECT * FROM '{{data_dir}}/**/*.parquet' LIMIT 20"

# Show schema of Parquet files
parquet-schema:
    @duckdb -c "DESCRIBE SELECT * FROM '{{data_dir}}/**/*.parquet'"

# Summary stats: count, timestamp range, distinct assets, event types
parquet-stats:
    @duckdb -c " \
        SELECT \
            count(*) AS total_rows, \
            min(recv_timestamp_us) AS min_recv_ts, \
            max(recv_timestamp_us) AS max_recv_ts, \
            count(DISTINCT asset_id) AS distinct_assets, \
            sum(CASE WHEN event_type = 1 THEN 1 ELSE 0 END) AS snapshots, \
            sum(CASE WHEN event_type = 2 THEN 1 ELSE 0 END) AS deltas, \
            sum(CASE WHEN event_type = 3 THEN 1 ELSE 0 END) AS trades \
        FROM '{{data_dir}}/**/*.parquet' \
    "

# ── Metrics ───────────────────────────────────────────────────

# Fetch Prometheus metrics
metrics:
    @curl -s localhost:9090/metrics

# Grep Prometheus metrics by pattern
metrics-grep pattern:
    @curl -s localhost:9090/metrics | grep '{{pattern}}'

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
