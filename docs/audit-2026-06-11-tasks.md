# Poly-Book Remediation Tasks — Phased Handoff

**Source:** [`docs/audit-2026-06-11-production-readiness.md`](audit-2026-06-11-production-readiness.md) (159 verified findings, A.1–A.159).
**Purpose:** an executable, dependency-ordered checklist for an engineer or agent to drive the codebase toward the Jane Street / Citadel production bar.

## How to use this document

- Work **top to bottom within a phase**. Tasks are ordered so earlier ones unblock later ones.
- Each task **merges all duplicate findings** that share one root cause (different reviewers reported the same defect from different angles). The `Findings` line lists every audit ID the task closes — update the audit report's status when you finish.
- **Re-verify before editing.** Findings come from *reading* code, not running it. Confirm each against current code first; the build/deploy/ClickHouse tasks (P1-INFRA-1, P1-STORE-1) are cheap to confirm empirically (`cargo build`, `docker build`, a ClickHouse roundtrip) and the most consequential if a detail has shifted.
- **Treat `file:line` as starting coordinates, not gospel.** Line numbers drift the moment you edit a file. Grep for the cited code, don't trust the number.
- **`Done when` is the contract.** A task isn't complete until its acceptance criterion holds *and* the named test exists and passes. Per repo convention (`CLAUDE.md`): run `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all -- --check` before committing, and propagate doc changes per each crate's README "Docs to Update After Changes" table.
- **Coverage map** at the end maps every finding A.1–A.159 to a task ID. Nothing is dropped; low/info items are batched into cleanup tasks.

## Phase summary

| Phase | Theme | Goal | Tasks |
|---|---|---|---|
| **1** | Correctness & Durability | Zero silent data loss; faithful, deterministic replay | P1-* (16) |
| **2** | Operational Hardening | Nothing degrades silently; exposure bounded; on-call can see and act | P2-* (22) |
| **3** | HFT-Grade Capabilities | Venue-anchored correctness, failover, continuous regression discipline | P3-* (6) |

---

# Phase 1 — Correctness & Durability

*Make the advertised guarantees real before anything else. Until this phase lands, the system loses data on crash and replay is not trustworthy.*

## Durability — the WAL

### [x] P1-WAL-1 — Add a steady-state flush + fsync policy to the WAL write path
- **Severity:** high · **Findings:** A.11, A.16, A.18, A.29, A.46, A.113, A.129
- **Files:** `crates/pb-wal/src/writer.rs:150` (`Segment::sync` has zero callers), `crates/pb-bin/src/commands/ingest.rs:133` (append-only hot loop, flush only at shutdown)
- **Problem:** `fdatasync` is never called in production; the only `flush()` is at graceful shutdown. Crash loses up to 64 KiB of acked records in the `BufWriter`; power loss loses all unsynced page cache; the serve tailer can't see records until 64 KiB accumulates, so a quiet market lags the live read model indefinitely. WAL open/append failure is `warn!`-and-continue, silently disabling the durability backbone. None of this path is instrumented.
- **Action:** Add a configurable flush/sync policy to `WalWriter`: `flush()` on a short interval (5–50 ms, or per-append) for tail visibility; `fdatasync` on a configurable interval or byte budget; on rotation, fsync the sealed segment then fsync the directory (also fsync the directory after the first segment is created). Drive it from the ingest loop with a `tokio::interval`. Make append/open failure fatal (or a counter + readiness flag), not a warning. Add `wal_fsync_latency`, `wal_append_failures` metrics (coordinate with P2-OBS-1).
- **Done when:** new config knobs `wal.flush_interval_ms` / `wal.sync_interval_*` exist and are honored; a test asserts records are visible to a `WalReader` within the flush interval; a fault-injection test asserts an append error sets the failure counter / non-ready state instead of being swallowed; the README durability claim matches actual behavior.

### [x] P1-WAL-2 — Recover torn / zeroed segment tails on writer reopen; checksum the frame header
- **Severity:** high · **Findings:** A.30, A.33, A.126, A.7 (forensic confirmation)
- **Files:** `crates/pb-wal/src/segment.rs:51` (`open_append` resumes at raw file length), `:153` (frame length field outside the CRC), `crates/pb-wal/src/reader.rs:105`
- **Problem:** `open_append` resumes at `metadata().len()` without validating a frame boundary. After a crash mid-frame the writer appends after garbage and the reader desyncs framing, silently dropping every post-restart record; after ext4/XFS zero-fill the reader treats `len==0` as clean EOF and stalls forever. The frame length is not covered by any checksum, so a flipped length byte is undetectable. The captured WAL already contains a real head-overwrite corruption event from this class of bug.
- **Action:** On `WalWriter::open`, scan the last segment frame-by-frame from offset 0 (or a known-good checkpoint), and `ftruncate` at the first invalid/torn/zero frame before resuming appends (standard RocksDB/etcd-style recovery). Extend the CRC to cover the length field (or add a header checksum). Make corruption emit a metric + a resync signal, not just a `warn!`.
- **Done when:** crash-recovery tests exist that (a) write N frames, truncate mid-frame, reopen → writer truncates to N-1 and the reader reads exactly N-1 with no desync; (b) zero-fill the tail, reopen → recovered identically; (c) flip a length byte → detected as corruption, not silent drop. (Pairs with P1-TEST-1.)

### [x] P1-WAL-3 — Make tail reads incremental and non-blocking; fix the permanent reader stall
- **Severity:** high · **Findings:** A.31, A.6, A.14, A.32
- **Files:** `crates/pb-wal/src/reader.rs:253` (`advance_segment` re-reads whole segment), `:274`, `:136` (`next()` returns `Ok(None)` forever when `current_data` is `None`), `crates/pb-bin/src/commands/serve.rs:178` (50 ms poll), `:196` (`lag_bytes` stats every segment)
- **Problem:** When caught up, every poll re-`std::fs::read`s the entire active segment (up to 64 MB) and re-lists the directory — ~1.3 GB/s of blocking page-cache traffic on the async runtime at the 50 ms serve poll. Separately, once `current_data` is `None` (empty-dir startup before ingest, or a prune race), the reader returns `Ok(None)` forever while `/health` still reports ready — the live tail is dead silently.
- **Action:** Keep an open `File` handle per segment; `stat` the length first and return "no change" without any read if unchanged; when grown, `pread` only `[prev_len, new_len)` and append to the cached buffer. Re-list the directory only when the current segment is exhausted (or rate-limit it). Run blocking WAL I/O under `spawn_blocking` (or a dedicated blocking thread). In `next()`, when `current_data` is `None`, refresh the segment list and retry `load_segment`/`advance_segment` before returning `None`; surface a true gap as the existing-but-unused `WalError::SegmentGap` so callers fail loudly.
- **Done when:** a benchmark/inspection shows a caught-up reader does O(0) bytes read per idle poll; a test asserts a reader opened on an empty dir starts delivering records once the writer creates segment 0; a test asserts a missing requested segment returns `SegmentGap`, not a silent stall.

## Durability — storage sinks

### [x] P1-STORE-1 — Make ClickHouse persistence functional and put it under CI
- **Severity:** high · **Findings:** A.3, A.4, A.25
- **Files:** `crates/pb-store/src/writer.rs:38` (DDL with `Nullable` in `ORDER BY`), `:232` (Enum8 serialized as Rust `String` over RowBinary), `crates/pb-replay/src/reader.rs` (matching reader structs), `.github/workflows/ci.yml:33`
- **Problem:** Two independent, empirically-reproduced defects make the ClickHouse sink unable to persist anything against a stock server: `Nullable` columns in the sorting key are rejected (`allow_nullable_key` off by default), and `Enum8` columns are written as `String` (RowBinary validator rejects → every batch aborts). `ensure_tables()` failure is only a `warn!`, so it fails silently while Parquet runs.
- **Action:** Remove `Nullable` from every `ORDER BY` (restructure the key or use sentinels); encode `Enum8` as integer discriminants (or `LowCardinality(String)`); fix the corresponding reader structs. Make `ensure_tables()` failure fatal. Un-`#[ignore]` the testcontainers roundtrip and run it in a Docker-enabled CI job.
- **Done when:** `CREATE TABLE` succeeds on a stock `clickhouse-server` (test via testcontainers); a write→read roundtrip for every dataset passes in CI; the CI job is required.

### [ ] P1-STORE-2 — Stop the single-error total shutdown; add bounded-retry flush with buffer retention
- **Severity:** high · **Findings:** A.5, A.12, A.26
- **Files:** `crates/pb-store/src/clickhouse_sink.rs:66`, `crates/pb-store/src/parquet_sink.rs:55`, `crates/pb-bin/src/commands/ingest.rs:145`, `crates/pb-store/src/pipeline.rs:96`
- **Problem:** A single transient sink flush error tears down all ingestion and exits `0`, so supervisors won't restart it; the buffered batch is dropped; the "will retry on insert" log describes behavior that doesn't exist.
- **Action:** Retry flushes with bounded backoff while **retaining** the buffer; isolate sinks so one failing sink can't kill the other or the WAL; on terminal failure exit non-zero (so the supervisor restarts). Add a `sink_flush_failures` counter.
- **Done when:** a fault-injection test asserts a transient flush error is retried and the buffer survives; a terminal error exits non-zero; a failing ClickHouse sink does not stop the Parquet sink or the WAL.

### [x] P1-STORE-3 — Close the Parquet data-loss and silent-overwrite windows
- **Severity:** high/medium · **Findings:** A.27, A.122, A.28, A.123, A.153
- **Files:** `crates/pb-store/src/parquet_sink.rs:13` (300 s in-memory buffer), `:46` (unbounded buffering, inline flush), `crates/pb-store/src/writer.rs:181` (deterministic name + `PutMode::Overwrite`), `:159` (out-of-range ts → 1970 partition), `crates/pb-bin/src/commands/backfill.rs:48` (uncanonicalized relative base path)
- **Problem:** Parquet buffers 5 minutes in a plain `Vec`; a crash loses that window with no WAL→storage reconciliation. Deterministic `{asset}_{first_ts}.parquet` names with `Overwrite` let quiet-book checkpoints and execution-append runs silently erase prior files. `backfill` passes a raw relative `./data` path that `object_store` percent-encodes to `/%2E/data/...`, so every flush fails while the command prints "complete". Cancellation flushes the local buffer but abandons records still in the mpsc channel.
- **Action:** Add a WAL→storage reconciliation/replay path (or shrink the window + fsync the WAL so it's the recovery source). Make file names collision-proof (append a monotonic/uuid suffix) or use `PutMode::Create` and handle conflicts. Reject/repartition out-of-range timestamps instead of defaulting to 1970. Canonicalize the backfill base path. On cancel, drain the channel before final flush.
- **Done when:** a crash-mid-window test recovers the buffered records from the WAL; a name-collision test does not lose the earlier file; `backfill` against `./data` actually writes readable Parquet (integration test); cancel drains the channel.
- **Progress (2026-06):** A.122 done — content-hashed Parquet names (`{asset}_{first_ts}_{hash}.parquet`) prevent silent overwrite. A.28 done — `build_object_store` canonicalizes/wires the base path (no more `/%2E/data`). A.123 done — out-of-range/unrepresentable timestamps go to an `invalid_timestamp` partition instead of 1970. **A.27 done** — `reconcile` CLI command rebuilds Parquet partitions from the durable WAL via `ParquetRecordWriter::write_batch_replacing` (per-`(dataset,asset,hour)` delete-then-write, idempotent, offline); a crashed 5-min Parquet buffer is now recoverable. Tested: reconcile collapses duplicate hour files to one + idempotent re-run; CLI parse. **A.153 done** — both sinks `drain_channel` (bounded 10s) before the final flush on cancellation, so queued records are not abandoned on graceful shutdown; tested (5 queued records persisted). **This task is now complete.**

## Replay & book correctness

### [x] P1-REPLAY-1 — Make replay validation non-vacuous
- **Severity:** high · **Findings:** A.8, A.23, A.52
- **Files:** `crates/pb-replay/src/engine.rs:105`, `:113`
- **Problem:** `replay_validation` picks the first checkpoint *after* the timestamp as reference, then `reconstruct_at` seeds from that same checkpoint via an inclusive bound — so `matched` compares the checkpoint to itself and is always `true`. Confirmed live against captured data. (All 85 captured checkpoints also have `wal_offset=NULL`, see P1-CKPT-1.)
- **Action:** Reconstruct from checkpoints strictly *older* than the reference; fix the inclusive/exclusive boundary; ensure the mock/test reader's checkpoint logic is reachable on the production path.
- **Done when:** a test feeds a deliberately divergent stream and asserts `matched=false`; a faithful stream asserts `matched=true`; the boundary is covered by a unit test.

### [ ] P1-REPLAY-2 — Make replay deterministic and wire-faithful
- **Severity:** high/medium · **Findings:** A.116, A.117, A.142, A.152
- **Files:** `crates/pb-replay/src/engine.rs:287` (sort key `(recv_ts, sequence)`; `buffer_unordered`)
- **Problem:** The per-asset synthetic sequence resets to 0 on every snapshot, so a same-microsecond pre-snapshot delta sorts *after* the snapshot (316 such rows in captured data); `buffer_unordered(8)` makes ties nondeterministic across runs. Checkpoint boundaries also mix exchange-clock and recv-clock domains, so NTP skew silently skips or double-applies deltas. Replay reconstruction also increments the *live* gap-detection metric and re-persists gap events, polluting production observability during offline replays.
- **Action:** Persist a non-resetting monotonic per-asset ingest ordinal (or WAL global offset) on every `BookEvent` and sort replay by it; replace `buffer_unordered` with an ordered merge over path-sorted files; align the checkpoint boundary to a single clock domain. Isolate replay-path metrics from the live recorder (or no-op them during replay).
- **Done when:** two replays of the same window produce byte-identical book state and identical integrity-event counts (determinism test); the 316-tie case orders pre-snapshot deltas before the snapshot; a replay does not move live gap metrics.
- **Progress (2026-06):** A.117 done — `sort_book_events` is a strict total order (timestamp → `ingest_ordinal` → sequence → side/price/size/source_event_id), so concurrent `buffer_unordered` reads cannot change output; permutation-invariance tests added. A.152 done — replay no longer touches the live gap recorder. **A.116 done** — added `EventProvenance.ingest_ordinal`, a process-monotonic counter stamped at the single ingest serialization point (ingest + auto-ingest drain loops), persisted through the WAL (codec v2, v1 rejected with drain hint), Parquet (`book_events.ingest_ordinal` column, nullable), and ClickHouse (`book_events` DDL + reader); replay sorts by it so a same-µs pre-snapshot delta sorts before its snapshot. Tested: sort-order, Parquet round-trip, codec v2. (Only A.142 single-clock-domain checkpoint boundary remains in this task — see proposal; lower priority.)

### [ ] P1-REPLAY-3 — Populate `checkpoint.wal_offset` and pass real WAL config to hydration
- **Severity:** high/medium · **Findings:** A.13, A.101, A.81, A.156
- **Files:** `crates/pb-replay/src/backfill.rs:141`, `crates/pb-api/src/hydration.rs:128`, `:153`
- **Problem:** `checkpoint.wal_offset` is never written (all 85 captured checkpoints are NULL), so every serve cold start replays the entire retained WAL; the skip arithmetic hardcodes the default `WalConfig` segment size, so any non-default `segment_size_mb` mis-skips records, and `global_offset` is non-monotonic for records larger than a segment.
- **Action:** Capture `WalWriter::global_offset()` at append time and persist it in `BookCheckpoint`; pass the full configured `WalConfig` into hydration; fix the offset math (or switch to a segment-id + intra-segment-offset pair).
- **Done when:** new checkpoints carry a non-NULL `wal_offset`; serve cold start replays only from the checkpoint offset (asserted by a test counting replayed records); a non-default segment size still resumes correctly.

### [ ] P1-BOOK-1 — Run crossed/locked-book integrity detection on live and replay paths
- **Severity:** medium · **Findings:** A.105, A.53, A.148, A.159
- **Files:** `crates/pb-book/src/book.rs:175` (`check_integrity` dead code), `:160` (`check_sequence` zero-sentinel)
- **Problem:** `check_integrity` (crossed/locked detection) has zero production callers; captured data contains real locked-book episodes nobody flagged. `check_sequence`'s zero-sentinel disables gap detection exactly post-snapshot/post-checkpoint where `sequence==0` is legitimate. The crossed-book proptest is vacuous (bids/asks use disjoint price ranges) and `fuzz_book_delta` never calls `check_integrity`.
- **Action:** Call `check_integrity` at message boundaries on the live and replay paths; emit a metric + a persisted integrity event on violation. Replace the zero-sentinel with `Option<Sequence>` and persist the book sequence in `BookCheckpoint`. Fix the proptest to allow overlapping ranges and have the fuzz target assert the invariant.
- **Done when:** a crossed/locked delta sequence produces an integrity event + metric on both paths; the proptest can generate crossed books and the invariant holds (or is reported); `check_sequence` no longer skips gap detection at `sequence==0`.

## Feed ingest correctness

### [x] P1-FEED-1 — Fix the `<=` stale-snapshot guard and atomic snapshot emission
- **Severity:** high · **Findings:** A.21, A.108
- **Files:** `crates/pb-feed/src/dispatcher.rs:172` (`exchange_ts <= last_ts`), `:200-202` (ts advanced before parse), `:217`, `:256` (per-level `?` aborts mid-batch)
- **Problem:** Two trades in the same millisecond produce two `book` snapshots with equal timestamps; the `<=` check drops the newer one (confirmed 7× in captured V2 data), and trade-induced changes only arrive via `book` events, so the state stays wrong until the next trade. Separately, a mid-message per-level conversion failure emits a truncated snapshot (e.g. bids only) that looks complete, and `last_snapshot_ts` was already advanced so retransmits are rejected — poisoning the tracker.
- **Action:** Skip only strictly older snapshots (`<`); dedupe true retransmits via the venue `hash` field. Convert all levels of a book/price_change message into a `Vec<BookEvent>` first and only then advance `last_snapshot_ts`, reset the sequence, and send; on conversion failure emit an `IngestEvent` (parse-failure / source-reset), never a partial snapshot. Snapshots with `exchange_ts==0` must not bypass the guard or skip the tracker update.
- **Done when:** a same-ms second snapshot is applied (test); a deliberately malformed level yields zero `BookEvent`s + one `IngestEvent`, and the tracker is not advanced.

### [x] P1-INGEST-1 — Make `auto-ingest` (the real production mode) write the WAL, with rotation overlap
- **Severity:** high/medium · **Findings:** A.19, A.75, A.98
- **Files:** `crates/pb-bin/src/commands/auto_ingest.rs:66` (no WAL writer), `:134` (rotation closes the old subscription before opening the new)
- **Problem:** `auto-ingest` is the continuous rotating-market production mode, but it never writes the WAL — breaking the documented `ingest→serve` topology so a serve replica has no live tail at all. Rotation also tears down the expiring market's subscription before subscribing to the next, dropping the final ~10 seconds of every 5-minute market (the highest-information endgame).
- **Action:** Wire the WAL writer (and its flush/fsync policy from P1-WAL-1) into `auto-ingest` exactly as `ingest` has it. Overlap subscriptions during rotation — subscribe to the next market before unsubscribing from the expiring one — so the endgame is captured. Reuse the loss-free drain template (P2-LIFE-1).
- **Done when:** an `auto-ingest` run produces a WAL a `serve` process can tail (integration test); a rotation test shows the last 10 s of the expiring market are captured with no gap.

## Numerics & data integrity

### [x] P1-NUM-1 — Replace f64 string parsing with exact integer-decimal fixed-point parsing
- **Severity:** medium · **Findings:** A.82, A.125, A.155, A.66
- **Files:** `crates/pb-types/src/fixed.rs:210`, `:170`, `:71`, `:112`
- **Problem:** All string→fixed-point parsing routes through `f64`: it breaks serde roundtrip above 2^53 raw (and the WAL bincode codec + checkpoint JSON use exactly this serde — a direct zero-data-loss violation), silently saturates oversized sizes to `u64::MAX`, and silently rounds sub-tick digits. The same path rounds execution fill prices by up to half a tick.
- **Action:** Parse decimals as integers (split on `.`, scale by 10^4 / 10^6, reject overflow and excess precision explicitly). Apply to price, size, and the execution append path.
- **Done when:** a proptest asserts `parse(display(x)) == x` for all representable `FixedPrice`/`FixedSize` including values > 2^53 raw; oversized/over-precise inputs return an error instead of saturating/rounding.

### [x] P1-NUM-2 — Enforce the `FixedPrice` range invariant and enable release overflow checks
- **Severity:** medium/low · **Findings:** A.154, A.140, A.146, A.149
- **Files:** `crates/pb-types/src/fixed.rs:44` (public tuple field / `new_unchecked`), `Cargo.toml:104` (`overflow-checks` off in release), `crates/pb-book/src/book.rs:97` (unchecked `u64` running totals)
- **Problem:** The `FixedPrice` range invariant is bypassable via the public field, and out-of-range values serialize but fail to deserialize (write-OK / read-FAIL poison). `overflow-checks` is off in release while `L2Book` totals use unchecked `u64` arithmetic reachable from feed-controlled saturated sizes — wraps silently in production, panics in debug.
- **Action:** Make the field private (construct only via validated constructors). Set `overflow-checks = true` in `[profile.release]` (and `[profile.bench]`), or switch the hot-path sums to `checked_add`/`saturating_add` with an integrity signal on overflow.
- **Done when:** out-of-range `FixedPrice` cannot be constructed via public API; a test that previously wrapped now panics/errors in release config.

## Production deploy (blocks all of the above from mattering)

### [ ] P1-INFRA-1 — Make the deployed system actually persist data
- **Severity:** critical/high · **Findings:** A.1, A.15, A.2, A.55
- **Files:** `crates/pb-bin/src/commands/pipeline.rs:55` (always `LocalFileSystem`), `infra/ecs.tf:36` (`s3://` path + missing `--tokens`, no volume), `Dockerfile:11`/`:20`, `.github/workflows/deploy.yml`
- **Problem:** `start_storage_sinks` unconditionally builds `LocalFileSystem`, so the `s3://<bucket>/orderbook` env value becomes a literal local dir `s3:` on **ephemeral** Fargate storage — every restart destroys all history. No code constructs `AmazonS3` despite the `aws` feature being on. The ECS task passes no `--tokens` so it crash-loops; the Docker image can't build (workspace member, `rust-version`/MSRV, missing `protoc`, missing `libssl3` at runtime); the deploy workflow has failed on every push to `main`.
- **Action:** Parse the storage URL scheme and build the matching `object_store` backend (`object_store::parse_url` / `AmazonS3Builder` for `s3://`). Mount durable storage (EFS) for the WAL. Fix the ECS command (use `auto-ingest` or supply `--tokens`). Fix the Dockerfile (workspace build, toolchain ≥ MSRV, `protobuf-compiler`, runtime `libssl3` or move to rustls per P2-BUILD-3). Make the deploy workflow green.
- **Done when:** an integration test asserts an `s3://` base path does *not* silently create a local dir; `docker build` succeeds in CI (P2-CI-1); a deploy to a scratch environment persists Parquet to S3 and survives a task restart.

## Tests that lock Phase 1 in

### [ ] P1-TEST-1 — Add crash-recovery, reconnect-gap, and golden-codec tests; put integration suite in CI
- **Severity:** high/medium · **Findings:** A.33, A.135, A.136, A.137, A.139, A.51
- **Files:** `.github/workflows/ci.yml:33` (integration package excluded), `crates/pb-wal/src/codec.rs:286` (no golden fixture), `crates/pb-feed/src/ws.rs:275`, `crates/pb-replay/src/reader.rs:69` (no Parquet schema version)
- **Problem:** The entire integration package is excluded from CI (so hydration/replay/roundtrip/determinism regressions pass green), the highest-stakes crash paths are untested, WAL codec roundtrip uses single hand-picked values with no version-compat golden fixture (bincode's positional encoding means a field reorder silently changes the v1 format), and there's no Parquet schema-version metadata (the pre-split 2026-03-06 capture is silently unreadable).
- **Action:** Add the integration package to CI (with a Docker-enabled job for ClickHouse). Add: writer-crash-mid-append recovery (pairs with P1-WAL-2), WS-reconnect-with-gap, kill-and-restart durability. Commit golden byte fixtures for every `PersistedRecord` codec variant. Add a Parquet schema-version field + a reader that rejects/migrates old layouts.
- **Done when:** `pb-integration-tests` runs in CI and is required; golden-fixture tests fail if the on-disk codec layout changes; a schema-version mismatch is a typed error, not a silent empty read.

---

# Phase 2 — Operational Hardening

*Make it safe to run unattended: nothing degrades silently, abuse and exposure are bounded, on-call can see and act.*

## Resilience & lifecycle

### [x] P2-SUP-1 — Supervise every spawned task; surface projector/tailer death
- **Severity:** medium · **Findings:** A.45, A.99, A.100, A.48, A.50
- **Files:** `crates/pb-api/src/live_state.rs:642` (dead projector still advances WAL positions), `crates/pb-bin/src/commands/ingest.rs:49`, `crates/pb-bin/src/commands/serve.rs:189` (tailer exits on resync/open failure, no recovery), `crates/pb-bin/src/commands/serve_api.rs:274`
- **Problem:** Dropped/await-only `JoinHandle`s swallow panics; a dead projector keeps the WAL tailer committing consumer positions for records that were never applied; the serve tailer terminates permanently on resync or reader-open failure while `/health` still reports ready; rotation reaping risks market-start gaps or orphaned tasks.
- **Action:** Put all spawned tasks under a `JoinSet`/supervisor; treat unexpected exit/panic as fatal-or-restart with a metric/alert. Make `LiveReadModel::apply_record` fail when the projector channel is closed so the tailer stops committing positions. Add a tailer recovery loop (re-hydrate on resync, retry reader open with backoff) and reflect not-ready in `/health` (pairs with P2-API-3).
- **Done when:** a panicking child task causes a non-zero exit or a logged+metered restart (test); a closed projector stops position commits; a tailer that hits resync re-hydrates instead of dying.

### [x] P2-LIFE-1 — Make graceful shutdown loss-free
- **Severity:** medium · **Findings:** A.44, A.97, A.104
- **Files:** `crates/pb-bin/src/commands/ingest.rs:120` (breaks immediately on cancel, drops up to 2048 buffered events + 2048 dispatcher frames), `:104` (abandons slow sink flushes after 10 s)
- **Problem:** `ingest` drops queued events on cancel before WAL/sink write; `auto-ingest` already gets this right and should be the shared template. Shutdown abandons slow sink flushes after 10 s and ignores further signals.
- **Action:** On shutdown, drain the channels (bounded by a deadline) into the WAL/sinks before exiting; reuse the `auto-ingest` drain template. Make the flush deadline configurable and log/meter abandoned records.
- **Done when:** a shutdown-under-load test asserts all in-flight events reach the WAL (or are counted as abandoned with a metric), not silently dropped.

### [x] P2-WAL-PRUNE — Wire WAL pruning and enforce `max_segments`
- **Severity:** medium · **Findings:** A.17, A.20, A.47, A.127, A.128, A.9
- **Files:** `crates/pb-wal/src/writer.rs:75` (`prune`/`prune_with_backpressure` have no callers), `crates/pb-bin/src/commands/pipeline.rs:156`, `crates/pb-wal/src/segment.rs:33` (no writer mutual exclusion)
- **Problem:** Pruning is implemented but never invoked; `wal.max_segments` is dead config; the WAL grows unbounded until disk-full, after which appends silently fail. Pruner safety depends on a manually-supplied consumer list and ignores malformed position files. No `flock`, so two ingest processes can interleave appends and `Segment::create(truncate=true)` can wipe a populated segment.
- **Action:** Invoke pruning on a cadence/backpressure trigger driven by persisted consumer positions; enforce `max_segments`; harden the pruner against missing/malformed position files (never prune below the slowest live consumer). Add an advisory `flock` on the WAL directory so a second writer fails fast.
- **Done when:** a long-running ingest test shows bounded segment count; a prune-vs-slow-consumer test never deletes an unread segment; a second writer on the same dir is rejected.

## Security & exposure

### [x] P2-SEC-1 — Close the SQL workbench SSRF / arbitrary-file-read hole
- **Severity:** high (impact) · **Findings:** A.24, A.130, A.43, A.120, A.83, A.91, A.121, A.89
- **Files:** `crates/pb-service/src/query.rs:181` (keyword blocklist, no allowlist), `:243`, `:327`, `crates/pb-api/src/server.rs:447`
- **Problem:** The guard is a write-keyword blocklist with no table/function allowlist, so `SELECT * FROM file('/etc/passwd',…)` / `url('http://169.254.169.254/…')` / `s3(…)` / `remote(…)` / `system.users` all pass, against ClickHouse with no `readonly` mode and no auth on a `0.0.0.0` bind. `max_rows` is client-controlled and unclamped; any `LIMIT` token (even in a subquery/CTE) suppresses the injected limit; the timeout covers only response headers, not body download. The query-guard fuzz target was removed from CI with known unfixed bypasses.
- **Action:** Run the workbench as a ClickHouse `readonly=2` user with table-function and `system.*` access revoked. In the guard: **allowlist** datasets/tables, reject table functions and `SETTINGS`, clamp `max_rows` server-side, enforce a top-level `LIMIT`, and set `max_execution_time`/`max_result_rows` on the CH side. Extend the request timeout to cover body download (reqwest overall timeout). Require auth before this is reachable on any non-loopback interface (pairs with P2-SEC-2). Re-enable `fuzz_query_guard` in CI with a cached corpus.
- **Done when:** the bypass corpus (`file`/`url`/`s3`/`remote`/`system`/comment/CTE/SETTINGS/multi-statement) is rejected by tests; a CH `readonly=2` user is used; `fuzz_query_guard` runs in CI.

### [ ] P2-SEC-2 — Default all surfaces to loopback; add auth; document the trust boundary
- **Severity:** medium/low · **Findings:** A.93, A.157, A.151, A.86, A.131
- **Files:** `config/default.toml:35`, `crates/pb-api/src/server.rs:112`, `crates/pb-grpc/src/lib.rs`, `infra/vpc.tf:53` (metrics `0.0.0.0/0`)
- **Problem:** No auth on any surface; HTTP (3000), gRPC (50051), and metrics (9090) all bind `0.0.0.0`; the trust boundary is undocumented; metrics are exposed to the public internet on a public-IP Fargate task.
- **Action:** Default binds to loopback (or an explicit private interface); add an auth layer (or document that the only supported deployment is behind an authenticating proxy on a private network). Restrict the metrics security-group ingress to the scraper CIDR; move tasks to private subnets (pairs with P2-INFRA-1). State the trust boundary in `docs/api.md` and `docs/serve-api.md`.
- **Done when:** default config binds loopback; metrics ingress is not `0.0.0.0/0`; the trust boundary is documented per `CLAUDE.md`'s "Persisting Design Decisions" rule.

### [ ] P2-SEC-3 — Supply-chain & container hardening
- **Severity:** medium/low · **Findings:** A.132, A.133, A.134, A.158, A.36, A.141
- **Files:** `.github/workflows/deploy.yml:29` (mutable tag/branch action refs), `infra/iam.tf:116` (`Resource="*"`), `infra/s3.tf:1` (`force_destroy=true`, no versioning, SSE-S3 not KMS), `Dockerfile:20` (root, tag-only base), `deny.toml`
- **Problem:** Actions pinned to mutable tags/branches (not SHAs); deploy IAM grants ECS `UpdateService`/`RegisterTaskDefinition` on `Resource="*"`; the S3 data bucket has `force_destroy=true`, no versioning, SSE-S3; the runtime image runs as root with tag-only base images; `deny.toml` disables unmaintained-crate detection and allows unlimited duplicate versions.
- **Action:** Pin all actions to commit SHAs; scope the deploy IAM to the specific cluster/service/task-def ARNs; enable S3 versioning, set `force_destroy=false`, switch to SSE-KMS; run the container as non-root with digest-pinned base images; turn on `deny.toml` unmaintained detection.
- **Done when:** `supply-chain.yml`/`cargo-deny` flags none of the above; Terraform plan shows versioning on + `force_destroy=false`; the image runs as a non-root UID.

## API & gRPC hardening

### [ ] P2-API-1 — Add timeouts, concurrency caps, and response-size caps to historical routes
- **Severity:** medium · **Findings:** A.92, A.42, A.118
- **Files:** `crates/pb-api/src/server.rs:290`, `crates/pb-service/src/lib.rs:124`, `crates/pb-replay/src/reader.rs:962` (epoch-scan DoS)
- **Problem:** Historical routes have no per-request timeout, concurrency limit, or response-size cap and buffer a full 24 h window in RAM; `continuity_events` is unbounded; integrity/replay reads pull whole windows over HTTP with no `LIMIT` then re-sort in Rust; `read_latest_checkpoint` does an unbounded epoch-scan (~1M FS ops) for a checkpoint-less asset — a cheap DoS via the public replay route.
- **Action:** Add `TimeoutLayer` + `ConcurrencyLimitLayer`; cap response arrays and the queryable window; push `LIMIT`/aggregation server-side; bound the checkpoint scan.
- **Done when:** an oversized window request is rejected with a typed 4xx; a checkpoint-less asset returns promptly without an epoch scan (test).

### [x] P2-API-2 — Bound WebSocket fan-out
- **Severity:** medium · **Findings:** A.94, A.49
- **Files:** `crates/pb-api/src/streaming.rs:96`, `crates/pb-feed/src/ws.rs:88`/`:224`
- **Problem:** WS fan-out is unbounded — no connection cap, no heartbeat/idle timeout, default ~64 MB message limit; half-open peers on quiet assets leak sessions. (The client-side feed reconnect backoff also never resets and jitter is nullified at the cap — fix alongside.)
- **Action:** Cap concurrent connections per asset/total; add server heartbeat + idle timeout; lower the max message size; reset the feed reconnect attempt counter after a successful session and keep jitter at the cap.
- **Done when:** a connection-flood test is bounded; an idle half-open peer is reaped; reconnect backoff returns to base after a healthy session.

### [x] P2-FEED-1 — Add a feed-liveness watchdog and fix reconnect backoff
- **Severity:** medium/low · **Findings:** A.107, A.106, A.150
- **Files:** `crates/pb-feed/src/ws.rs:186` (pongs ignored, no read-idle timeout), `:88`/`:224` (attempt counter never resets, jitter nullified at cap)
- **Problem:** Pongs are ignored and there's no read-idle timeout, so a half-open TCP connection stalls the feed silently for 15+ minutes while the process looks healthy. The reconnect `attempt` counter only increments for the process lifetime, degrading to a fixed 30 s gap per disconnect after ~9 cumulative disconnects, and jitter is nullified at the cap (synchronizing reconnects exactly when a thundering herd matters).
- **Action:** Add a read-idle/heartbeat watchdog (track pongs; force reconnect on no traffic within a deadline). Reset `attempt` to 0 after a successful, stable session; keep jitter active at the cap. Emit a feed-staleness metric (pairs with P2-OBS-1).
- **Done when:** a simulated half-open connection triggers a reconnect within the deadline (test); backoff returns to base after a healthy session; jitter is non-zero at the cap.

### [x] P2-API-3 — Sanitize error responses; split health endpoints
- **Severity:** medium · **Findings:** A.95, A.96, A.112
- **Files:** `crates/pb-api/src/error.rs:39`, `crates/pb-api/src/server.rs:157`, `crates/pb-grpc/src/lib.rs:258`/`:255`
- **Problem:** Internal error details (ClickHouse URLs, storage errors) are returned verbatim to unauthenticated clients and 500s are never logged; `/health` returns 200 when not ready (breaking status-code probes); the gRPC server logs "bound" before binding and swallows bind failures, so `serve` silently runs without gRPC.
- **Action:** Map internal errors to opaque client messages + log server-side with a correlation id; split `/health/live` and `/health/ready` with correct status codes; make gRPC bind failure fatal and log after a successful bind.
- **Done when:** a forced internal error returns no internals and is logged; `/health/ready` returns non-200 until hydrated; a gRPC bind failure exits non-zero.

### [x] P2-GRPC-1 — Move input validation into pb-service so gRPC can't bypass it
- **Severity:** high (impact) · **Findings:** A.22, A.64
- **Files:** `crates/pb-grpc/src/lib.rs:107`, `:187`
- **Problem:** gRPC RPCs bypass every HTTP-layer guard: a far-future `end_us` drives `hour_paths` into billions of iterations (remote OOM); `limit` is unclamped; `ExecutionTimeline` buffers the whole window.
- **Action:** Move range/limit/window validation into the `pb-service` traits so both HTTP and gRPC inherit it; add gRPC request timeout, concurrency, and message-size limits.
- **Done when:** a hostile `end_us`/`limit` over gRPC is rejected the same way as over HTTP (shared test).

## Observability & config

### [ ] P2-OBS-1 — Add the metrics on-call actually needs; commit alerts/dashboards/runbook
- **Severity:** medium · **Findings:** A.85, A.113, A.114, A.115
- **Files:** `crates/pb-metrics/src/recorder.rs:4` (no gauges), `crates/pb-metrics/src/server.rs:9` (60 s summaries, no buckets), `crates/pb-bin/src/commands/pipeline.rs:20` (no `run_upkeep`)
- **Problem:** Zero gauges — no feed-staleness, WAL consumer-lag (only in `/health` JSON), WAL disk/segment count, channel depth, or `wal_append_failures`/`sink_flush_failures`; no end-to-end recv→durable latency. "Histograms" are 60 s rolling summaries (no buckets), so quantiles can't aggregate across processes and the 5-min Parquet sample expires before scrape. No `run_upkeep` task, so buckets grow unbounded if scraping stalls.
- **Action:** Add gauges (feed staleness, WAL lag, disk/segments, channel depth) and failure counters; configure explicit histogram buckets for a recv→durable latency metric; add `run_upkeep`. Commit `monitoring/` with Prometheus alert rules (feed staleness, WAL lag, sink failure, crossed-book, disk) + dashboards + a `RUNBOOK.md`.
- **Done when:** `/metrics` exposes the gauges and bucketed histogram; `monitoring/` alert rules exist and load; a runbook covers each alert.

### [x] P2-CONF-1 — Make config fail-fast and wire (or delete) dead keys
- **Severity:** medium · **Findings:** A.102, A.103, A.54, A.87
- **Files:** `crates/pb-bin/src/main.rs:146` (missing `--config` ignored), `:49` (boolean toggles can't be disabled), `:166`
- **Problem:** Missing `--config` is ignored; parse/type errors and negative ints are swallowed via `unwrap_or`/`as`; the boolean `--parquet`/`--metrics` toggles literally cannot be disabled (so the documented two-process recipe collides on port 9090); `wal.max_segments`, `storage.parquet_row_group_size`, `storage.clickhouse_batch_*`, and `logging.format` are dead config (JSON logging is impossible).
- **Action:** Typed deserialize with fail-fast on parse/range errors; require `--config` when explicitly passed; make boolean toggles disable-able (`--no-metrics` or `--metrics=false`); wire the dead keys (incl. `logging.format` → JSON logging) or remove them and update docs.
- **Done when:** a malformed/out-of-range config aborts with a clear error; the two-process recipe runs without a port collision; `logging.format=json` produces JSON logs (or the key is gone).

## Infra, CI, frontend, execution, build, docs

### [ ] P2-INFRA-1 — Bring deployed topology in line with the documented architecture
- **Severity:** medium · **Findings:** A.84, A.88, A.41
- **Files:** `infra/ecs.tf:13` (single `FARGATE_SPOT`, no CH, no serve, no health check, no circuit breaker), `infra/s3.tf:1`
- **Problem:** Infra is a single Fargate Spot task (routine reclaim = capture gap) with no ClickHouse, no `serve` service, no EFS, no health check, no circuit breaker — zero backing for the multi-replica WAL story. No data retention anywhere (no CH TTL, no Parquet/WAL expiry; `force_destroy=true`).
- **Action:** Move ingest to on-demand (or dual-AZ) capacity with a health check + deployment circuit breaker; provision ClickHouse and a `serve` service; mount EFS (pairs with P1-INFRA-1); add data-retention (CH TTL, Parquet/WAL lifecycle, S3 lifecycle) and remove `force_destroy=true` (pairs with P2-SEC-3).
- **Done when:** a Spot reclaim no longer drops capture; retention policies exist; Terraform plan matches the documented topology (or docs are corrected to match — pairs with P2-DOCS-1).

### [ ] P2-CI-1 — Close the CI gaps
- **Severity:** medium · **Findings:** A.90, A.138, A.145
- **Files:** `.github/workflows/ci.yml:129`
- **Problem:** No docker-build gate, no coverage gate, no Criterion perf-regression gate, 30 s smoke-only fuzz, releases ship no artifacts or smoke test; fuzzing stops at serde (dispatcher normalization, `codec::decode`, config parsing unfuzzed; corpus not cached); the execution CLI round-trip tests are excluded.
- **Action:** Add a docker-build gate, a coverage ratchet, a Criterion regression gate vs baselines, longer fuzz + cached corpus + new targets (dispatcher, codec, config), and a release smoke test. (Integration suite handled in P1-TEST-1.)
- **Done when:** each gate runs and is required; new fuzz targets exist with a cached corpus.

### [ ] P2-FE-1 — Fix the WebSocket lifecycle and data-correctness bugs
- **Severity:** high/medium · **Findings:** A.10, A.67, A.68, A.69, A.70, A.71, A.73
- **Files:** `web/src/shared/hooks/use-orderbook-stream.ts:33`/`:78`/`:116`, `web/vite.config.ts:28`, `web/src/shared/api/queries.ts:43`, `web/src/app/error-boundary.tsx:29`, `web/src/shared/api/client.ts:5`
- **Problem:** A shared `unmountedRef` reset across effect runs lets the old socket's `onclose` clobber the new socket, leak the connection, and spawn a ghost reconnect loop on every asset switch and under StrictMode. No WS staleness/heartbeat detection (a ~50 s blip downgrades to HTTP polling forever; frozen WS data is preferred over fresh HTTP under a green "live" badge). The WS→TanStack-Query bridge writes the wrong key (`bids.length` as depth). The dev proxy lacks `ws:true` (WS is dead in dev/e2e, which is why these hide). Source mode isn't in query keys (demo/live cache pollution); the route ErrorBoundary never resets on navigation; a 4 s hard timeout aborts legitimate SQL queries.
- **Action:** Use a per-run `cancelled` local + socket instance-identity check; add staleness/heartbeat detection and recovery from fallback; fix the cache key; add `ws:true` to the dev proxy; include source mode in query keys; reset the ErrorBoundary on navigation; remove/scope the 4 s timeout for the workbench.
- **Done when:** an asset-switch/StrictMode test shows no leaked sockets or ghost reconnects; a staleness test surfaces stale data and recovers; WS streaming works in `vite dev`.

### [ ] P2-FE-2 — Frontend accessibility
- **Severity:** medium · **Findings:** A.72
- **Files:** `web/src/shared/components/command-palette.tsx:35`
- **Problem:** Command palette lacks dialog semantics/focus trap; sort headers are mouse-only; lazy-route heading focus races.
- **Action:** Add `role="dialog"`/focus trap to the palette; make sort headers keyboard-operable; fix the heading-focus timing on lazy routes.
- **Done when:** biome a11y checks pass and a keyboard-only pass can operate the palette and sort.

### [ ] P2-EXEC-1 — Harden the execution write path
- **Severity:** medium/low · **Findings:** A.60, A.61, A.62, A.63, A.65, A.124, A.144
- **Files:** `crates/pb-bin/src/commands/execution_append.rs:209`/`:179`/`:103`, `crates/pb-types/src/event.rs:208`, `crates/pb-replay/src/reader.rs:1020`, `crates/pb-service/src/lib.rs:163`, `crates/pb-store/src/writer.rs:534`
- **Problem:** The only operator-driven write path is unguarded: no idempotency (retries double-count fills in `MergeTree`, overwrite in Parquet; multi-table CH flush is non-atomic); no timestamp unit/range validation (ms/s lands in a 1970 partition, invisible); no `LatencyTrace` monotonicity check (web renders negative durations); no timeline tie-break (nondeterministic, backend-divergent); no pagination (oldest-first truncation hides recent events); no order-lifecycle validation (fill-after-cancel etc. accepted). (Fill-price rounding handled in P1-NUM-1.)
- **Action:** Add a content-derived `event_id` + `ReplacingMergeTree`/dedup token; validate timestamp units/range; add `LatencyTrace` monotonicity checks; deterministic timeline tie-break; server-side pagination; basic order-lifecycle state validation.
- **Done when:** a re-run of `execution-append` does not duplicate rows (test); an out-of-unit timestamp is rejected; timeline ties are deterministic across both backends; pagination reaches events beyond the first 200.
- **Progress (2026-06):** A.61 (timestamp range validation), A.62 (LatencyTrace monotonicity), A.63 (deterministic timeline tie-break), A.65 (server-side offset/order pagination across service+HTTP+gRPC+web), and **A.144 done** — `validate_execution_lifecycle` rejects within-batch incoherence (event-after-terminal, fill-after-cancel, ack-before-submit, duplicate SubmitIntent, cumulative fill > submitted size) plus the earlier empty-order_id / fill-without-price-size checks; `--skip-lifecycle-checks` escape hatch for partial backfills; 8 tests. **Remaining: A.60/A.124** (ClickHouse at-least-once-without-duplicates — content-derived event_id + ReplacingMergeTree/`insert_deduplication_token` + per-table retry tracking). The Parquet append path is already idempotent for identical re-runs via content-hashed filenames (A.122); the ClickHouse dedup change is **env-blocked** (needs a live ClickHouse to verify RowBinary/ReplacingMergeTree behavior).

### [ ] P2-BUILD-1 — Remove `target-cpu=native`; document the release profile; align toolchains
- **Severity:** medium · **Findings:** A.34, A.35, A.37
- **Files:** `.cargo/config.toml:1`, `Cargo.toml:104`, `rust-toolchain.toml:2`, `Dockerfile`
- **Problem:** Checked-in `target-cpu=native` makes binaries non-reproducible and SIGILL-prone, is overridden by CI `RUSTFLAGS`, and never reaches Docker — so it only applies on dev laptops. The release profile (`panic=abort` + `strip=symbols`, no debuginfo) changes failure semantics from per-task isolation to whole-process abort, makes crashes untriageable, is undocumented (no ADR), and is never compiled in CI. Four-way toolchain skew (pin 1.94.0, CI `@stable`, Docker 1.93 below MSRV, fuzz/miri `@nightly`) with no MSRV job.
- **Action:** Remove `target-cpu=native` (pin an explicit microarch floor in the Docker build matching the fleet, applied consistently in CI/bench). Add line-table debuginfo to the release profile and write an ADR documenting `panic=abort`. Align toolchains and add an MSRV verification job; build `--release` in CI.
- **Done when:** binaries are reproducible across hosts; an ADR records the release-profile rationale; CI compiles `--release` and verifies MSRV.

### [ ] P2-BUILD-3 — Unify on rustls; pin the WAL codec format
- **Severity:** low/medium · **Findings:** A.141, A.36
- **Files:** `Cargo.toml:43`, `crates/pb-wal/src/codec.rs:17`
- **Problem:** The production binary carries two HTTP stacks and two TLS implementations; the latency-critical WS feed alone rides OpenSSL (also the missing-`libssl3` Docker runtime failure). The WAL on-disk format rests on frozen bincode 1.3.3 positional encoding with only self-consistent roundtrip tests.
- **Action:** Move `tokio-tungstenite` to rustls (removes the OpenSSL runtime dep, fixes the Docker failure). Pin the codec with golden byte fixtures (pairs with P1-TEST-1) and pin the bincode version.
- **Done when:** the binary links one TLS stack; golden codec fixtures guard the on-disk format.

### [ ] P2-DOCS-1 — Reconcile docs and OpenSpec with reality
- **Severity:** medium/low · **Findings:** A.56, A.57, A.58, A.59, A.55, A.143
- **Files:** `docs/operations.md` (pruning/`WalPruner`, dead config keys, deploy flow), `docs/api.md:166` (undocumented 24 h cap), `docs/serve-api.md` (wrong health route), `docs/architecture.md:168` (ParquetSink in serve-api that doesn't exist), `openspec/changes/archive/.../tasks.md`, `justfile:49`
- **Problem:** Docs describe a live `WalPruner` that doesn't exist, four dead config keys, a Docker/ECS flow that can't build, a 4-surface SPA when 6 ship (incl. `/query`), the wrong health route, and an architecture diagram with a non-existent sink. Archived OpenSpec tasks are checked complete for `WalPruner` and a benchmark gate that never shipped. `just replay` always fails (missing `--mode`); DuckDB helpers target the pre-split schema.
- **Action:** After Phases 1–2 land the real behavior, do a documentation reconciliation pass per `CLAUDE.md`'s "Persisting Design Decisions" + each crate README's "Docs to Update After Changes" table. Fix the `justfile` recipes.
- **Done when:** every doc claim matches shipped behavior; `just replay` / DuckDB helpers run; OpenSpec archive reflects what actually shipped.

### [ ] P2-CH-SCHEMA — ClickHouse schema & query tuning
- **Severity:** medium · **Findings:** A.38, A.39, A.40
- **Files:** `crates/pb-store/src/writer.rs:25`/`:37`, `crates/pb-store/src/clickhouse_sink.rs:12`
- **Problem:** `execution_events` primary key doesn't match the timeline query filter (full scans); high-repetition string columns (`asset_id`/`source`/`mode`/`status`) are plain `String` not `LowCardinality`/`Enum`; insert strategy ignores async-insert guidance and the documented batch-tuning knobs are dead config. (Cross-check against `clickhouse-best-practices` rules: `schema-pk-prioritize-filters`, `schema-types-lowcardinality`, `insert-async-small-batches`.)
- **Action:** Reorder/redefine primary keys to match query filters; convert high-repetition columns to `LowCardinality`/`Enum`; adopt `async_insert` (or wire the batch knobs). Cite the specific best-practice rule in each change.
- **Done when:** timeline queries use the index (verify with `EXPLAIN`); column types follow the rules; batch config is honored or removed.

---

# Phase 3 — HFT-Grade Capabilities

*Close the distance to the bar: venue-anchored correctness, failover, and continuous regression discipline.*

### [ ] P3-SEQ-1 — Venue-anchored sequencing & gap-fill
- **Severity:** medium · **Findings:** A.74, A.109, A.110, A.111
- **Files:** `crates/pb-feed/src/dispatcher.rs:362`/`:156`, `crates/pb-feed/src/rest.rs:32`
- **Problem:** Sequences are locally synthesized (gap-free by construction), so `SequenceGap`/`record_gap_detected()` are dead at ingest and silent WS loss is undetectable; the venue `hash`/`best_bid`/`best_ask` fields are parsed but never validated; unknown messages are dropped at debug with no metric; `RestClient` has no HTTP timeouts.
- **Action:** Validate the venue book `hash` after each delta; on mismatch emit `SequenceGap`/`BookMismatch` + trigger a REST resnapshot and record the reconnect window as a queryable data hole. Meter unknown-message drops. Add `RestClient` timeouts.
- **Done when:** an injected dropped-message scenario is detected and resnapshotted (integration test); data holes are queryable; a hung REST call times out.

### [ ] P3-TIME-1 — Time discipline
- **Severity:** medium · **Findings:** A.76, A.147, A.119
- **Files:** `crates/pb-feed/src/ws.rs:235`, `crates/pb-feed/src/dispatcher.rs:410`/`:423`, `crates/pb-replay/src/backfill.rs`
- **Problem:** Wall-clock only — no monotonic source, replay order can diverge from live apply order; divergent ms/µs heuristics misclassify seconds/nanosecond inputs and map `"0"` to a 1970 partition (the two copies disagree on the zero case); backfill silently falls back to 1970 on seconds-resolution timestamps.
- **Action:** Stamp a global monotonic arrival ordinal at the dispatcher and order replay by it (pairs with P1-REPLAY-2); centralize one validated ms/µs converter; alarm on negative clock skew; document NTP/PTP requirements and acceptable skew.
- **Done when:** replay order is provably the live apply order; one converter is used everywhere with tests for s/ms/µs/ns and zero; skew beyond threshold alerts.

### [ ] P3-HA-1 — Failover & flow-control policy
- **Severity:** medium · **Findings:** A.78, A.79
- **Files:** `crates/pb-bin/src/commands/serve.rs:189`, `crates/pb-bin/src/commands/ingest.rs:43`
- **Problem:** Single feed, single writer, manual recovery on WAL resync, multi-replica explicitly deferred with no RTO. Channel capacities are hard-coded (2048/10000) with whole-pipeline head-of-line blocking as the only flow-control mode and no stated load-shedding policy.
- **Action:** Add feed redundancy with arbitration; ingest writer leasing/lock so a standby can take over the shared WAL; in-process re-hydration; documented RTOs. Define and document the flow-control policy (WAL is the only unconditionally-blocking consumer; sinks may lag with alerting; channel capacities sized from measured rotation bursts with depth gauges).
- **Done when:** a writer-failover test shows a standby resumes from the shared WAL within the stated RTO; channel sizing has a documented rationale + depth gauges.

### [ ] P3-MON-1 — Live data-quality monitors that page within seconds
- **Severity:** medium · **Findings:** A.77
- **Files:** `crates/pb-api/src/live_state.rs:258`
- **Problem:** Gap/staleness/crossed-book conditions are computed but nothing pages, and `check_integrity` is never run live (wiring done in P1-BOOK-1; this is the alerting layer).
- **Action:** Wire live monitors (crossed-book, stale feed, WAL lag, sink failure, REST-vs-WS divergence reconciliation) to the alerting layer from P2-OBS-1 with second-scale paging.
- **Done when:** each condition fires a test alert within seconds in a simulated incident.

### [ ] P3-CHG-1 — Change safety: replay-based regression, canary, schema evolution
- **Severity:** medium · **Findings:** A.80, A.51
- **Files:** `tests/integration/book_determinism.rs` (new), `crates/pb-replay/src/reader.rs:69`
- **Problem:** No replay-based regression against captured data, no shadow/canary deployment, `:latest` image deploys, untested codec/schema evolution.
- **Action:** Add a golden-WAL replay regression in CI (byte-identical book + integrity-event counts vs a captured fixture); deploy image-digest-pinned with staged rollout + shadow/canary diffing; add cross-version codec fixtures (P1-TEST-1) + Parquet/ClickHouse schema-version metadata and a documented migration procedure.
- **Done when:** a book-logic change that alters replay output fails CI; deploys are digest-pinned; a schema-version bump has a tested migration path.

### [ ] P3-EXEC-2 — Execution lifecycle state machine & exact-decimal prices
- **Severity:** low · **Findings:** A.144 (state machine), A.66 (covered by P1-NUM-1)
- **Files:** `crates/pb-bin/src/commands/execution_append.rs:103`
- **Problem:** No order-lifecycle state-machine validation (fill-after-cancel, ack-before-submit, oversized fills, empty `order_id`, intra-order timestamp inversions all accepted silently).
- **Action:** Validate order-lifecycle transitions on append; reject illegal transitions. (Exact-decimal fill prices land in P1-NUM-1.)
- **Done when:** illegal lifecycle transitions are rejected with a typed error (test).

---

# Coverage map (every finding → task)

> All 159 confirmed findings are accounted for. Low/info items are folded into the task that shares their root cause.

| Task | Findings closed |
|---|---|
| P1-WAL-1 | A.11, A.16, A.18, A.29, A.46, A.113, A.129 |
| P1-WAL-2 | A.7, A.30, A.33, A.126 |
| P1-WAL-3 | A.6, A.14, A.31, A.32 |
| P1-STORE-1 | A.3, A.4, A.25 |
| P1-STORE-2 | A.5, A.12, A.26 |
| P1-STORE-3 | A.27, A.28, A.122, A.123, A.153 |
| P1-REPLAY-1 | A.8, A.23, A.52 |
| P1-REPLAY-2 | A.116, A.117, A.142, A.152 |
| P1-REPLAY-3 | A.13, A.81, A.101, A.156 |
| P1-BOOK-1 | A.53, A.105, A.148, A.159 |
| P1-FEED-1 | A.21, A.108 |
| P1-INGEST-1 | A.19, A.75, A.98 |
| P1-NUM-1 | A.66, A.82, A.125, A.155 |
| P1-NUM-2 | A.140, A.146, A.149, A.154 |
| P1-INFRA-1 | A.1, A.2, A.15, A.55 |
| P1-TEST-1 | A.33, A.51, A.135, A.136, A.137, A.139 |
| P2-SUP-1 | A.45, A.48, A.50, A.99, A.100 |
| P2-LIFE-1 | A.44, A.97, A.104 |
| P2-WAL-PRUNE | A.9, A.17, A.20, A.47, A.127, A.128 |
| P2-SEC-1 | A.24, A.43, A.83, A.89, A.91, A.120, A.121, A.130 |
| P2-SEC-2 | A.86, A.93, A.131, A.151, A.157 |
| P2-SEC-3 | A.36, A.132, A.133, A.134, A.141, A.158 |
| P2-API-1 | A.42, A.92, A.118 |
| P2-API-2 | A.49, A.94 |
| P2-FEED-1 | A.106, A.107, A.150 |
| P2-API-3 | A.95, A.96, A.112 |
| P2-GRPC-1 | A.22, A.64 |
| P2-OBS-1 | A.85, A.113, A.114, A.115 |
| P2-CONF-1 | A.54, A.87, A.102, A.103 |
| P2-INFRA-1 | A.41, A.84, A.88 |
| P2-CI-1 | A.90, A.138, A.145 |
| P2-FE-1 | A.10, A.67, A.68, A.69, A.70, A.71, A.73 |
| P2-FE-2 | A.72 |
| P2-EXEC-1 | A.60, A.61, A.62, A.63, A.65, A.124, A.144 |
| P2-BUILD-1 | A.34, A.35, A.37 |
| P2-BUILD-3 | A.36, A.141 |
| P2-DOCS-1 | A.55, A.56, A.57, A.58, A.59, A.143 |
| P2-CH-SCHEMA | A.38, A.39, A.40 |
| P3-SEQ-1 | A.74, A.109, A.110, A.111 |
| P3-TIME-1 | A.76, A.119, A.147 |
| P3-HA-1 | A.78, A.79 |
| P3-MON-1 | A.77 |
| P3-CHG-1 | A.51, A.80 |
| P3-EXEC-2 | A.66, A.144 |

*Note: a few findings (A.33, A.36, A.49, A.51, A.66, A.113, A.141, A.144) appear under two tasks because the root-cause fix and its test/hardening live in different phases — close the finding when both are done. A.49's client-side reconnect reset is in P2-FEED-1; its server-side WS backpressure is in P2-API-2.*

**Refuted (do not action):** 9 claims were raised and rejected by verification — see Appendix B of the audit report.
