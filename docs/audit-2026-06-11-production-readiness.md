# Poly-Book Production-Readiness Audit — Synthesis Report

## 1. Executive Summary

Poly-book is a well-architected hobby-to-prosumer system that is materially closer to production than most, but it is **not at the Jane Street / Citadel bar, and in its currently-deployed form it is not capturing data durably at all.** The design instincts are genuinely good — single-writer book updates, bounded channels with backpressure, fixed-point-only state, dual-timestamp provenance, a CRC-framed WAL, checkpoint+WAL hydration, and dual-mode replay. The failures are almost entirely in *wiring, failure-mode behavior, and operational hardening*, not in core data structures. That is the optimistic read. The pessimistic read is that several headline guarantees the docs advertise — zero data loss, deterministic replay, integrity validation, multi-replica WAL coordination, S3 storage — are either unimplemented, unwired, or provably broken, and CI is green anyway because the tests that would catch them are excluded.

Five themes dominate everything else:

1. **Durability is aspirational, not real.** The WAL is never `fsync`ed and never flushed in steady state (`writer.rs:150`, `ingest.rs:133`), the writer doesn't repair a torn tail on restart (`segment.rs:51`), pruning is implemented but never invoked, append failures are warn-and-continue, and the production rotating mode (`auto-ingest`) bypasses the WAL entirely (`auto_ingest.rs:66`). Forensic analysis of the captured WAL even found a *real* head-overwrite corruption event. The system's central promise has the least working machinery behind it.

2. **The pipeline fails closed in the worst way.** A single transient sink error tears down all ingestion and exits `0` (`clickhouse_sink.rs:66`), which supervisors will not restart. There is no task supervision anywhere, so panics are swallowed and a dead projector keeps advancing WAL consumer positions for records that were never applied (`live_state.rs:642`).

3. **The deployed system is fictional.** The Docker image cannot build (four independent reasons), the ECS task crash-loops on a missing `--tokens`, the `s3://` storage path silently becomes a local directory on ephemeral container storage (`ecs.tf:36`, `pipeline.rs`), and the deploy workflow has failed at startup on every push to `main` for a month with no alerting. ClickHouse — half the storage story — is non-functional against a stock server (`writer.rs:38`, `:232`) and untested in CI.

4. **Read-path integrity is partly theater.** Replay validation seeds reconstruction from the very checkpoint it validates against, so `matched` is always true (`engine.rs:105`) — confirmed live against captured data. Replay ordering is nondeterministic and mis-orders pre-snapshot deltas because sequences reset on every snapshot (`engine.rs:287`). Crossed/locked-book detection (`check_integrity`) exists but is dead code on every production path (`book.rs:175`) — and captured data contains real locked-book episodes nobody flagged.

5. **No production trust boundary and no operational eyes.** Every surface binds `0.0.0.0` with no auth; the SQL workbench is an unauthenticated SSRF / arbitrary-file-read primitive via ClickHouse table functions (`query.rs:181`); metrics are exposed to the internet on a public-IP Fargate task (`vpc.tf:53`). Observability is counters/histograms only — zero gauges, no feed-staleness or WAL-lag metric, no alert rules, no dashboards, no runbook — so an on-call rotation would have nothing to page on.

Net assessment: roughly **6–9 months of focused work** across correctness, durability, and operations separates this from the stated bar. The architecture does not need to be rebuilt; the guarantees need to be made real and continuously verified.

---

## 2. Scorecard

| Area | Rating | One-line justification |
|---|---|---|
| Durability / WAL | **Weak** | No fsync/flush/prune wired; torn-tail unrecovered (`segment.rs:51`); append failures swallowed; captured WAL shows a real corruption event. |
| Storage | **Weak** | ClickHouse non-functional against stock server; one sink error halts all ingest; 5-min in-memory loss window; `s3://` path writes to local ephemeral disk. |
| Feed ingest | **Adequate** | Strong structure/tests, but backoff never resets, no liveness watchdog, `<=` drops same-ms snapshots, partial snapshots on parse failure, no venue continuity check. |
| Book / replay correctness | **Weak** | Core L2 semantics sound, but replay validation is vacuous, replay ordering nondeterministic, and crossed-book detection is dead code — the integrity story is mostly unenforced. |
| API surface | **Adequate** | Excellent watch-based read model, but unbounded historical/WS resource use, no auth, leaks internal errors, `/health` returns 200 when not ready. |
| Security | **Weak** | Unauthenticated SQL SSRF/file-read primitive; all binds `0.0.0.0`; metrics open to internet; root Docker; mutable action tags; over-broad IAM. |
| Concurrency | **Adequate** | Bounded channels and single-writer design are genuinely good; failure modes (exit-0 cascade, blocking I/O on runtime, swallowed panics) are not. |
| Numerics | **Adequate** | Fixed-point end-to-end is real and well-tested; gaps are the f64 string-parse path, segment-size-dependent WAL offset math, and `overflow-checks` off in release. |
| Testing | **Adequate** | Strong unit/proptest/fuzz/miri foundation, but integration suite excluded from CI, highest-stakes crash paths untested, and crossed-book properties vacuous. |
| Observability / Ops | **Weak** | Zero gauges; no WAL-lag/feed-staleness metrics; histograms are 60s summaries; no alerts/dashboards/runbooks; deploy broken; no data retention. |
| Frontend | **Adequate** | Prior audit items fixed and patterns are good, but a WS lifecycle race leaks sockets, no staleness detection, and several real correctness bugs remain. |
| Architecture | **Strong (design) / Adequate (realized)** | Single-writer, channel-based, provenance-rich, WAL-centric design is sound; the deductions are unwired capabilities and undeployed topology. |

---

## 3. Top 10 Priorities (in order)

1. **Make the deployed system actually persist data.** Parse the storage URL scheme and construct a real `object_store::aws::AmazonS3` backend instead of silently treating `s3://...` as a local dir; mount durable storage (EFS) for the WAL; fix the ECS command to pass `--tokens` (or use `auto-ingest`); fix the Docker build and deploy workflow. *CRITICAL.* `infra/ecs.tf:36`, `crates/pb-bin/src/commands/pipeline.rs:55`, `Dockerfile:11`, `.github/workflows/deploy.yml`.

2. **Wire WAL durability.** Add a steady-state flush cadence (tens of ms) for tail visibility, a configurable `fdatasync` policy, directory fsync on rotation, and torn/zeroed-tail scan-and-truncate on reopen; make append/open failure fatal (or counter + readiness flag), not warn-and-continue. `crates/pb-wal/src/writer.rs:150`, `crates/pb-wal/src/segment.rs:51`, `crates/pb-bin/src/commands/ingest.rs:133`.

3. **Stop the single-error total shutdown.** Retry sink flushes with bounded backoff while retaining the buffer; isolate sinks so one failing sink cannot kill the other or the WAL; return non-zero on terminal failure. `crates/pb-store/src/clickhouse_sink.rs:66`, `crates/pb-store/src/parquet_sink.rs:55`, `crates/pb-bin/src/commands/ingest.rs:145`.

4. **Make `auto-ingest` (the real production mode) write the WAL**, and overlap subscriptions during rotation so the final ~10 s of each 5-minute market is captured. `crates/pb-bin/src/commands/auto_ingest.rs:66`, `:134`.

5. **Fix ClickHouse persistence and put it under CI.** Remove `Nullable` columns from every `ORDER BY`, encode `Enum8` as integer discriminants (or `LowCardinality(String)`), fix the matching reader structs, and run the currently `#[ignore]`d testcontainers roundtrip in a Docker CI job. `crates/pb-store/src/writer.rs:38`, `:232`; `crates/pb-replay/src/reader.rs:1038`; `.github/workflows/ci.yml:33`.

6. **Make replay validation non-vacuous.** Reconstruct from checkpoints strictly *older* than the reference, fix `MockReader` so its checkpoint logic is reachable in production, and add a test asserting a divergent stream yields `matched=false`. `crates/pb-replay/src/engine.rs:105`, `:113`.

7. **Make replay deterministic and wire-faithful.** Persist a non-resetting monotonic per-asset ingest ordinal (or WAL offset) on every `BookEvent` and sort replay by it; replace `buffer_unordered` with ordered merge over path-sorted files. `crates/pb-replay/src/engine.rs:287`.

8. **Close the SQL workbench RCE-class hole.** Run it as a ClickHouse `readonly=2` user with table-function/`system.*` access revoked; in the guard, allowlist datasets and reject table functions and `SETTINGS`; clamp `max_rows` server-side and enforce a top-level `LIMIT`; extend the timeout to cover body download. Require auth before exposing it anywhere networked. `crates/pb-service/src/query.rs:181`, `:243`, `:327`; `crates/pb-api/src/server.rs:447`.

9. **Supervise tasks; never silently degrade.** Watch every `JoinHandle` (a `JoinSet`/supervisor), treat unexpected exit/panic as fatal-or-restart with alerting, and make `LiveReadModel::apply_record` fail when the projector channel is closed so the WAL tailer stops committing positions for unapplied records. `crates/pb-api/src/live_state.rs:642`, `crates/pb-bin/src/commands/ingest.rs:49`, `crates/pb-bin/src/commands/serve.rs:189`.

10. **Populate `checkpoint.wal_offset`** from `WalWriter::global_offset()` at append time and pass the full `WalConfig` into hydration (don't hardcode default segment size), so cold-start replay is bounded by checkpoint cadence rather than total retention. `crates/pb-replay/src/backfill.rs:141`, `crates/pb-api/src/hydration.rs:128`, `:153`.

---

## 4. Thematic Findings

### A. Durability & the WAL (the backbone)

The WAL is the system's claimed source of truth, yet almost none of its durability machinery is connected.

- **No steady-state fsync or flush.** `Segment::sync()` (fdatasync) has zero production callers; the only `flush()` is at graceful shutdown. Up to 64 KiB of acked records sit in the `BufWriter` on crash, everything in page cache is lost on power loss, and the serve tailer can't see records until 64 KiB accumulates — so in a quiet market the "live" read model lags arbitrarily. `crates/pb-wal/src/writer.rs:150`, `crates/pb-bin/src/commands/ingest.rs:133`.
- **No torn-tail recovery.** `Segment::open_append` resumes at raw file length without validating a frame boundary; after a crash mid-frame (or ext4/XFS zero-fill), the writer appends after garbage and the reader desyncs framing, silently dropping every post-restart record. `crates/pb-wal/src/segment.rs:51`.
- **Frame length is outside the CRC.** A single flipped length byte is undetectable and causes either a treated-as-EOF silent drop of the segment remainder or a misaligned skip; all corruption handling is a `warn!` with no metric and no resync signal. `crates/pb-wal/src/segment.rs:153`, `crates/pb-wal/src/reader.rs:105`.
- **Pruning never runs.** `prune`/`prune_with_backpressure` have no callers; `wal.max_segments` is dead config; the WAL grows unbounded until disk-full, at which point appends silently fail. `crates/pb-wal/src/writer.rs:75`, `crates/pb-bin/src/commands/pipeline.rs:156`.
- **Tail polling is quadratic.** When caught up, the reader re-`std::fs::read`s the entire active segment (up to 64 MB) and re-lists the directory every 50 ms poll — on the async runtime, blocking it. `crates/pb-wal/src/reader.rs:253`, `:274`; this should be incremental `pread` from the last offset via `spawn_blocking`.
- **Permanent reader stall.** If `current_data` is `None` (empty-dir startup before ingest, or a prune race), `next()` returns `Ok(None)` forever while `/health` still reports ready. `crates/pb-wal/src/reader.rs:136`.
- **No writer mutual exclusion.** No `flock`; two ingest processes on the same directory interleave appends and `Segment::create(truncate=true)` can wipe a populated segment on id collision. `crates/pb-wal/src/segment.rs:33`.
- **Forensic confirmation.** The captured `segment_00000000000000000000.wal` physically contains a later session's frames overwriting the file head — direct evidence the capture-era `open_append` (no seek-to-end) corrupted history in place. Fixed at HEAD, but it underscores that this class of bug already happened. `crates/pb-wal/src/segment.rs:58`.

### B. Storage sinks & ClickHouse

- **ClickHouse is non-functional against a stock server,** for two independent, empirically-reproduced reasons: `Nullable` columns in `ORDER BY` (rejected as `ILLEGAL_COLUMN`) and `Enum8` columns serialized as Rust `String` over RowBinary (`CANNOT_READ_ALL_DATA`). `ensure_tables()` failure is only a `warn!`, then the sink dies on first flush → silent total CH data loss while Parquet runs. `crates/pb-store/src/writer.rs:38`, `:232`.
- **Fail-stop with no retry cascades to total halt.** Covered in priorities; the buffered batch is dropped and the "will retry on insert" log message describes behavior that does not exist. `crates/pb-store/src/clickhouse_sink.rs:66`, `crates/pb-store/src/pipeline.rs:96`.
- **5-minute in-memory loss window with no reconciliation.** Parquet buffers 300 s in a plain `Vec`; on crash that window is gone from the storage datasets, and no command replays WAL→storage. `crates/pb-store/src/parquet_sink.rs:13`. Buffering is also unbounded between flushes and flush runs inline, coupling WAL latency to S3 I/O. `:46`.
- **`backfill` is silently broken.** It passes the raw relative `./data` base path (uncanonicalized), so `object_store` percent-encodes it to `/%2E/data/...` and every flush fails while the command prints "backfill complete." `crates/pb-bin/src/commands/backfill.rs:48`.
- **Silent overwrite on deterministic names.** `{asset}_{first_ts}.parquet` with `PutMode::Overwrite` lets quiet-book checkpoints (timestamp doesn't advance) and execution-append runs erase prior files; out-of-range timestamps default to a 1970 partition that time-windowed reads never find. `crates/pb-store/src/writer.rs:181`, `:159`.
- **Non-atomic multi-table CH flush with no idempotency** means any future retry duplicates rows on plain `MergeTree`. `crates/pb-store/src/writer.rs:534`.
- **Schema/lifecycle gaps:** `asset_id`/`source`/`mode`/`status` stored as plain `String` instead of `LowCardinality`/`Enum`; daily partitions with no TTL; documented batch-tuning keys (`clickhouse_batch_*`, `parquet_row_group_size`) are dead config. `crates/pb-store/src/writer.rs:25`, `:37`; `crates/pb-store/src/clickhouse_sink.rs:12`.

### C. Feed ingest correctness & continuity

- **`<=` stale-snapshot guard drops same-millisecond snapshots.** Two trades in one ms produce two `book` events with equal timestamps; the newer is dropped (confirmed 7× in captured V2 data). Use `<` and dedupe true retransmits via the venue `hash`. `crates/pb-feed/src/dispatcher.rs:172`.
- **No venue continuity validation.** Sequences are locally synthesized (gap-free by construction), so `SequenceGap`/`record_gap_detected()` are dead at ingest; the venue `hash` and `best_bid/ask` fields are parsed but never validated. Silent WS loss is undetectable. `crates/pb-feed/src/dispatcher.rs:362`.
- **Partial snapshot on mid-message parse failure.** Per-level `?` in the Book/PriceChange arms emits a truncated snapshot (e.g. bids only) that looks complete, and `last_snapshot_ts` was already advanced so retransmits are rejected. Convert all levels into a `Vec` first, then emit atomically. `crates/pb-feed/src/dispatcher.rs:217`, `:256`.
- **Reconnect backoff never resets.** `attempt` only increments for process lifetime, degrading to a fixed 30 s gap per disconnect after ~9 cumulative disconnects; jitter is also nullified at the cap. `crates/pb-feed/src/ws.rs:88`, `:224`.
- **No liveness watchdog.** Pongs are ignored and there's no read-idle timeout, so a half-open TCP connection stalls the feed silently for ~15+ minutes while the process looks healthy. `crates/pb-feed/src/ws.rs:186`.
- **Unparsed/unknown messages dropped at debug with no metric** (e.g. array-framed batches, future V2 event types) — a venue format drift would zero ingestion silently. `crates/pb-feed/src/dispatcher.rs:156`.
- **No HTTP timeouts on `RestClient`** — a hung discovery/backfill request stalls `auto-ingest` market rotation indefinitely. `crates/pb-feed/src/rest.rs:32`.

### D. Book & replay correctness / integrity

The core `L2Book` is sound (last-wins deltas, zero-size removal, O(1) totals cross-checked by proptests). The defects are at the validation boundary and in replay.

- **Vacuous replay validation** (priority 6): `reconstruct_at` seeds from the reference checkpoint via an inclusive bound, so the comparison is checkpoint-vs-itself. Confirmed live. `crates/pb-replay/src/engine.rs:105`, `:113`.
- **Replay ordering** (priority 7): per-asset sequence resets to 0 on every snapshot, so a same-microsecond pre-snapshot delta sorts *after* the snapshot; `buffer_unordered(8)` makes ties nondeterministic across runs. 316 such tie rows exist in captured data. `crates/pb-replay/src/engine.rs:287`.
- **Crossed/locked-book detection is dead code.** `check_integrity` has zero production callers; captured data contains 5 locked-book episodes in 137 s that it would have flagged. Run it at message boundaries on the live and replay paths and emit a metric + persisted integrity event. `crates/pb-book/src/book.rs:175`.
- **`check_sequence` zero-sentinel disables gap detection** exactly post-snapshot and post-checkpoint (where `sequence == 0` is legitimate), under-reporting real loss. Replace the sentinel with `Option<Sequence>` and persist the book sequence in `BookCheckpoint`. `crates/pb-book/src/book.rs:160`.
- **Checkpoint clock-domain mismatch.** REST checkpoints store exchange-ms timestamps but are used as the floor of a recv-clock query and as the recv-clock skip threshold; NTP skew silently skips or re-applies deltas. `crates/pb-replay/src/engine.rs:181`.
- **Snapshot grouping is duplicated from the live path with weaker (timestamp-equality) semantics**, merging distinct snapshots under ms collisions; and `read_latest_checkpoint` does an unbounded epoch-scan (~1M FS ops) for a checkpoint-less asset, a cheap DoS via the public replay route. `crates/pb-replay/src/reader.rs:962`.
- **Legacy capture is invisible:** no Parquet schema-version metadata, so the pre-split 2026-03-06 capture (1.69M rows, old layout/schema) is silently unreadable. `crates/pb-replay/src/reader.rs:69`.

### E. Security & exposure

- **Unauthenticated SSRF / arbitrary-file-read** via the SQL workbench (priority 8): the guard is a keyword blocklist with no table/function allowlist, so `SELECT * FROM file('/etc/passwd',…)` / `url('http://169.254.169.254/…')` / `s3(…)` / `system.users` all pass, with no `readonly` mode and no auth on a `0.0.0.0` bind. `crates/pb-service/src/query.rs:181`.
- **No auth on any surface and all bind `0.0.0.0`** (API 3000, gRPC 50051, metrics 9090); the trust boundary is not documented in `api.md`/`operations.md`. Default to loopback. `config/default.toml:35`, `crates/pb-api/src/server.rs:112`.
- **Metrics exposed to the public internet** on a public-IP Fargate task (`0.0.0.0/0` → 9090). `infra/vpc.tf:53`.
- **gRPC bypasses all input guards** — a far-future `end_us` drives `hour_paths` into ~billions of iterations (remote OOM); `limit` is unclamped. Move validation into `pb-service`. `crates/pb-grpc/src/lib.rs:107`.
- **Internal errors leaked verbatim** (ClickHouse URLs, storage errors) to unauthenticated clients, and 500s are never logged. `crates/pb-api/src/error.rs:39`.
- **Supply chain & infra:** actions pinned to mutable tags/branches not SHAs (`deploy.yml:29`); root Docker with tag-only base images (`Dockerfile:20`); over-broad ECS IAM on `Resource="*"` (`iam.tf:116`); S3 `force_destroy=true`, no versioning, SSE-S3 not KMS (`s3.tf:1`).

### F. Concurrency & failure modes

The bones are good — all channels bounded, single-writer projector, cancel-safe `select!` loops, correct broadcast-lag resync. The failures are systemic.

- **No task supervision** (priority 9): dropped/await-only `JoinHandle`s swallow panics; a dead projector still has the WAL tailer committing consumer positions for unapplied records. `crates/pb-api/src/live_state.rs:642`, `crates/pb-bin/src/commands/ingest.rs:49`.
- **Blocking WAL I/O on the async runtime** (fsync, full-segment re-read, per-record lag stat) stalls the same runtime serving HTTP/WS. `crates/pb-wal/src/reader.rs:274`, `crates/pb-bin/src/commands/serve.rs:196`.
- **Shutdown drops queued events.** `ingest` breaks immediately on cancel, discarding up to 2048 buffered events (and 2048 raw frames in the dispatcher) before WAL write; `auto-ingest` gets this right and should be the shared template. `crates/pb-bin/src/commands/ingest.rs:120`.
- **No tailer recovery.** `needs_resync` and reader-open failure both terminate the tailer with no re-hydration; `/health` can still report ready while serving frozen data. `crates/pb-bin/src/commands/serve.rs:189`.
- **Rotation reaping is awkward** (serve-api joins old children inline pre-subscribe → market-start gap; auto-ingest never joins them → orphaned tasks). `crates/pb-bin/src/commands/serve_api.rs:274`.

### G. Numerics & fixed-point

Fixed-point discipline is real end-to-end; the gaps are concentrated and tractable.

- **All string→fixed-point parsing routes through f64,** which (a) breaks serde roundtrip above 2^53 raw — and the WAL bincode codec and checkpoint JSON use exactly this serde, a direct zero-data-loss violation; (b) silently saturates oversized sizes to `u64::MAX`; (c) silently rounds sub-tick digits. Replace with integer decimal parsing. `crates/pb-types/src/fixed.rs:210`, `:170`, `:71`.
- **`FixedPrice` range invariant is bypassable** via the public tuple field / `new_unchecked`; out-of-range values serialize but fail to deserialize — write-OK/read-FAIL poison for persisted records. Make the field private. `crates/pb-types/src/fixed.rs:44`.
- **`overflow-checks` off in release** while `L2Book` running totals use unchecked `u64` arithmetic reachable from feed-controlled saturated sizes — wraps silently in production, panics in debug. Enable `overflow-checks = true` (and in `[profile.bench]`). `Cargo.toml:104`, `crates/pb-book/src/book.rs:97`.
- **WAL resume-offset arithmetic is segment-size-dependent** but hydration hardcodes the default `WalConfig`, so any non-default `segment_size_mb` would mis-skip records once `wal_offset` is populated. `crates/pb-api/src/hydration.rs:128`.
- **Divergent ms/µs timestamp heuristics** misclassify seconds/nanosecond inputs and map `"0"` to a 1970 partition; the two copies disagree on the zero case. Centralize one validated converter. `crates/pb-feed/src/dispatcher.rs:423`, `crates/pb-replay/src/backfill.rs`.

### H. API & gRPC surface hardening

- **SQL row cap is client-controlled and bypassable** with the response buffered unbounded in memory (priority 8). `crates/pb-api/src/server.rs:447`, `crates/pb-service/src/query.rs:243`.
- **Historical routes have no timeout, concurrency cap, or response-size cap** and buffer a full 24 h window in RAM; `continuity_events` is unbounded. Add `TimeoutLayer` + `ConcurrencyLimitLayer`, cap arrays. `crates/pb-api/src/server.rs:290`, `crates/pb-service/src/lib.rs:124`.
- **WS fan-out is unbounded** — no connection cap, no heartbeat, default ~64 MB message limit; half-open peers on quiet assets leak sessions. `crates/pb-api/src/streaming.rs:96`.
- **`/health` returns 200 when not ready,** breaking status-code-based probes; split `/health/live` and `/health/ready`. `crates/pb-api/src/server.rs:157`.
- **gRPC server reports "bound" before binding and swallows bind failures,** so `serve` silently runs without gRPC; it also has no timeout/concurrency/encoding-size limits and binds `0.0.0.0`. `crates/pb-grpc/src/lib.rs:258`, `:255`.

### I. Observability & operations

- **No gauges anywhere** (`grep gauge!` → nothing): no feed-staleness, no WAL consumer-lag (it's an `AtomicU64` surfaced only via `/health` JSON), no WAL disk/segment count, no channel depth, no `wal_append_failures`/`sink_flush_failures` counters, no end-to-end recv→durable latency. `crates/pb-metrics/src/recorder.rs:4`.
- **Latency "histograms" are 60 s rolling summaries** (no buckets configured), so quantiles can't be aggregated across the ingest/serve processes and the 5-minute Parquet flush sample expires before scrape. `crates/pb-metrics/src/server.rs:9`.
- **No `run_upkeep` task** for the Prometheus recorder — histogram buckets grow unbounded if scraping stalls, in the always-on ingest process. `crates/pb-bin/src/commands/pipeline.rs:20`.
- **No alert rules, dashboards, or runbooks** anywhere in the repo; on-call has nothing to page on.
- **Config is fail-silent throughout:** missing `--config` ignored, parse/type errors swallowed via `unwrap_or`, negative ints wrap via `as`, several documented keys dead, and the boolean `--parquet`/`--metrics` toggles literally cannot be disabled (so the documented two-process recipe collides on port 9090). `logging.format` is dead so JSON logs are impossible. `crates/pb-bin/src/main.rs:146`, `:49`, `:166`.
- **Infra contradicts the docs:** a single `FARGATE_SPOT` task (routine reclaim = capture gap), no ClickHouse provisioned, no `serve` service, no EFS, no health check, no circuit breaker — the multi-replica WAL story has zero infra backing. `infra/ecs.tf:13`.
- **No data retention:** no ClickHouse TTL, no Parquet/WAL expiry, and `force_destroy=true` on the data bucket is a one-command data-annihilation path. `infra/s3.tf:1`.
- **CI gaps:** no docker-build gate, no coverage gate, no Criterion perf-regression gate, 30 s smoke-only fuzz, and the query-guard fuzz target was removed from CI with a comment admitting unfixed bypasses. `.github/workflows/ci.yml:129`.

### J. Testing rigor

Strong foundation (WAL framing/corruption tests, broad proptests, six fuzz targets, correctly-scoped miri, meaningful Criterion), but the highest-stakes failure modes are unexecuted.

- **The entire integration package is excluded from CI** and ClickHouse tests are additionally `#[ignore]`d, so hydration/replay/roundtrip/determinism regressions pass green. `.github/workflows/ci.yml:33`.
- **Writer-crash-mid-append recovery is untested** and code inspection shows it loses records. `crates/pb-wal/src/segment.rs:51`.
- **Crossed-book properties are vacuous** — bid/ask strategies use disjoint price ranges and `fuzz_book_delta` never calls `check_integrity`. `crates/pb-book/src/book.rs:1148`, `fuzz/fuzz_targets/fuzz_book_delta.rs`.
- **No WS reconnect-with-gap, kill-and-restart durability, or sink-failure-injection tests.** `crates/pb-feed/src/ws.rs:275`, `crates/pb-store/src/tests.rs:302`.
- **Fuzzing stops at serde** — dispatcher normalization, `codec::decode`, and config parsing are unfuzzed, and the corpus isn't cached across CI runs. `fuzz/fuzz_targets/fuzz_ws_deser.rs:5`.
- **No golden-bytes codec fixture** — bincode's positional encoding means a field reorder silently changes the v1 format while all roundtrip tests stay green. `crates/pb-wal/src/codec.rs:286`.

### K. Frontend

All five March 2026 audit items are genuinely fixed; the remaining defects are real but contained.

- **WebSocket lifecycle race** (the worst): a shared `unmountedRef` reset across effect runs lets the old socket's `onclose` clobber the new socket, leak the open connection, and spawn a ghost reconnect loop on every asset switch and under StrictMode. Use a per-run `cancelled` local + instance identity check. `web/src/shared/hooks/use-orderbook-stream.ts:33`.
- **No WS staleness/heartbeat detection and no recovery from permanent fallback** — a ~50 s blip downgrades the session to HTTP polling forever, and frozen WS data is preferred over fresh HTTP under a green "live" badge. `:116`.
- **WS→TanStack-Query bridge writes the wrong key** (`bids.length` used as depth), so the unified-data update is almost always a no-op. `:78`.
- **Dev proxy lacks `ws:true`** — WS streaming is dead in the default dev/e2e workflow, which is why these bugs stay hidden. `web/vite.config.ts:28`.
- **Source mode missing from query keys** (demo/live cache pollution) and **route ErrorBoundary never resets on navigation** (one error bricks the app). `web/src/shared/api/queries.ts:43`, `web/src/app/error-boundary.tsx:29`.
- **4 s hard timeout aborts legitimate SQL workbench queries.** `web/src/shared/api/client.ts:5`.
- **A11y:** command palette lacks dialog semantics/focus trap; sort headers are mouse-only; lazy-route heading focus races. `web/src/shared/components/command-palette.tsx:35`.

### L. Execution subsystem

The only operator-driven write path in a nominally read-only system, and it is unguarded.

- **No idempotency** — operator retries double-count fills in ClickHouse `MergeTree`; Parquet retries overwrite. Neither sink is retry-safe. Add a content-derived `event_id` + `ReplacingMergeTree`/dedup token. `crates/pb-bin/src/commands/execution_append.rs:209`.
- **No timestamp unit/range validation** — a ms/s-resolution `event_timestamp_us` is filed into a 1970 partition, invisible to all correct queries, while the command reports success. `:179`.
- **No `LatencyTrace` monotonicity checks** (web waterfall renders negative durations); **no timeline tie-break** (nondeterministic, backend-divergent results); **no pagination** (oldest-first truncation hides recent events). `crates/pb-types/src/event.rs:208`, `crates/pb-replay/src/reader.rs:1020`, `crates/pb-service/src/lib.rs:163`.
- **gRPC execution RPC bypasses the HTTP guards** and buffers the whole window. `crates/pb-grpc/src/lib.rs:187`.
- **Zero CLI tests** and the write→read integration tests are excluded from CI. `crates/pb-bin/src/commands/execution_append.rs:1`.

### M. Build, dependency & toolchain hygiene

- **`target-cpu=native` checked in** for all Linux/macOS builds — non-reproducible binaries, SIGILL portability risk, benchmarks tuned to the build host, and it's overridden by CI `RUSTFLAGS` and never seen by Docker, so it only applies on dev laptops. `.cargo/config.toml:1`.
- **Release profile is never compiled or tested.** `panic=abort` + `strip=symbols` + no debug info changes failure semantics from per-task isolation to whole-process abort, makes crashes untriageable, and is undocumented (no ADR) and absent from CI. `Cargo.toml:104`.
- **WAL on-disk format rests on frozen bincode 1.3.3 positional encoding** with only self-consistent roundtrip tests; `deny.toml unmaintained="none"` hides the maintenance status. Commit golden byte fixtures. `crates/pb-wal/src/codec.rs:17`.
- **Four-way toolchain skew:** pin 1.94.0, CI `@stable`, Docker 1.93 (below MSRV), fuzz/miri `@nightly`, with no MSRV verification job. `rust-toolchain.toml:2`.
- **Two HTTP stacks + two TLS implementations** in the production binary, with the latency-critical WS feed alone on OpenSSL (also the missing-`libssl3` Docker runtime failure). Move tokio-tungstenite to rustls. `Cargo.toml:43`.

### N. Docs / spec drift

Route docs are accurate; operational claims are not. WAL pruning is documented as a live `WalPruner` that doesn't exist (`docs/operations.md:345`, `CLAUDE.md:37`); four config keys are documented but dead (`docs/operations.md:30`); the Docker/ECS deploy flow can't build (`docs/operations.md:176`); four docs still describe a 4-surface SPA when 6 ship including `/query` (`docs/operations.md:426`); the 24 h window cap that 400s requests is undocumented (`docs/api.md:166`); `serve-api.md` names the wrong health route; the architecture diagram shows a `ParquetSink` in `serve-api` that doesn't exist (`docs/architecture.md:168`); and archived OpenSpec tasks are checked for `WalPruner` and a benchmark gate that never shipped (`openspec/changes/archive/clean-slate-serving-architecture/tasks.md:16`). The `just replay` recipe always fails (missing `--mode`) and the DuckDB helpers target the pre-split schema (`justfile:49`).

---

## 5. Phased Roadmap

### Phase 1 — Correctness & Durability (make the guarantees real)
*Goal: zero silent data loss and faithful, deterministic replay before anything else.*

- WAL: steady-state flush + configurable fsync + directory fsync on rotation + torn/zeroed-tail recovery; CRC over the frame header; fatal-on-append-failure. (`writer.rs:150`, `segment.rs:51`, `:153`, `ingest.rs:133`)
- Sink resilience: bounded-retry flush with buffer retention; isolate sinks; non-zero exit on terminal failure. (`clickhouse_sink.rs:66`, `parquet_sink.rs:55`)
- ClickHouse DDL/Enum fix + un-ignore roundtrip in CI. (`writer.rs:38`, `:232`)
- `auto-ingest` writes the WAL; rotation overlap for endgame capture. (`auto_ingest.rs:66`, `:134`)
- Replay validation non-vacuous; replay sorted by a persisted monotonic ingest ordinal; ordered file merge. (`engine.rs:105`, `:287`)
- Populate `checkpoint.wal_offset`; pass full `WalConfig` to hydration. (`backfill.rs:141`, `hydration.rs:128`)
- Crossed-book detection on live+replay paths; `check_sequence` `Option` state; persist book sequence in checkpoints. (`book.rs:175`, `:160`)
- Stale-snapshot `<` + hash dedupe; atomic snapshot emission. (`dispatcher.rs:172`, `:217`)
- Integer decimal fixed-point parsing; private `FixedPrice` field; `overflow-checks=true` in release. (`fixed.rs:210`, `:44`, `Cargo.toml:104`)
- Wire a real S3 `object_store` backend; mount EFS for the WAL; fix Docker build and ECS `--tokens`. (`pipeline.rs:55`, `ecs.tf:36`, `Dockerfile:11`)
- Add integration tests to CI; crash-recovery + WS-reconnect-with-gap tests; golden codec fixtures. (`ci.yml:33`, `codec.rs:286`)

### Phase 2 — Operational Hardening (make it safe to run unattended)
*Goal: nothing degrades silently; abuse and exposure are bounded; on-call can see and act.*

- Task supervision across all spawned tasks; projector-death surfaces in `/health` and stops position commits. (`live_state.rs:642`)
- Tailer recovery loop (re-hydrate on resync, retry reader open). (`serve.rs:189`)
- SQL workbench: readonly CH user + dataset/table-function allowlist + server-side row/byte/time caps + auth; full-request timeout. (`query.rs:181`, `:327`, `server.rs:447`)
- Default all binds to loopback; document trust boundary; restrict metrics ingress to a scraper CIDR; move tasks to private subnets. (`config/default.toml:35`, `vpc.tf:53`)
- API/gRPC: request timeout + concurrency cap + response-size cap; validation moved into `pb-service`; WS connection cap + heartbeat + message-size limit; sanitize/log internal errors; split health endpoints. (`server.rs:290`, `streaming.rs:96`, `grpc/lib.rs:107`, `error.rs:39`, `server.rs:157`)
- Observability: feed-staleness / WAL-lag / disk / channel-depth gauges; append/flush failure counters; recv→durable latency histogram with explicit buckets; `run_upkeep`; commit `monitoring/` alert rules + dashboards + `RUNBOOK.md`. (`recorder.rs:4`, `server.rs:9`, `pipeline.rs:20`)
- Config: typed deserialize with fail-fast on parse/range errors; required `--config` when explicit; fixable boolean toggles; wire or delete dead keys; JSON logging. (`main.rs:146`, `:49`)
- Wire WAL pruning + enforce `max_segments`. (`writer.rs:75`)
- Infra: on-demand (or dual-AZ) ingest, EFS, ClickHouse, circuit breaker, health check; S3 versioning + `force_destroy=false`; TTL/retention; SHA-pinned actions; non-root Docker. (`ecs.tf:13`, `s3.tf:1`, `deploy.yml:29`)
- CI: docker-build gate, coverage ratchet, Criterion regression gate, re-enable query-guard fuzz with corpus cache. (`ci.yml:129`)
- Frontend: fix WS lifecycle race + staleness/heartbeat + cache-key + dev proxy `ws:true` + per-call timeouts + error-boundary reset + a11y. (`use-orderbook-stream.ts:33`)
- Execution: idempotent appends + timestamp validation + timeline tie-break + pagination + gRPC guards. (`execution_append.rs:209`, `:179`, `reader.rs:1020`)
- Toolchain/build: remove `target-cpu=native`, pin a fleet microarch floor; align toolchains + MSRV job; document release profile in an ADR with line-table debuginfo; unify TLS on rustls. (`.cargo/config.toml:1`, `rust-toolchain.toml:2`, `Cargo.toml:43`)
- Doc/spec reconciliation pass. (`docs/operations.md`, `docs/architecture.md:168`)

### Phase 3 — HFT-Grade Capabilities (close the gap to the bar)
*Goal: venue-anchored correctness, failover, and continuous regression discipline.*

- Venue-anchored sequencing: validate the book hash after each delta, emit `SequenceGap`/`BookMismatch` + REST resnapshot on mismatch, and record reconnect windows as queryable data holes. (`dispatcher.rs:362`)
- Time discipline: stamp a global monotonic arrival ordinal at the dispatcher and order replay by it; alarm on negative clock skew; document NTP/PTP requirements and acceptable skew. (`ws.rs:235`, `dispatcher.rs:410`)
- Failover: feed redundancy with arbitration, ingest writer leasing/lock for standby takeover on the shared WAL, in-process re-hydration, and documented RTOs. (`serve.rs:189`)
- Live data-quality monitors that page within seconds: crossed-book, stale feed, WAL lag, sink failure, REST-vs-WS divergence reconciliation. (`live_state.rs:258`)
- Change safety: golden-WAL replay regression in CI (byte-identical book + integrity-event counts), image-digest-pinned staged rollouts, shadow/canary diffing, cross-version codec fixtures + Parquet/ClickHouse schema-version metadata and migration procedure. (`tests/integration/book_determinism.rs:1`, `reader.rs:69`)
- Documented flow-control & load-shedding policy: WAL is the only unconditionally-blocking consumer; sinks may lag with alerting; channel capacities sized from measured rotation bursts with depth gauges. (`ingest.rs:43`)
- Execution lifecycle state-machine validation and exact-decimal fill prices on the operator write path. (`execution_append.rs:103`, `fixed.rs:112`)


---

# Appendix A — All Confirmed Findings (159)

Every finding below survived adversarial verification (critical/high: 3-skeptic panel, 2-of-3 vote; medium: independent skeptic).


## Severity: CRITICAL

### A.1 Deployed infra writes all market data to ephemeral container storage: S3 path is not wired to an S3 object store and ECS task is missing required --tokens
- **Severity:** critical  |  **Area:** hft-gap  |  **Location:** `infra/ecs.tf:36`

The Terraform deployment runs a single Fargate task with command ["--config", "/etc/poly-book/default.toml", "ingest"] and sets PB__STORAGE__PARQUET_BASE_PATH = "s3://<bucket>/orderbook". But pipeline::start_storage_sinks unconditionally constructs object_store::local::LocalFileSystem and treats the path as a local directory (canonicalize-or-create_dir_all), so the "s3://..." string becomes a literal local directory named "s3:" inside the container. No code anywhere constructs an AmazonS3 store despite the workspace enabling object_store's "aws" feature. There is also no EFS/volume mount in ecs.tf, so Parquet data and the WAL live on Fargate ephemeral storage — every task restart or redeploy destroys all captured history. Additionally, ingest::run bails with "--tokens is required" when tokens is None, and the ECS command passes no --tokens, so the task as defined crash-loops. docs/operations.md:211 claims "S3 for Parquet storage", which the code does not implement.

**Recommendation:** Parse the storage URL scheme and construct the matching object_store backend (object_store::parse_url or AmazonS3Builder for s3://), add an integration test that a s3:// base path does not silently create a local directory, mount durable storage (EFS) for the WAL, and fix the ECS command (use auto-ingest or supply tokens). Add a CI check that the Terraform command/env is exercised against the real CLI.


## Severity: HIGH

### A.2 Production Docker build cannot succeed and the Deploy workflow has failed at startup on every push to main
- **Severity:** high  |  **Area:** build-dependency-hygiene  |  **Location:** `Dockerfile:11`

The deploy path to ECS is completely broken, in four independent ways. (1) The builder stage uses rust:1.93-slim while workspace rust-version = "1.94" (Cargo.toml:22), so cargo hard-errors ('requires rustc 1.94 or newer'). (2) The builder copies only Cargo.toml, Cargo.lock and crates/ (Dockerfile:14-15) but the workspace declares member tests/integration (Cargo.toml:15), so cargo fails to load the workspace manifest before compiling anything. (3) No protobuf-compiler is installed in the builder, but pb-grpc/build.rs calls tonic_prost_build::compile_protos (CI needs the dedicated setup-protobuf action for exactly this). (4) Even if it built, the runtime stage (debian:bookworm-slim + ca-certificates only, Dockerfile:20-24) does not provide libssl3, which the binary dynamically links via native-tls (pb-feed + tokio-tungstenite native-tls feature; Cargo.lock shows openssl-sys with no vendored feature), so the container would fail at exec with a loader error. Additionally the web-builder uses node:22-slim (Dockerfile:2) while web/package.json declares engines node >=24.13.1 and .nvmrc pins 24.13.1; there is no .dockerignore (whole repo incl. .git/target enters build context); and cargo build runs without --locked. Crucially, this is not theoretical: `gh run list --workflow=deploy.yml` shows every Deploy run on main completing as `startup_failure` in ~1s (e.g. run 25726996283, 'This run likely failed because of a workflow file issue' — most likely the reusable ci.yml call requesting checks:write at .github/workflows/ci.yml:62 exceeding the caller's permissions block at deploy.yml:11-13). Production deployment has been silently broken for at least a month with no alerting, which also means the Dockerfile defects above have never been exercised.

**Recommendation:** Fix deploy.yml caller permissions (add checks: write or drop it from ci.yml's audit job), bump builder to rust:1.94-slim and copy rust-toolchain.toml, COPY tests/integration (or restructure so the bin builds without test members), apt-get install protobuf-compiler in the builder and libssl3 in the runtime stage (or eliminate native-tls per the TLS finding), align node base image with engines/.nvmrc (node:24), add a .dockerignore, use `cargo build --release --locked`, and add a CI job that actually runs `docker build` on PRs plus alerting on Deploy failures.

### A.3 Nullable columns in ORDER BY make table creation fail on a stock ClickHouse server, causing silent total ClickHouse data loss
- **Severity:** high  |  **Area:** clickhouse  |  **Location:** `crates/pb-store/src/writer.rs:38`

book_events declares `sequence Nullable(UInt64)` (line 31) and uses it in `ORDER BY (asset_id, recv_timestamp_us, sequence, price)` (line 38); ingest_events declares `source_session_id Nullable(String)` (line 72) and uses it in `ORDER BY (recv_timestamp_us, event_kind, source_session_id)` (line 77). A default ClickHouse server rejects Nullable sorting-key columns. I reproduced both against a local ClickHouse: `Code: 44 ... Sorting key contains nullable columns, but merge tree setting allow_nullable_key is disabled`. ensure_tables() therefore errors. In pipeline.rs the ensure failure is only tracing::warn!d, and the first failed flush kills the sink task; every subsequent clickhouse_tx.send() then logs 'clickhouse sink send failed' per event. Net effect: all ClickHouse-bound market data is silently dropped while Parquet continues, defeating the dual-sink durability design. Violates schema-types-avoid-nullable and schema-pk-plan-before-creation.

**Recommendation:** Remove Nullable columns from every ORDER BY. Make sequence/source_session_id NOT NULL with a sentinel DEFAULT (0 / '') per schema-types-avoid-nullable, or drop them from the key (they add little pruning value as deep key suffixes). Treat ensure_tables() failure as fatal at startup instead of a warning, and add a CI gate that runs the currently #[ignore]'d roundtrip against a real server so this cannot regress silently.

### A.4 Enum8 columns are inserted as Rust String, which the clickhouse RowBinary schema validator rejects, aborting every insert batch
- **Severity:** high  |  **Area:** clickhouse  |  **Location:** `crates/pb-store/src/writer.rs:232`

The DDL declares event_kind Enum8('Snapshot'=1,'Delta'=2) and side Enum8('Bid'=1,'Ask'=2) (lines 27-28; also trade/execution side and execution event_kind), but the insert rows model these as Rust String (BookEventRow.event_kind/side, TradeEventRow.side, ExecutionEventRow.event_kind/side) and write human strings like "Snapshot"/"Bid". The pinned clickhouse 0.15 client has validation enabled by default and uses RowBinaryWithNamesAndTypes; its validator maps SerdeType::Str/String only to String/JSON columns (validation.rs:535) and routes Enum columns to err_on_schema_mismatch, producing a SchemaMismatch error that aborts the whole INSERT. With validation disabled it is worse: RowBinary would write a length-prefixed UTF-8 blob where a 1-byte Enum8 value is expected, corrupting the stream. Either way ClickHouse inserts cannot succeed even after the Nullable-key DDL is fixed. Violates schema-types-enum.

**Recommendation:** Represent Enum columns with a serde repr that emits the integer discriminant (i8/i16 field or #[serde(into)] enum), or change the columns to LowCardinality(String) for string semantics. Add an integration assertion that writes one row of every record type and reads it back so the Enum mapping is exercised in CI.

### A.5 Single transient sink flush failure permanently tears down the entire ingest pipeline and exits with code 0
- **Severity:** high  |  **Area:** concurrency  |  **Location:** `crates/pb-store/src/clickhouse_sink.rs:66`

ClickHouseSink::run_with_token propagates any flush error out of run() ('self.flush(&mut buffer).await?'), killing the sink task and dropping the unflushed batch. The fan-out forwarder in ingest.rs then fails its send, breaks, and drops its ftx; pipeline::fanout_event returns false; the main ingest loop breaks (ingest.rs:145-147) and run() returns Ok(()), so the process exits 0. One transient ClickHouse 500 or Parquet/object-store hiccup stops ALL ingestion (WAL, Parquet, and ClickHouse) until an operator restarts, and 'restart=on-failure' supervisors will not restart a 0-exit. ParquetSink has the identical pattern (parquet_sink.rs:55,65,74). There is no retry anywhere in pb-store. Additionally, ParquetSink's in-memory Vec buffer is unbounded between 5-minute flushes.

**Recommendation:** Make sink flush errors non-fatal: retry with bounded exponential backoff inside flush, keep the buffer on failure (bounded by a max-buffer cap with explicit drop accounting), emit a metric/health signal, and never let one sink's death stop WAL writes or the sibling sink. If the pipeline must stop, return an error so the process exits non-zero.

### A.6 Blocking WAL file I/O on the async runtime, including fsync and a quadratic full-segment re-read in the serve live tailer
- **Severity:** high  |  **Area:** concurrency  |  **Location:** `crates/pb-wal/src/reader.rs:274-281`

No spawn_blocking exists anywhere in the workspace. The serve runtime's WAL tailer task (serve.rs:164-246) performs synchronous std::fs I/O on a tokio worker: (1) WalReader::advance_segment re-reads the ENTIRE active segment file ('std::fs::read(&path)') every time the file has grown — at a 50ms poll interval, tailing an active segment approaching the 64MB default means re-reading up to 64MB per poll, i.e. potentially >1GB/s of redundant blocking reads (quadratic in segment size); (2) reader.lag_bytes() stats every available segment file at the top of EVERY loop iteration, i.e. once per applied record (serve.rs:196); (3) commit_reader_position performs fsync + directory fsync inline (reader.rs:151-156). These stall the same runtime serving HTTP/WS requests. The ingest loop similarly performs WalWriter::append/flush (BufWriter write syscalls, file create on rotation) inline (ingest.rs:133-144), which is milder but on the hottest path.

**Recommendation:** Tail incrementally: keep the file handle open, seek to the previous length and read only the delta instead of std::fs::read of the whole segment. Move lag_bytes()/needs_resync() to a timer (e.g. once per second), and run commit_position (fsync) and segment loads via spawn_blocking or a dedicated I/O thread communicating over a channel.

### A.7 Captured WAL segment contains confirmed head-overwrite corruption: a later session's frames physically overwrote the start of the file (open_append never seeked to end)
- **Severity:** high  |  **Area:** data-artifact-forensics  |  **Location:** `crates/pb-wal/src/segment.rs:58`

Frame-by-frame decode of data/wal/segment_00000000000000000000.wal (104,758 frames, 24,975,824 bytes) shows frames 1-2 (offsets 0 and 64) are reconnect_success + source_reset with recv_ts=1773045014543102 (2026-03-09 08:30:14 UTC), while frames 3..104758 carry data from 07:03:29-07:05:47 UTC — 85 minutes EARLIER. Parquet ingest_events prove the 07:03 session wrote its own reconnect pair at recv_ts=1773039809062651 (file ingest_events/2026/03/09/07/global_1773039809062651.parquet), which is absent from the WAL: the 08:30 restart overwrote it in place. Root cause: the code that wrote this data (commit a7148c2, the working tree at capture time) had Segment::open_append set write_offset=file.len() but never seek the fd, so the resumed writer wrote at byte 0 while reporting append offsets near 25 MB. Consequences observed in the data: (1) prior-session WAL history destroyed in place (frame alignment survived only because every session starts with two ingest frames of identical byte lengths, 56 and 142); (2) the new session's records were invisible to the tailing serve reader (consumer pos already at EOF=24,975,824; file length never grew); (3) WAL data from the 06:55 and 06:59 sessions that morning is entirely absent, consistent with the 07:03 session having overwritten it the same way. HEAD is fixed (seek End(0) added in 25559df) and lib.rs:537 writer_resumes_from_last_segment is a genuine regression test, but the retained capture remains corrupted and must not be treated as a faithful single-session record.

**Recommendation:** Document in the data directory (e.g. data/wal/README) that segment 0's first 214 bytes were rewritten by a later session and the file is not session-faithful; treat 07:03:29 as the earliest trustworthy WAL record. Keep the existing reopen regression test, and add a stronger invariant test: after reopen+append, re-read the ENTIRE segment and assert the original head frames are byte-identical. Consider adding a per-frame monotonic writer epoch/sequence in the frame header so head-overwrite corruption is detectable at read time instead of silently parseable.

### A.8 Replay validation matched=true is vacuous: the reference checkpoint validates against a book initialized from itself — confirmed live against captured data
- **Severity:** high  |  **Area:** data-artifact-forensics  |  **Location:** `crates/pb-replay/src/engine.rs:113`

replay_validation() picks the first checkpoint with checkpoint_timestamp_us > replay_ts as reference (engine.rs:107), then calls reconstruct_at(asset, reference.checkpoint_timestamp_us, mode). reconstruct_at calls read_latest_checkpoint(asset, target_ts) which uses an inclusive <= bound, so it returns the reference checkpoint itself; apply_checkpoint seeds the book from it and zero events replay in the empty (ref_ts, ref_ts] window. The comparison in books_match_checkpoint (engine.rs:350) is then checkpoint-vs-itself and trivially true. Empirically confirmed: `cargo run -- replay --token 1076195558... --at 1773039860000000 --validate` returned matched=true; an independent reconstruct at exactly the reference timestamp 1773039868961000 printed 'Used checkpoint: true' with 99 bids/0 asks — byte-identical to the reference checkpoint — proving no independent reconstruction occurred. No replay_validations dataset exists anywhere in the captured data, so validation has also never been exercised operationally.

**Recommendation:** In replay_validation, reconstruct using only checkpoints strictly OLDER than the reference (e.g. read_latest_checkpoint(asset, reference_ts - 1) or exclude the reference's timestamp), or reconstruct from the previous snapshot/checkpoint and replay events up to reference_ts. Add a test asserting that a deliberately corrupted event stream yields matched=false.

### A.9 WAL pruning/retention documented as a live operational capability but never invoked anywhere in the runtime; `WalPruner` type does not exist
- **Severity:** high  |  **Area:** docs-spec-drift  |  **Location:** `docs/operations.md:345`

Operations docs and CLAUDE.md present WAL segment reclamation as a shipped runtime property: /Users/weiming/Documents/GitHub/poly-book/docs/operations.md:345 lists "WAL gap detection, lag tracking, and backpressure-aware pruning" under Current Scope, and /Users/weiming/Documents/GitHub/poly-book/CLAUDE.md:37 says "`WalPruner` reclaims" (also CLAUDE.md:106 "backpressure pruning for multi-replica setups"). In code, `WalWriter::prune()` and `prune_with_backpressure()` exist (/Users/weiming/Documents/GitHub/poly-book/crates/pb-wal/src/writer.rs:75,99) but are never called from any pb-bin command — grep over crates/ shows zero runtime call sites; the `ingest` event loop (/Users/weiming/Documents/GitHub/poly-book/crates/pb-bin/src/commands/ingest.rs:120-148) only appends and flushes. There is no `WalPruner` type anywhere. Consequence: an operator relying on the documented retention behavior gets unbounded WAL disk growth in the ingest process.

**Recommendation:** Either wire periodic `prune_with_backpressure()` into the ingest event loop (it owns the WalWriter) or rewrite docs/operations.md Current Scope, CLAUDE.md:37/:106, and AGENTS.md to state that pruning is an API-only capability not yet scheduled, and document the resulting unbounded-growth operational risk.

### A.10 WebSocket lifecycle race: shared unmountedRef across effect runs leaks sockets and spawns ghost reconnect loops
- **Severity:** high  |  **Area:** frontend  |  **Location:** `web/src/shared/hooks/use-orderbook-stream.ts:33`

useOrderBookStream uses a single `unmountedRef` shared across effect runs: cleanup sets it true, but the next effect run (new assetId, or StrictMode re-mount) immediately resets it to false. The old socket's `onclose` always fires asynchronously AFTER that reset, so it passes the guard, sets `wsRef.current = null` (clobbering the new socket so cleanup can never close it), flips status to 'reconnecting', and schedules `setTimeout(connect, delay)` with the OLD closure's assetId into a timer variable whose cleanup already ran. Result: switching assets creates a parallel reconnect loop for the stale asset, a leaked open WebSocket for the current asset, status flapping, and unbounded socket/memory growth over a long session. StrictMode dev double-invoke triggers the same race on every mount.

**Recommendation:** Replace the shared ref with a per-effect-run `let cancelled = false` local captured by connect/onclose, and have every handler also verify `wsRef.current === ws` (instance identity) before acting. Clear ws.onclose before calling ws.close() in cleanup so a deliberate close never schedules a reconnect. Add a unit test that changes assetId and asserts the old socket's onclose does not open a new connection.

### A.11 WAL is never flushed or fsynced in steady state — up to 64 KB of acknowledged records lost on crash, and zero fsync even on graceful shutdown
- **Severity:** high  |  **Area:** hft-gap  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:133`

The ingest loop calls wal.append() per record, which only writes into a 64 KiB BufWriter (segment.rs:16). WalWriter::flush() is called once, at graceful shutdown (ingest.rs:151-155), and WalWriter::sync() (fdatasync) is never called anywhere in production code — rotation only flushes to page cache. Consequences: (1) a panic/OOM/SIGKILL of the ingest process silently loses up to 64 KB of records that were already counted as ingested; (2) an OS crash or power loss can lose everything since the last rotation because nothing ever reaches stable storage via fsync; (3) the serve process's live WAL tail cannot see records until 64 KB accumulates, so in a quiet market the "live" read model can lag by minutes with no bound. This directly contradicts the zero-data-loss / "serve can be killed and restarted without data loss" claims in docs/serve-api.md:97.

**Recommendation:** Add a configurable flush/sync cadence to the ingest loop (e.g., flush on every record or every N ms for tail visibility, fdatasync on an interval and on segment rotation/seal), and document the explicit durability window (group-commit style). Make WAL append failure observable (metric + escalation), not just a warn.

### A.12 A single transient sink failure (ClickHouse insert or Parquet write) kills the sink and then shuts down the entire ingest pipeline — no retry, no degraded mode
- **Severity:** high  |  **Area:** hft-gap  |  **Location:** `crates/pb-store/src/clickhouse_sink.rs:66`

Both sinks propagate the first flush error out of run() (`self.flush(&mut buffer).await?`), terminating the sink task. The fanout forwarding task then observes the closed channel and breaks, which makes pipeline::fanout_event return false, which breaks the main ingest loop (ingest.rs:145-147 `if !pipeline::fanout_event(...) { break; }`) and gracefully shuts down the whole process — including the WebSocket feed and WAL writing. A ClickHouse restart, network blip, or one transient Parquet IO error therefore halts all market-data capture. The buffered batch is also dropped on error (no dead-letter, no retry), so storage semantics are at-most-once and the WAL is not used to repair sink gaps. At a trading firm, capture must degrade (keep feed + WAL alive, retry sinks with backoff) rather than stop.

**Recommendation:** Wrap sink flushes in bounded retry with exponential backoff and a spill/dead-letter path; never let a sink error terminate the feed/WAL loop. Treat the WAL as the durable source of truth and make sinks WAL consumers with committed positions so crash/dependency outages are repaired by replay (exactly-once-per-sink via consumer offsets).

### A.13 Checkpoint wal_offset is never populated despite docs claiming "WAL offset capture" — every serve cold start replays the entire retained WAL, and the skip math uses the wrong segment size
- **Severity:** high  |  **Area:** hft-gap  |  **Location:** `crates/pb-replay/src/backfill.rs:141`

docs/architecture.md:132 describes "CheckpointProducer (periodic REST snapshots + WAL offset capture)", and hydration is designed to seek to checkpoint.wal_offset. But checkpoint_from_rest hardcodes `wal_offset: None`, and no production code path ever sets it (WalWriter::global_offset exists but has no caller). Hydration therefore always replays from the earliest WAL segment — up to max_segments x 64 MB (1 GB default) of records per restart — making the recovery-time objective unbounded relative to retention rather than bounded by checkpoint cadence. There is also a latent correctness bug: replay_wal_tail computes global offsets as seg_id * config.segment_size using `WalConfig { ..Default::default() }` (64 MB) instead of the operator-configured wal.segment_size_mb, so if wal_offset were ever populated with a non-default segment size, the skip boundary would be wrong and records could be skipped or double-applied.

**Recommendation:** Plumb the live WAL position into checkpoint records at append time in the ingest loop (the writer and checkpoint both flow through the same task), pass the real WalConfig into hydrate(), and add an integration test asserting cold-start replay reads only post-checkpoint records. Correct architecture.md until implemented.

### A.14 WAL reader silently skips corrupt records into the live read model, and the tail loop re-reads the entire active segment with blocking I/O every 50 ms
- **Severity:** high  |  **Area:** hft-gap  |  **Location:** `crates/pb-wal/src/reader.rs:105`

Two issues in the serve read path. (1) On CRC mismatch the reader logs a warning and skips the frame; no metric, no ingest/integrity event, and the LiveReadModel keeps serving a book that silently lost a delta — corruption becomes invisible state divergence. A corrupt length field could also make the skip (`FRAME_HEADER_LEN + len`) jump into the middle of subsequent valid frames, cascading further skips. Truncated records are treated as end-of-segment, which is only correct for tail truncation. (2) WalReader::advance_segment's catch-up branch executes std::fs::read of the whole current segment file (up to 64 MB) just to compare lengths, and the serve tailer polls this every 50 ms directly on the tokio runtime (serve.rs:178-207, plus fs::metadata calls in lag_bytes per iteration) — blocking, allocation-heavy O(segment_size) work per poll that grows quadratically over a segment's lifetime and stalls the async executor.

**Recommendation:** Emit a counter and a persisted integrity event on CRC skip and force a resync/rehydration rather than continuing; switch tailing to incremental reads (retain a file handle, stat for size, read only the delta from the last offset) and run file I/O via spawn_blocking or a dedicated thread.

### A.15 Production deploy path is non-functional: Docker image cannot build (3 independent causes) and the ECS task command crash-loops
- **Severity:** high  |  **Area:** ops  |  **Location:** `Dockerfile:11`

The Docker build fails before producing a binary: (1) builder uses rust:1.93-slim but the workspace pins rust-version = "1.94" (Cargo.toml:22) and rust-toolchain.toml pins 1.94.0, so cargo refuses to compile; (2) the Dockerfile copies only Cargo.toml, Cargo.lock, and crates/ but the workspace declares member "tests/integration" (Cargo.toml:15), so manifest loading fails immediately; (3) pb-grpc's build.rs runs tonic_prost_build::compile_protos which requires protoc, which is never installed in the builder stage (CI needs the .github/actions/setup-protobuf action for exactly this). Even if the image built, infra/ecs.tf:36 runs `ingest` with no --tokens flag, and crates/pb-bin/src/commands/ingest.rs:19 bails: "--tokens is required" (tokens is CLI-only, not settable via PB__ env), so the ECS task exits on startup and the service crash-loops. CI never builds the Docker image, so all of this merges green and is only discovered when the deploy workflow (every push to main) fails.

**Recommendation:** Fix the builder image to match the pinned toolchain (or COPY rust-toolchain.toml and let rustup resolve it), COPY tests/ (or exclude the member via a build profile), and apt-get install protobuf-compiler in the builder stage. Change the ECS command to `auto-ingest` or add `--tokens` via task definition. Add a `docker build` job to ci.yml so image breakage blocks merge instead of failing post-merge deploys.

### A.16 WAL has no steady-state flush or fsync; append failures are warn-and-continue — crash loses buffered events and serve read model can lag indefinitely
- **Severity:** high  |  **Area:** ops  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:151`

WalWriter::flush() is called only during graceful shutdown; WalWriter::sync() (fsync) is never called anywhere in pb-bin. Records sit in a 64 KiB user-space BufWriter (segment.rs:14) until the buffer fills or the segment rotates. On SIGKILL/OOM (Fargate gives 512 MB; Spot reclaim force-kills after the stop timeout) up to 64 KiB of acknowledged events are lost from the WAL, and because Parquet buffers up to 5 minutes in memory and there is no WAL→sink recovery replay, those events are gone permanently — directly violating the zero-data-loss goal and the operations.md claim of restart "without data loss". On power loss, everything in page cache is lost since fsync is never issued. Additionally, WAL append/encode errors are logged at warn and ingestion continues (ingest.rs:136-142) with no metric counter, so durability silently degrades (e.g. disk full) with nothing for on-call to alert on. Side effect: the separated `serve` process tails the WAL file, so during quiet markets the live read model can be stale by up to 64 KiB of un-flushed events with no upper time bound.

**Recommendation:** Add a periodic flush (e.g. every N ms or M records) and a configurable fsync policy (per-batch or interval-based) to the ingest WAL loop. Treat repeated WAL append failures as fatal or at minimum emit a `pb_wal_append_failures_total` counter and flip a readiness flag. Document the explicit durability window in operations.md.

### A.17 WAL is never pruned in any production code path; wal.max_segments is a dead config knob — unbounded disk growth under 24/7 ingest
- **Severity:** high  |  **Area:** ops  |  **Location:** `crates/pb-wal/src/writer.rs:75`

WalWriter::prune() and prune_with_backpressure() exist and are tested, but no production caller exists — grep across pb-bin and all crates finds zero invocations outside pb-wal's own tests (serve.rs mentions pruning only in a comment about gap detection). The `wal.max_segments` setting (default 16) is read into WalConfig (pipeline.rs:156) and documented in config/default.toml:29 and docs, with lib.rs:26 claiming "Oldest sealed segments are pruned", but nothing in writer.rs/segment.rs ever enforces it. CLAUDE.md and the architecture docs describe a "WalPruner [that] reclaims" and "backpressure pruning for multi-replica setups" — none of it is wired. A 24/7 ingest process therefore accumulates 64 MB segments forever until the disk fills, at which point WAL appends fail (warn-only, see prior finding) and eventually Parquet local buffering/checkpointing breaks too.

**Recommendation:** Spawn a periodic pruning task in `ingest` (and `serve` if it owns positions) that calls prune_with_backpressure with the known consumer position files, and enforce max_segments as a hard cap (with a needs_resync signal for lagging consumers rather than disk exhaustion). Add a `pb_wal_segments` / `pb_wal_disk_bytes` gauge.

### A.18 WAL flushed only at shutdown/rotation: crash loses buffered records and serve tail lags
- **Severity:** high  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:150`

WalWriter::append goes through a 64 KiB BufWriter (pb-wal/src/segment.rs:14) and ingest.rs never calls wal.flush() inside the event loop — only once after the loop exits (and never wal.sync()/fsync). Two consequences: (1) on crash/OOM-kill/power loss, up to 64 KiB of records that were already fanned out to Parquet/ClickHouse are absent from the WAL, so a serve replica hydrating/tailing the WAL permanently diverges from the stores with no detection; (2) the serve process tails the on-disk file, so in a quiet market records sit in ingest's user-space buffer indefinitely — serve's read model and WS broadcasts can lag arbitrarily (well past api.stale_after_secs=15), making live data stale while ingest is perfectly healthy.

**Recommendation:** Flush the WAL on a bounded cadence in the hot loop (e.g., flush when the channel is momentarily empty, or on a 50-100ms tick) and add a configurable periodic sync() for fsync durability. Also escalate WAL append/open failures (ingest.rs:75-84, 136-143) beyond warn-and-continue — at minimum a metrics counter and a health/ready flag, ideally fail-fast since the WAL is the serve replica's source of truth.

### A.19 Auto-rotate drops the final ~10 seconds of every 5-minute market
- **Severity:** high  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/commands/auto_ingest.rs:134`

Both auto_ingest.rs and serve_api.rs auto-rotate sleep until `target_bucket - 10` (10 seconds before the next window starts), then discover the next market, cancel the old WS subscription, and spawn a new client subscribed only to the new market's tokens. The old market for bucket B remains live until B+300, but its feed is cancelled at ~B+290 — so the last ~10 seconds of every single market window (the settlement endgame, typically the most informative period of a 5-minute up/down market) are systematically never ingested, recorded to WAL, or persisted. There is also a discovery-latency gap of a few seconds before the new subscription is live.

**Recommendation:** Overlap subscriptions: spawn the new WS client for the next market while keeping the old subscription alive until its window actually expires (B+300 plus a grace period), then cancel the old token. A single WsClient subscribed to the union of both token sets during the overlap also works.

### A.20 WAL pruning never wired: unbounded disk growth, wal.max_segments is dead config
- **Severity:** high  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/commands/pipeline.rs:156`

pb-wal implements WalWriter::prune and prune_with_backpressure (writer.rs:75, 99), and config exposes wal.max_segments=16 and wal.max_consumer_lag_bytes, but no code in pb-bin (or anywhere) ever invokes pruning — grep shows zero call sites. The ingest WAL therefore grows without bound (64 MB per segment, forever). When the disk eventually fills, WAL appends and Parquet flushes both start failing, and per the warn-and-continue handling in ingest.rs those failures are silent, producing real data loss. CLAUDE.md's claim of "backpressure pruning for multi-replica setups" is not implemented at the process level, and max_segments is read into WalConfig (pipeline.rs:156) but used by nothing.

**Recommendation:** Add a periodic pruning tick to the ingest event loop (e.g., after each segment rotation or on a timer) calling prune_with_backpressure with the discovered consumer_*.pos files, and either enforce max_segments or remove the dead key from config and docs.

### A.21 Stale-snapshot guard uses `<=`, silently dropping legitimate same-millisecond book snapshots
- **Severity:** high  |  **Area:** pb-feed  |  **Location:** `crates/pb-feed/src/dispatcher.rs:172`

Polymarket emits a full `book` event per trade with a millisecond-resolution timestamp. Two trades in the same millisecond (common in active 5-minute BTC markets) produce two snapshots with equal timestamps; the `exchange_ts <= last_ts` check classifies the second — the newer book state — as stale and drops it (only an IngestEvent is persisted). Because trade-induced book changes are conveyed via `book` events rather than `price_change`, the dropped state is not healed until the next trade's snapshot, leaving persisted/live book state wrong in exactly the high-activity bursts that matter most. Snapshots with unparseable/missing timestamps (exchange_ts == 0) bypass the guard entirely and also never update the tracker.

**Recommendation:** Skip only strictly older snapshots (`exchange_ts < last_ts`); applying an equal-timestamp snapshot is idempotent at worst and correct when state advanced within the same millisecond. Use the venue `hash` field to deduplicate true retransmits of identical state.

### A.22 gRPC RPCs bypass every input guard the HTTP layer enforces; hostile timestamps can OOM/stall the serve process
- **Severity:** high  |  **Area:** pb-grpc-metrics  |  **Location:** `crates/pb-grpc/src/lib.rs:107`

The HTTP API validates time windows (start_us < end_us, window <= 24h via MAX_QUERY_WINDOW_US at pb-api/src/server.rs:292-305), clamps execution limit to 1..=1000 (server.rs:353-357), and validates depth against max_depth (server.rs:469-481). The gRPC handlers pass req.at_us, req.start_us/end_us, req.depth, and req.limit straight to pb-service with zero validation. Consequences: (1) IntegritySummary/ExecutionTimeline with a far-future end_us (e.g. 4e18 µs ≈ year 128,000, valid for chrono) drives ParquetReader::hour_paths (pb-replay/src/reader.rs:88-98) into ~1.1 billion synchronous loop iterations building PathBufs — tens of GB of allocation and a blocked tokio worker, i.e. remote OOM/DoS of the serve process; (2) limit = u32::MAX returns the entire execution dataset in one response (tonic's encoding size is unlimited by default); (3) unbounded windows make build_integrity_summary load every event in the window into memory. The README claims backend behavior 'applies equally to gRPC and HTTP' but the guard rails do not.

**Recommendation:** Move validate_time_window, the execution limit clamp, and depth validation down into pb-service (single source of truth) so both transports inherit them; additionally clamp hour_paths iteration count in pb-replay as defense in depth.

### A.23 Replay validation is vacuous: reconstruction seeds from the reference checkpoint itself, so matched is always true
- **Severity:** high  |  **Area:** pb-replay  |  **Location:** `crates/pb-replay/src/engine.rs:105`

replay_validation picks the first checkpoint strictly after replay_timestamp_us as the reference, then calls reconstruct_at(asset, reference.checkpoint_timestamp_us). Inside reconstruct_at, read_latest_checkpoint(asset, target_us) is inclusive (`checkpoint_timestamp_us <= ?` in ClickHouse at reader.rs:1307; `checkpoint_ts > end_us -> skip` filter in Parquet extract_checkpoints at reader.rs:665), so it returns the reference checkpoint itself. The market-data window is then [reference_ts, reference_ts] and the event loop skips everything (strict `>` at engine.rs:181), so the reconstructed book is a byte copy of the reference checkpoint. books_match_checkpoint then compares the checkpoint with itself and always reports matched=true. The persisted ReplayValidation rows feeding the integrity summary (pb-service) and `pb-bin replay --validate` therefore provide false confidence and can never detect real replay divergence. The unit test masks this because MockReader::read_latest_checkpoint returns None while with_checkpoints is set — a combination a real reader can never produce (tests.rs:517-551).

**Recommendation:** Make validation reconstruct independently of the reference: bound the checkpoint search at replay_timestamp_us (e.g., a reconstruct variant taking a separate checkpoint_search_bound, or search `checkpoint_timestamp_us < reference_ts`), so the book is rebuilt from an earlier checkpoint/snapshot plus deltas and only then compared to the reference. Fix MockReader so read_latest_checkpoint derives from the configured checkpoints list, and add a test asserting a divergent event stream produces matched=false.

### A.24 SQL guard is a write-keyword blocklist with no table/function allowlist: SELECT-rooted queries reach ClickHouse table functions (file/url/s3/remote) and system tables
- **Severity:** high  |  **Area:** pb-service  |  **Location:** `crates/pb-service/src/query.rs:181`

validate_read_only() only (a) requires the root token be in ALLOWED_ROOT_KEYWORDS (SELECT/WITH/SHOW/DESCRIBE/EXPLAIN) and (b) rejects a fixed list of DDL/DML keywords. It places NO restriction on which tables or table functions an accepted SELECT may reference. Any read-rooted query that uses ClickHouse table functions is accepted: `SELECT * FROM file('/etc/passwd','LineAsString')` (arbitrary server file read), `SELECT * FROM url('http://169.254.169.254/...')` (SSRF), `SELECT * FROM s3(...)`/`remote(...)`/`mysql(...)` (exfiltration/lateral movement), and `SELECT * FROM system.users`/`system.*` (credential & config disclosure). The connection sends none of ClickHouse's own defenses: ClickHouseQueryService::new builds `query_url = {url}/?database={db}&default_format=JSONCompact` with no `readonly=1`, no settings_constraints, and no auth header (query.rs:286). A `SETTINGS max_execution_time=0, max_memory_usage=0` suffix is likewise accepted because read-only mode is not enforced. The pb-api server has no authentication and binds 0.0.0.0:3000 by default, so enabling api.query_workbench_enabled exposes this surface to the network.

**Recommendation:** Defense in depth: (1) connect as a dedicated ClickHouse user constrained by a readonly=2 profile with table-function access (file/url/s3/remote/mysql/jdbc/executable) revoked and system database access denied; append `&readonly=1` style settings constraints to the HTTP endpoint. (2) In the guard, allowlist the specific datasets the workbench is meant to expose (book_events, trade_events, etc.) and reject any identifier resolving to system.* or to a table function; reject a `SETTINGS` clause entirely. Treat the textual guard as secondary to a privilege-restricted CH user.

### A.25 ClickHouse persistence is non-functional: DDL rejected (Nullable in ORDER BY) and Enum8 columns written as String over RowBinary
- **Severity:** high  |  **Area:** pb-store  |  **Location:** `crates/pb-store/src/writer.rs:38`

Two independent defects make the ClickHouse sink unable to persist market data. First, the DDL for book_events (ORDER BY ... sequence Nullable(UInt64)), trade_events (trade_id Nullable(String)), and ingest_events (source_session_id Nullable(String)) places Nullable columns in the sorting key, which ClickHouse rejects unless allow_nullable_key=1 (off by default since ~21.x). Verified against clickhouse-server 26.2: 'Code: 44. DB::Exception: Sorting key contains nullable columns... (ILLEGAL_COLUMN)'. ensure_tables() therefore fails, pipeline.rs only logs a warning, and the first insert then fails on a missing table. Second, even with tables force-created, BookEventRow/TradeEventRow/ExecutionEventRow serialize event_kind and side as Rust String, but the columns are Enum8; RowBinary encodes Enum8 as Int8, so the server misparses the stream. Verified empirically: a byte-exact replica of the writer's row fails with 'Code: 33 CANNOT_READ_ALL_DATA (Bytes read: 36, Bytes expected: 64)', while the identical row with Int8 enum bytes inserts and reads back correctly. The pb-replay reader row structs (reader.rs:1038-1066) have the same String-vs-Enum8 mismatch on SELECT. The roundtrip tests that would catch all of this (tests/integration/clickhouse_roundtrip.rs) are #[ignore] and CI runs cargo test --exclude pb-integration-tests, so the path has never been validated.

**Recommendation:** Remove Nullable columns from ORDER BY (e.g. ORDER BY (asset_id, recv_timestamp_us, price) and coalesce sequence, or store sequence as UInt64 with 0 sentinel). Change enum fields to #[repr(i8)] enums with serde_repr (per the clickhouse crate docs) or change columns to LowCardinality(String). Fix the matching reader structs in pb-replay. Un-ignore the testcontainers roundtrip tests in a CI job with Docker so the CH path is continuously verified.

### A.26 Single flush error permanently halts the entire ingest pipeline, drops the buffered batch, and exits with code 0
- **Severity:** high  |  **Area:** pb-store  |  **Location:** `crates/pb-store/src/clickhouse_sink.rs:66`

Both sinks propagate any flush error straight out of run()/run_with_token() ('self.flush(&mut buffer).await?'), terminating the sink task with the un-flushed batch still in memory. There is no retry, no backoff, no dead-letter, and no re-queue. In pb-bin the failure cascades: the sink's rx drops, the forwarding task's send fails and breaks (ingest.rs:96-99), fanout_event returns false, and the main ingest loop breaks (ingest.rs:145-147) - so one transient ClickHouse restart or S3 503 stops BOTH sinks and all ingestion, and run() returns Ok so the process exits 0, defeating Restart=on-failure supervisors. The log in pipeline.rs:96 ('will retry on insert') describes retry behavior that does not exist. Given the broken CH DDL (previous finding), enabling --clickhouse today halts the whole ingest pipeline about one second after the first event.

**Recommendation:** Make flush errors non-fatal inside the sink loop: retry with capped exponential backoff while retaining the buffer (it is already kept on error - only the early return discards it), emit a metric/alert on consecutive failures, and only abort after a configurable retry budget. Decouple sinks so one failing sink does not stop the other (per-sink isolation instead of fanout_event returning false). Return a non-zero exit when ingestion stops due to sink failure.

### A.27 Up to 5 minutes of records exist only in sink memory; crash loses them permanently with no WAL-to-storage reconciliation
- **Severity:** high  |  **Area:** pb-store  |  **Location:** `crates/pb-store/src/parquet_sink.rs:13`

ParquetSink buffers records in a plain Vec for DEFAULT_FLUSH_INTERVAL (300s); ClickHouseSink for 1s/10k rows. On panic, OOM, or SIGKILL the buffered window is gone from the storage datasets. The WAL (written in ingest.rs before fanout) does capture the events, but no command or tool replays WAL contents into Parquet/ClickHouse - WalReader is consumed only by the serve read-model (serve.rs hydration/tail), and the WAL is bounded (max_segments=16 x 64MB plus backpressure pruning), so the loss becomes permanent. Additionally, on graceful shutdown the final flush is bounded by shutdown_handles' 10s timeout (pipeline.rs:334-341); a 5-minute buffer flushing to slow object storage can exceed this, after which main returns, the runtime drops, and the in-flight flush task is aborted.

**Recommendation:** Document the 5-minute window as accepted RPO or shrink it; add a 'wal replay-to-storage' subcommand (WAL consumer position per sink) so crash windows can be reconciled - the WAL infrastructure with independent consumer positions already exists. Raise or make configurable the shutdown flush timeout, and consider flushing on a row-count threshold so the at-risk window is bounded in rows, not only time.

### A.28 backfill command's relative base path breaks every Parquet flush - all backfilled data is lost while the command appears to run
- **Severity:** high  |  **Area:** pb-store  |  **Location:** `crates/pb-bin/src/commands/backfill.rs:48`

pipeline.rs (ingest path) canonicalizes storage.parquet_base_path to an absolute path before constructing ParquetSink, but backfill.rs passes the raw config value straight through. The default config is parquet_base_path = "./data". ObjectPath::from("./data/...") percent-encodes the '.' segment and LocalFileSystem::new() resolves from filesystem root, so every flush tries to create '/%2E/data/...' and fails. Verified empirically with object_store 0.13: parsed path is '%2E/data/...' and put fails with 'Unable to create dir /%2E/data/...: Read-only file system'. Because the sink task only logs 'parquet sink failed during backfill' (backfill.rs:53) and the command still ends with 'backfill complete', an operator collecting REST snapshots loses the entire run silently unless they read logs.

**Recommendation:** Extract the canonicalization in pipeline.rs:55-62 (and execution_append.rs resolve_parquet_base_path) into one shared helper and use it in backfill.rs. Better: have ParquetRecordWriter validate/canonicalize base_path itself, or construct LocalFileSystem::new_with_prefix(base_path) so relative configs cannot escape. Propagate sink failure into the backfill command's exit status.

### A.29 No fsync ever issued on the WAL write path in production; data-loss window is the entire BufWriter on process crash and all unsynced page cache on OS crash
- **Severity:** high  |  **Area:** pb-wal  |  **Location:** `crates/pb-wal/src/writer.rs:150`

Segment::sync() (flush + fdatasync) exists but has zero production callers. The ingest loop (pb-bin/src/commands/ingest.rs:133-155) only appends; the sole flush() is at graceful shutdown. rotate() calls flush() only — sealed segments are never fsynced, and the directory is never fsynced after creating a new segment file, so after power loss a newly rotated segment can vanish entirely. Concrete windows: (a) process crash (SIGKILL/panic): up to 64 KiB of appended-and-acked records sit in the BufWriter (segment.rs:16, BUF_WRITER_CAPACITY) and are lost; (b) OS crash/power loss: everything not yet written back by kernel writeback policy is lost — the application provides no durability bound at all. Secondary consequence: because WalReader reads the file from disk, records buffered in the BufWriter are invisible to the serve process; at low event rates the live read model can lag by up to 64 KiB of events indefinitely (no periodic flush exists). The README's claim of 'Explicit durability: WalWriter::sync() calls fdatasync on demand' is never demanded.

**Recommendation:** Add a flush/sync policy to WalWriter: flush() on a short interval (5-50 ms) or per-append for live-tail visibility, fdatasync on a configurable durability interval or byte budget, and on rotation fsync the sealed segment then fsync the directory (also fsync the directory after the first segment is created). Wire this into the ingest event loop with a tokio interval.

### A.30 Writer reopen does not scan/truncate a torn or zeroed segment tail, causing misaligned frames, reader stall, and silent loss of post-restart records
- **Severity:** high  |  **Area:** pb-wal  |  **Location:** `crates/pb-wal/src/segment.rs:51`

Segment::open_append resumes at metadata().len() without validating that the tail ends on a frame boundary. After a crash mid-frame (BufWriter flush is not atomic) the writer appends new frames immediately after the torn bytes: the torn frame's stale length field then points into the new data, producing a CRC mismatch whose skip lands mid-record, cascading into misparse or TruncatedRecord that drops the rest of the segment. Worse, after an OS crash on ext4/XFS with delayed allocation the tail can be zero-filled: read_record_at treats len==0 as clean end-of-data (segment.rs:156-159), so a reader stalls permanently before the zeroed region (advance_segment's grew-check at reader.rs:274-281 never gets past it) and, after the writer rotates, silently skips every record the writer appended after the zeros — no error, no log.

**Recommendation:** On WalWriter::open, scan the last segment frame-by-frame from offset 0 (or a known-good checkpoint) and ftruncate at the first invalid/torn/zero frame before resuming appends. This is standard WAL recovery (cf. RocksDB/etcd). Add crash-recovery tests that simulate torn and zero-filled tails followed by writer reopen.

### A.31 Tail polling re-reads the entire active segment (up to 64 MB) and re-lists the directory on every poll; with the 50 ms serve poll this is ~1.3 GB/s of steady-state I/O
- **Severity:** high  |  **Area:** pb-wal  |  **Location:** `crates/pb-wal/src/reader.rs:253`

When the reader is caught up, every next() call hits the Ok(None) path and calls advance_segment, which (1) runs list_segment_ids — a full read_dir — and (2) reads the whole current segment file with std::fs::read just to compare its length against the cached copy (reader.rs:277: it reads the data first, then checks `data.len() > prev_len`). The serve tailer polls every 50 ms (pb-bin/src/commands/serve.rs:178), so with default 64 MB segments a fully written active segment is re-read ~20x/sec (~1.28 GB/s page-cache traffic plus a 64 MB allocation each time), and even when data does arrive the entire file is re-read instead of the new suffix. lag_bytes() additionally stats every segment per loop iteration (serve.rs:196).

**Recommendation:** Keep an open File handle per segment; stat (metadata().len()) first and return false without any read if unchanged; when grown, pread only the bytes from the previous length to the new length and append to the cached buffer. Only re-list the directory when the current segment is exhausted, or rate-limit listing.

### A.32 Reader permanently stalls returning Ok(None) once current_data is None — empty-dir startup or a prune race silently kills the live tail forever
- **Severity:** high  |  **Area:** pb-wal  |  **Location:** `crates/pb-wal/src/reader.rs:136`

next() short-circuits to Ok(None) whenever current_data is None and never attempts to (re)load or advance. load_segment sets current_data=None when the segment file is missing (reader.rs:243-245). Triggers: (a) WalReader::open on an empty WAL directory (serve process deployed before ingest creates segment 0) — start position defaults to (0,0), load_segment(0) fails, and the reader returns None forever even after the writer creates segments; the serve tailer (serve.rs:207-226) polls forever, /health reports ready (hydrated, no resync), but no live data ever flows; (b) cross-process race: advance_segment lists segments, then the ingest-side pruner deletes the chosen next segment before load_segment reads it — Ok(false) leaves current_data=None and the reader is stuck, with needs_resync() potentially still false because it compares against the stale cached listing.

**Recommendation:** In next(), when current_data is None, refresh the segment list and retry load_segment/advance_segment before returning None. Make open/open_at return WalError::SegmentGap (the variant already exists and is unused) when the requested position's segment is absent, so callers fail loudly instead of stalling.

### A.33 WAL torn-write crash recovery is untested, and inspection shows it silently loses records
- **Severity:** high  |  **Area:** testing  |  **Location:** `crates/pb-wal/src/segment.rs:51`

No test simulates a process crash mid-append (partial frame at segment tail) followed by writer restart. `Segment::open_append` resumes at the raw file length without scanning or truncating a torn tail frame, so a writer that crashed mid-frame (realistic: the ingest hot loop buffers up to 64 KiB in BufWriter and only flushes at shutdown) will append new valid frames immediately after the garbage bytes. A reader reaching the torn offset then reads a garbage length field spanning into the new frames, hits CrcMismatch, and skips `FRAME_HEADER_LEN + garbage_len` bytes (reader.rs:113-118), permanently desyncing framing — every valid record appended after the crash is silently lost for that consumer. The only resume test, `writer_resumes_from_last_segment` (lib.rs:537), covers the clean-shutdown case (flushed, whole frames). `fuzz_wal_corruption` corrupts bytes only AFTER a clean writer shutdown and never reopens a writer over the corrupted file, so this exact crash sequence is outside every test and fuzz input space.

**Recommendation:** Add crash-recovery tests: write N records, flush, then truncate the file mid-frame (and separately mid-header), reopen WalWriter, append M more records, and assert a fresh reader returns all N-1+M intact records. Fix open_append to scan frames from the last known-good offset and truncate the torn tail before resuming. Extend fuzz_wal_corruption to interleave corruption/truncation with writer reopen+append cycles.


## Severity: MEDIUM

### A.34 Checked-in `target-cpu=native` for all Linux and macOS builds: non-reproducible binaries, SIGILL portability risk, and three divergent codegen environments
- **Severity:** medium  |  **Area:** build-dependency-hygiene  |  **Location:** `.cargo/config.toml:1`

The repo-level cargo config applies `-C target-cpu=native` to every build on macOS and Linux. Consequences: (a) any release binary built from a checkout and run on a different host (older microarch EC2/Fargate instance, colleague's machine, future release artifacts) can SIGILL on unsupported instructions; (b) builds are not reproducible across machines; (c) Criterion baselines in pb-types/pb-book/pb-api are tuned to the build host and incomparable across machines. Worse, the flag is applied inconsistently: in CI, `env: RUSTFLAGS: -Dwarnings` (.github/workflows/ci.yml:12) silently overrides target.*.rustflags entirely per cargo's precedence rules (RUSTFLAGS env wins, config rustflags ignored), so CI compiles baseline x86-64 without -Dwarnings... rather, with only -Dwarnings and no native tuning; and the Dockerfile never copies .cargo/, so the production image (if it built) would also be baseline codegen. Net effect: local dev, CI, and production all compile with different flags, local benchmark numbers do not represent production codegen at all, and the only place native tuning actually applies is developer laptops — the one place it matters least.

**Recommendation:** Remove target-cpu=native from the checked-in config. If native tuning is wanted for local benches, put it in a documented cargo alias or per-developer config. For production, pin an explicit microarchitecture floor matching the fleet (e.g. `-C target-cpu=x86-64-v3`) in the Docker build, and apply the same flags in CI and bench runs so measured performance corresponds to shipped performance.

### A.35 Release profile (panic=abort + strip=symbols, no debug info) creates production failure semantics that are never tested, never compiled in CI, undocumented, and untriageable after a crash
- **Severity:** medium  |  **Area:** build-dependency-hygiene  |  **Location:** `Cargo.toml:104`

`[profile.release]` sets `panic = "abort"` and `strip = "symbols"` (with default debug=0). Three compounding problems. (1) Divergent failure modes: dev/test/CI all build with unwinding, where a panicking tokio task is isolated and the process keeps running (the codebase has 25+ tokio::spawn sites across pb-api, pb-bin commands, etc., many with unobserved JoinHandles); in release, the same panic aborts the entire process — ingest, WAL writer, sinks, API — instantly. The failure mode the fleet actually exhibits is literally never executed by any test. Fail-stop is a defensible choice for a durability system, but it is recorded nowhere: no ADR, and grep of docs/, README.md and all crate READMEs finds no mention of panic=abort, lto, or the profile rationale (there are ADRs for mimalloc and FxHashMap but not for process-death semantics). (2) Untriageable crashes: panic=abort + strip="symbols" + debug=0 means a production abort emits a panic message and then an unsymbolizable backtrace of raw addresses — for a system whose bar is 'zero data loss, correctness under every failure mode', the post-mortem story is empty. (3) The release profile is never compiled anywhere automated: ci.yml contains no `--release` job, and the only release consumer (the Docker deploy) has failed at startup on every push (see Docker finding). Any release-only breakage (LTO + codegen-units=1 interactions, abort-runtime linkage) surfaces only at deploy time. No catch_unwind exists in the codebase, so nothing becomes dead code — but nothing guards against the divergence either.

**Recommendation:** Write an ADR documenting fail-stop-on-panic. Keep panic=abort but make crashes triageable: set `debug = "line-tables-only"` (or `debug = 1` + `split-debuginfo`) and change strip to `"debuginfo"` or none, archiving split debug artifacts per release. Add a CI job that builds `--release --locked` (and ideally runs the test suite once under a panic=abort profile or a soak target) so release codegen and abort semantics are exercised before deploy.

### A.36 Durability-critical WAL format built on frozen bincode 1.3.3 positional encoding with no byte-level format pin; deny.toml disables unmaintained-crate detection
- **Severity:** medium  |  **Area:** build-dependency-hygiene  |  **Location:** `crates/pb-wal/src/codec.rs:17`

Every WAL record is `version byte + bincode::serialize(PersistedRecord)` (workspace dep `bincode = "1"`, resolved 1.3.3 — last released 2021; upstream development moved to the incompatible 2.x line, leaving 1.x effectively frozen). Bincode 1 encoding is purely positional: struct field order, enum variant order, and field count are the schema. The codec has a CURRENT_VERSION byte (good), but nothing forces a bump: all codec tests (pb-wal/src/codec.rs:92-304) are self-consistent round-trips, so a developer who reorders or inserts a field in PersistedRecord, BookEvent, or EventProvenance ships a silently different v1 format with a green test suite. Failure scenarios: new binary fails to decode existing segments (hydration/replay outage), or — worse — same-width field reorders (e.g. swapping the two u64 timestamps in EventProvenance, or adjacent Option<u64> sequence fields in IngestEvent) decode *successfully* with transposed semantics: silent data corruption in replay and integrity reporting, the exact class of failure this system exists to prevent. Compounding this, deny.toml sets `unmaintained = "none"`, so supply-chain CI will never surface bincode's maintenance status (this is the unmaintained-policy knob, distinct from the advisory ignores already reported by the security dimension).

**Recommendation:** Commit golden byte fixtures: one encoded blob per PersistedRecord variant checked into the repo, with tests asserting exact bytes for encode and exact decoded values for the stored bytes — this turns any accidental schema drift into a test failure that forces a deliberate version bump. Add a doc comment on PersistedRecord stating that field/variant order is on-disk ABI. Plan migration off bincode 1 (bincode 2 with explicit config, or postcard/prost with the existing version-byte dispatch). Set deny.toml `unmaintained` to at least "workspace"/"transitive" warn.

### A.37 Four-way toolchain skew: pinned 1.94.0 is honored only on developer machines; CI tests a moving stable; Docker builds below MSRV; no MSRV verification job
- **Severity:** medium  |  **Area:** build-dependency-hygiene  |  **Location:** `rust-toolchain.toml:2`

rust-toolchain.toml pins 1.94.0 and workspace rust-version = 1.94, but CI uses `dtolnay/rust-toolchain@stable` (ci.yml:20,30,40,52), which exports RUSTUP_TOOLCHAIN and therefore bypasses the repo pin — CI builds and tests whatever the current stable is, not 1.94.0. The Dockerfile uses rust:1.93-slim, *below* the declared MSRV (hard build error). Fuzz and miri jobs ride a moving `@nightly`. Net: the toolchain that developers use, the toolchain CI validates, and the toolchain production would build with are three different compilers, and the declared rust-version is never actually verified by any job (CI stable is always >= 1.94, so the floor can silently rot). For deterministic-replay and reproducible-build standards, the build toolchain should be a single pinned, verified version everywhere.

**Recommendation:** Point CI at the pinned toolchain (`dtolnay/rust-toolchain@1.94.0` or actions-rust-lang/setup-rust-toolchain which reads rust-toolchain.toml), add a dedicated MSRV check job (`cargo check` with the rust-version toolchain), align the Docker base image with the pin and copy rust-toolchain.toml into the build context, and consider pinning the nightly date for miri/fuzz to avoid unrelated breakage.

### A.38 execution_events primary key does not match the timeline query filter, forcing full-table scans on time-range lookups
- **Severity:** medium  |  **Area:** clickhouse  |  **Location:** `crates/pb-replay/src/reader.rs:1371`

execution_events is ordered by (order_id, event_timestamp_us) (writer.rs:130), but ExecutionService::timeline issues WHERE event_timestamp_us >= ? AND event_timestamp_us <= ? with order_id optional, then appends AND order_id = ? only when an order is given. When order_id is None (the API default for /execution/orders by asset/time), the query filters solely on event_timestamp_us, which is not the ORDER BY prefix, so ClickHouse cannot prune by primary key and scans the whole table; the per-asset limit is applied in Rust after fetching every matching row. Violates schema-pk-prioritize-filters and schema-pk-filter-on-orderby for the dominant access pattern.

**Recommendation:** If cross-order time-range scans are a primary pattern, reorder to lead with time/date (e.g. ORDER BY (event_date, event_timestamp_us, order_id)) or add a minmax/bloom skipping index, and push the limit and an asset_id filter into SQL so the server bounds the result set rather than the client.

### A.39 High-repetition string columns stored as plain String instead of LowCardinality/Enum
- **Severity:** medium  |  **Area:** clickhouse  |  **Location:** `crates/pb-store/src/writer.rs:25`

asset_id (String, leading key of book_events/trade_events/book_checkpoints), plus source, fidelity, mode, status, and the String-typed event_kind in ingest/execution are low-cardinality in practice (a handful of active BTC assets; ~6 sources; 2 fidelities/modes; a fixed status set) yet stored as full String, repeated every row. Per schema-types-lowcardinality (<10K uniques) and schema-types-enum these should be LowCardinality(String) or Enum8 for dictionary encoding, smaller storage, and faster scans; LowCardinality is valid as a leading ORDER BY column. Minor related nits folded here: price is UInt32 though Polymarket FixedPrice raw values fit UInt16 (schema-types-minimize-bitwidth), and microsecond timestamps stored as UInt64 forgo native DateTime64 semantics (schema-types-native-types).

**Recommendation:** Switch asset_id/source/fidelity/mode/status and the String event_kinds to LowCardinality(String) (or Enum8 for fully-closed sets), keeping asset_id as the leading key. Re-evaluate price as UInt16 if the FixedPrice domain guarantees the range.

### A.40 ClickHouse insert strategy ignores async-insert guidance and the documented batch-tuning knobs are dead config
- **Severity:** medium  |  **Area:** clickhouse  |  **Location:** `crates/pb-store/src/clickhouse_sink.rs:12`

The sink flushes on a 1s timer OR at 10,000 buffered rows with no async_insert. On quiet assets the timer fires with far fewer than the 1,000-row minimum (insert-batch-size), creating many tiny parts across six tables; insert-async-small-batches recommends async_insert=1 + wait_for_async_insert=1 for exactly this high-frequency/small-batch shape so the server coalesces parts. Worse, config/default.toml advertises clickhouse_batch_interval_secs and clickhouse_batch_size (also in docs/operations.md), but a grep shows nothing reads them — ClickHouseSink::new hardcodes DEFAULT_BATCH_INTERVAL/DEFAULT_BATCH_SIZE, so operators cannot tune batch behavior despite config and docs claiming they can.

**Recommendation:** Enable async inserts on the writer client (async_insert=1, wait_for_async_insert=1, async_insert_max_data_size, async_insert_busy_timeout_ms) per insert-async-small-batches, and actually wire clickhouse_batch_size/clickhouse_batch_interval_secs into ClickHouseSink (with_batch_size/with_batch_interval) or remove the keys from config and docs.

### A.41 Daily partitioning with no TTL or lifecycle policy grows partitions unbounded
- **Severity:** medium  |  **Area:** clickhouse  |  **Location:** `crates/pb-store/src/writer.rs:37`

Every table uses PARTITION BY event_date (daily). Per schema-partition-low-cardinality this yields ~365 partitions/year per table and grows without bound, and per schema-partition-lifecycle partitioning should serve a retention/lifecycle purpose — but there is no TTL, no DROP PARTITION policy, and no monthly rollup, so old partitions accumulate forever and there is no cheap retention path. For a continuously-running ingestion service this is an operational risk (partition/metadata growth).

**Recommendation:** Switch to monthly partitions (toStartOfMonth / toYYYYMM) to bound cardinality, and add a TTL or scheduled DROP PARTITION for retention so old data is removed as a metadata operation rather than scanned.

### A.42 Integrity/replay reads pull entire time windows over HTTP with no LIMIT or server-side aggregation, then re-sort in Rust
- **Severity:** medium  |  **Area:** clickhouse  |  **Location:** `crates/pb-replay/src/reader.rs:1136`

read_market_data runs three fetch_all queries returning every book/trade/ingest row in [start_us,end_us] with no LIMIT, materializes them all in memory, and re-sorts in Rust even though each SQL already has ORDER BY (lines 1197-1257) — wasted work and unbounded memory on a 24h window for a busy asset. build_integrity_summary (pb-service/src/lib.rs:91) consumes that full window only to compute counts (book_event_count, gap_count, etc.), the exact 'full aggregation on every query' anti-pattern from query-mv-incremental: it should push count()/countIf() to the server or maintain an AggregatingMergeTree incremental MV instead of transferring millions of rows to count them.

**Recommendation:** For integrity summaries issue server-side aggregate queries (count(), countIf by event kind) or back them with an incremental materialized view per query-mv-incremental. For replay, stream or bound rows, and drop the redundant Rust sort since the SQL ORDER BY already returns sorted rows.

### A.43 Query workbench enforces limits only client-side; no ClickHouse server-side readonly/resource guards
- **Severity:** medium  |  **Area:** clickhouse  |  **Location:** `crates/pb-service/src/query.rs:329`

ClickHouseQueryService posts guarded SQL with default_format=JSONCompact and wraps the request in a tokio::time::timeout, but sets no ClickHouse server settings. Read-only enforcement is purely app-level keyword filtering plus a LIMIT injected into the outer query, and the timeout is client-side: when it fires, reqwest drops the response but ClickHouse keeps executing (HTTP cancellation needs cancel_http_readonly_queries_on_client_close). An expensive SELECT (large JOIN, sleep(), system.* scan, or a heavy inner subquery whose outer LIMIT still forces full computation) can consume server resources unbounded despite the client giving up. This is general ClickHouse hardening guidance rather than one of the 28 rules, but matters at a trading-firm bar.

**Recommendation:** Append server-side settings to the query URL or use a dedicated read-only ClickHouse user/profile: readonly=1, max_execution_time=timeout_secs, max_result_rows=max_rows, max_rows_to_read, and cancel_http_readonly_queries_on_client_close=1, so resource and read-only limits are enforced by the server, not just the client.

### A.44 Graceful shutdown drops up to 2048 buffered events before they reach the WAL or sinks
- **Severity:** medium  |  **Area:** concurrency  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:121-131`

The ingest main loop uses 'biased; _ = shutdown.cancelled() => break' and never drains event_rx after cancellation, so up to 2048 events sitting in the channel (plus up to 2048 raw WS messages in the dispatcher's input channel) are discarded before WAL append on every SIGTERM. The 'Write to WAL before fan-out' invariant is violated exactly when durability matters. The backfill command has a related race: its ParquetSink is started with run_with_token(shutdown.child_token()) (backfill.rs:50-55), so on shutdown the sink flushes its buffer and exits while records may still sit undrained in the 10000-capacity channel.

**Recommendation:** Implement ordered drain-on-shutdown: cancel the WS client first, let the dispatcher run to input-channel closure, drop all event senders, then drain event_rx to None (writing each record to WAL and sinks) before flushing and exiting. In backfill, stop the producer and drop event_tx, then let the sink exit via channel-closed (which already flushes) instead of cancelling the sink's token.

### A.45 Unobserved task death: JoinHandles dropped and panics swallowed; a dead projector silently advances WAL consumer positions
- **Severity:** medium  |  **Area:** concurrency  |  **Location:** `crates/pb-api/src/live_state.rs:642`

Many spawned tasks discard their JoinHandle, so panics are silently swallowed and the process keeps running in a degraded state with no health signal. Worst case: the projector ('tokio::spawn(projector.run(cmd_rx, token))', handle discarded, token never cancelled). If it panics, every LiveReadModel mutation becomes a no-op because all senders ignore failure ('let _ = self.cmd_tx.send(...)' then 'let _ = ack_rx.await', live_state.rs:739-746). The serve WAL tailer (serve.rs:208-219) keeps consuming records, sets dirty_position, and commits the consumer position — records are marked consumed but never applied, the watch state freezes at the last published value, and /health still reports ready=true. Similarly, ingest.rs:49-61 drops the WS-client and dispatcher handles; if the dispatcher dies the event channel stays open via the checkpoint producer's tx clone, so ingest appears healthy while persisting only periodic REST checkpoints. auto_ingest.rs:174-186 also drops rotated ws/dispatcher handles, and pipeline.rs:27 drops the metrics-server handle.

**Recommendation:** Watch every JoinHandle (JoinSet or supervisor task) and treat unexpected completion/JoinError::is_panic as fatal or restartable with alerting. Make apply_record return an error (or panic) when cmd_tx is closed so the WAL tailer stops committing positions; surface 'projector dead' and 'feed task dead' in /health.

### A.46 WAL is never flushed or fsynced during normal operation — crash loses buffered records and serve replicas see stale data in quiet markets
- **Severity:** medium  |  **Area:** concurrency  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:151-155`

WalWriter::flush() is called only at shutdown and on segment rotation; WalWriter::sync() (fsync) has zero production call sites. Records accumulate in a 64KB BufWriter (segment.rs:16), so: (1) on process crash (panic, OOM-kill) up to 64KB of appended records are lost from the WAL even though append() returned Ok; (2) on power loss everything in page cache is lost since sync_data is never invoked; (3) the separated serve process tailing the WAL cannot see records until 64KB accumulates — in a quiet market the live read model can lag the feed by an unbounded wall-clock time despite a 50ms poll interval, silently defeating the WAL-tail architecture.

**Recommendation:** Flush the BufWriter on a short timer (e.g. every 25-100ms when dirty) and fsync on a configurable durability interval (e.g. wal.sync_interval_ms), both off-runtime. At minimum flush after every batch drained from event_rx so the serve tailer's visibility latency is bounded.

### A.47 WAL pruning is never invoked and checkpoint wal_offset is never populated: unbounded disk growth and full-history re-hydration on every serve restart
- **Severity:** medium  |  **Area:** concurrency  |  **Location:** `crates/pb-wal/src/writer.rs:75`

WalWriter::prune and prune_with_backpressure have no call sites outside pb-wal's own tests — no command in pb-bin ever prunes, so WAL segments accumulate forever and the wal.max_segments / max_consumer_lag_bytes config is dead (eventually disk-full breaks WAL appends, which ingest only warns about, ingest.rs:136-138). Compounding this, BookCheckpoint.wal_offset is hardcoded None (backfill.rs:141) and never enriched with WalWriter::global_offset() in the ingest loop, so hydration's min_wal_offset is always None and pb-api::hydration replays the ENTIRE WAL from segment 0 through the projector on every serve start — restart time grows without bound. The 'WAL coordination: gap detection, lag tracking, backpressure pruning' described in CLAUDE.md is only partially implemented (gap detection and lag tracking exist; pruning is unwired).

**Recommendation:** Add a periodic pruning task in the ingest runtime calling prune_with_backpressure with the registered consumer position files, and populate checkpoint.wal_offset from WalWriter::global_offset() at append time so hydration can skip already-checkpointed history. Also ensure hydration reconstructs WalConfig with the configured segment_size (hydration.rs:128-131 uses Default), otherwise global-offset math will diverge from the writer once wal_offset is populated.

### A.48 serve WAL tailer exits permanently on resync detection or reader-open failure with no recovery path
- **Severity:** medium  |  **Area:** concurrency  |  **Location:** `crates/pb-bin/src/commands/serve.rs:189-193`

When needs_resync() is true the tailer logs 'triggering re-hydration', sets the atomic, and breaks — but no re-hydration ever happens; the task is dead until process restart (health does at least report ready=false). Worse, if WalReader::open/open_at fails at startup (serve.rs:170-176) the task returns immediately: wal_lag_bytes stays 0, needs_resync stays false, so /health reports ready=true while the live read model never receives another record — the API serves frozen hydration-time books indefinitely with a healthy status.

**Recommendation:** On open failure set needs_resync (or a dedicated tailer_dead flag) so ready=false. Implement the implied recovery: on resync, re-run hydration from the latest checkpoints and reopen the reader at the new position in a retry loop, instead of requiring a process restart.

### A.49 WebSocket reconnect backoff never resets after a successful connection; full-channel backpressure suppresses pings and forces disconnects
- **Severity:** medium  |  **Area:** concurrency  |  **Location:** `crates/pb-feed/src/ws.rs:88-134`

'attempt' increments on every reconnect cycle for the life of the process and is never reset after a healthy connection. After ~9 cumulative disconnects (days/weeks of uptime), every subsequent reconnect waits the 30s cap even if the prior session was healthy for hours — a recurring 30-second data gap per disconnect, directly worsening feed completeness. Separately, when the raw channel (2048) is full because the dispatcher is slow, 'self.tx.send(...).await' inside the stream branch body (ws.rs:181) blocks the entire select loop, so ping_interval ticks are not serviced; the venue will eventually drop the silent connection, converting downstream backpressure into a disconnect/reconnect data gap rather than flow control.

**Recommendation:** Reset attempt to 0 after a connection survives some minimum healthy duration (e.g. 30s connected or first message received). Consider try_send with a small bounded retry or a dedicated ping task on the sink half so pings continue under downstream backpressure.

### A.50 auto-rotate runtimes join or abandon old feed tasks in ways that risk market-start data gaps and orphaned tasks
- **Severity:** medium  |  **Area:** concurrency  |  **Location:** `crates/pb-bin/src/commands/serve_api.rs:274-284`

In serve-api auto-rotate, rotation cancels the old token and then awaits shutdown_handles inline with a 10s timeout per child (up to 20s for ws+dispatcher) before subscribing to the new market; rotation begins only 10s before the bucket boundary, so a slow-to-die child delays the new subscription past market start — a guaranteed data gap at the start of the 5-minute window the system exists to capture. In auto_ingest.rs:163-166 the opposite tradeoff: old children are cancelled with only a yield_now() and their JoinHandles were never retained, so they are never joined at all — a wedged old dispatcher would linger invisibly (and its panics are unobservable, see the unobserved-task-death finding).

**Recommendation:** Start the new market's WS/dispatcher first, then cancel and reap the old generation in the background (store handles in a JoinSet drained opportunistically). Per-asset event keying already makes brief overlap safe.

### A.51 No Parquet schema-version mechanism: the 2026-03-06 capture (old path layout + old schema) is silently invisible to all current readers
- **Severity:** medium  |  **Area:** data-artifact-forensics  |  **Location:** `crates/pb-replay/src/reader.rs:69`

The 03-06 files live at data/{YYYY}/{MM}/{DD}/{HH}/events_{asset}_{ts}.parquet with schema (event_type:u8, sequence required, no source/source_event_id/source_session_id) — predating the split-dataset refactor. Current ParquetReader::hour_paths only scans {base}/{dataset}/{Y}/{M}/{D}/{H}, and pb-store's book_event_schema (schema.rs:15) uses event_kind with three extra columns, so 1.69M rows of captured market data are unreachable and would fail schema mapping even if reached. No file carries any format-version metadata (writer.rs:171 writes plain Arrow schema), so this breakage is silent: a replay over that window simply finds no data rather than erroring. Positive side verified empirically: the 2026-03-09 and 2026-05-12 files are schema-identical to today's schema_for_record output (field names, types, nullability), and an actual `cargo run -- replay` over the 03-09 window read book_events, book_checkpoints and ingest_events correctly — today's readers do read post-refactor files.

**Recommendation:** Embed a format version in Parquet key-value metadata at write time and check it at read time (warn on unknown). Migrate or explicitly quarantine the 03-06 capture. Add a replay-CLI warning when a requested window contains zero files but sibling legacy layouts contain data.

### A.52 All 85 persisted checkpoints have wal_offset=NULL — the checkpoint→WAL hydration handoff has never functioned on real data
- **Severity:** medium  |  **Area:** data-artifact-forensics  |  **Location:** `crates/pb-replay/src/backfill.rs:141`

BookCheckpoint.wal_offset exists specifically so serve hydration can resume WAL tailing from the checkpoint position (pb-api/src/hydration.rs:62). The only checkpoint producer in the captured data is the REST backfill (all 85 rows across 2026-03-09 and 2026-05-12 have source=rest_snapshot), and backfill.rs:141 hardcodes wal_offset: None. Both WAL-resident checkpoints decode with wal_offset=None as well. Consequently hydration always takes the replay_wal_tail(model, wal_dir, None) path, replaying the ENTIRE WAL from the earliest segment through the read model after checkpoints are applied — re-applying records that predate the checkpoint over fresher checkpoint state until the next snapshot arrives in the stream, and scaling cost with total WAL size rather than tail size.

**Recommendation:** Plumb WalWriter::global_offset() into checkpoint creation in the ingest process (the field and hydration consumer already exist), or remove the dead field and redesign hydration to use the source_reset boundary. Add an integration test asserting hydration does not re-apply pre-checkpoint WAL records over checkpoint state.

### A.53 Locked book states (best bid == best ask) occur in real captured delta streams and nothing on the live path detects them
- **Severity:** medium  |  **Area:** data-artifact-forensics  |  **Location:** `crates/pb-book/src/book.rs:175`

Replaying the captured WAL through pb-book's own L2Book with the exact snapshot-grouping logic of pb-api/live_state.rs produces bid==ask states 6 times (5 distinct episodes) for asset 107619555807... AFTER proper snapshot initialization, e.g. recv_ts=1773039811628492 locked at price 0.91, and at 0.94/0.99 later — within a 137-second window. check_integrity() classifies bid>=ask as CrossedBook and would have flagged every one, but it is never invoked on the ingest or serve hot paths, so these states were served as valid books. Venue snapshots themselves are clean — 0 crossed and 0 locked among all 51,849 snapshot groups in 6.29M Parquet book_event rows — so locking arises specifically from delta application order (add-before-remove within one venue update), i.e. mostly transient artifacts that an integrity hook would either confirm as venue states or expose as ordering bugs.

**Recommendation:** Run check_integrity after each applied venue message batch (not per level) on the live path and emit a metric plus an ingest event on bid>=ask; distinguish transient intra-message locking from persistent locked states by checking only at message boundaries.

### A.54 Four config keys documented as live tunables are dead: wal.max_segments, storage.parquet_row_group_size, storage.clickhouse_batch_*, logging.format
- **Severity:** medium  |  **Area:** docs-spec-drift  |  **Location:** `docs/operations.md:30`

The "Current defaults" block in /Users/weiming/Documents/GitHub/poly-book/docs/operations.md:16-66 (mirroring config/default.toml) presents all keys as effective configuration. Four have no effect: (1) `storage.parquet_row_group_size` (operations.md:30) — pb-store hard-codes `ROW_GROUP_SIZE: usize = 65_536` (/Users/weiming/Documents/GitHub/poly-book/crates/pb-store/src/writer.rs:20) and never reads the key; (2) `storage.clickhouse_batch_interval_secs`/`clickhouse_batch_size` (operations.md:35-36) — ClickHouseSink uses compile-time `DEFAULT_BATCH_INTERVAL`/`DEFAULT_BATCH_SIZE` (/Users/weiming/Documents/GitHub/poly-book/crates/pb-store/src/clickhouse_sink.rs:12-13) and pipeline.rs never reads these keys; (3) `wal.max_segments` (operations.md:55) — parsed into WalConfig (/Users/weiming/Documents/GitHub/poly-book/crates/pb-bin/src/commands/pipeline.rs:156) but the field is referenced nowhere in pb-wal's writer/reader/segment implementation (only in lib.rs declaration and tests), and the WalConfig doc-comment even claims "Oldest sealed segments are pruned" (crates/pb-wal/src/lib.rs:26); (4) `logging.format` (operations.md:65) — main.rs reads only `logging.level` (/Users/weiming/Documents/GitHub/poly-book/crates/pb-bin/src/main.rs:159).

**Recommendation:** Wire the keys (preferred for batch/row-group sizes since the constants match the documented defaults) or delete them from config/default.toml and operations.md, and fix the misleading WalConfig doc-comment and pb-wal README key-types row ("max retained segments").

### A.55 Documented Docker/ECS deployment flow cannot build: Dockerfile fails on workspace member, rust-version, and missing protoc
- **Severity:** medium  |  **Area:** docs-spec-drift  |  **Location:** `docs/operations.md:176`

/Users/weiming/Documents/GitHub/poly-book/docs/operations.md:170-199 describes deployment as functioning ("Merges to main trigger the deploy workflow after CI passes... Build the Docker image (multi-stage...)"), and .github/workflows/deploy.yml builds the image on every push to main. The Dockerfile cannot build for three independent reasons: (1) /Users/weiming/Documents/GitHub/poly-book/Dockerfile:15 copies only `Cargo.toml Cargo.lock crates/` while the workspace manifest lists member `tests/integration` (/Users/weiming/Documents/GitHub/poly-book/Cargo.toml:15), so cargo fails to load the workspace; (2) Dockerfile:11 uses `rust:1.93-slim` while workspace `rust-version = "1.94"` (Cargo.toml:22) and rust-toolchain.toml pins 1.94.0, so cargo refuses to compile; (3) the builder stage installs no `protobuf-compiler`, which pb-grpc's build.rs requires via tonic_prost_build (CI needed a dedicated setup-protobuf action for exactly this).

**Recommendation:** Fix the Dockerfile (copy tests/integration or restructure the workspace, bump base image to rust:1.94, apt-get install protobuf-compiler), or amend operations.md to state the deploy pipeline is currently broken/disabled so operators do not assume merges to main produce a deployable image.

### A.56 Four docs describe a 4-surface SPA and a 'deferred' Query Workbench view; the shipped SPA has 6 routes including /orderbook and /query
- **Severity:** medium  |  **Area:** docs-spec-drift  |  **Location:** `docs/operations.md:426`

/Users/weiming/Documents/GitHub/poly-book/docs/operations.md:356-363 ("The SPA currently ships: Live Feed, Replay Lab, Integrity, Execution Timeline") and :422-426 ("Deferred from the Current SPA Pass — ... Query Workbench SPA view (backend routes are implemented and opt-in)"), /Users/weiming/Documents/GitHub/poly-book/docs/api.md:12-20, /Users/weiming/Documents/GitHub/poly-book/docs/serve-api.md:220-227, and /Users/weiming/Documents/GitHub/poly-book/README.md:156-157 all describe a 4-surface SPA. The actual app shell registers six routes — /live-feed, /orderbook, /replay, /execution, /integrity, /query (/Users/weiming/Documents/GitHub/poly-book/web/src/app/App.tsx:106-112) — with full feature modules under web/src/features/orderbook/ and web/src/features/query/, and web/README.md:6-15 correctly documents all six. The Query Workbench SPA view explicitly called 'deferred' exists and ships.

**Recommendation:** Update the shipped-surfaces lists in operations.md, api.md, serve-api.md, and README.md to include Orderbook and Query, and remove the Query Workbench SPA view from every 'deferred' list (keeping the note that the backend remains opt-in via api.query_workbench_enabled).

### A.57 API contract drift: undocumented 24-hour time-window cap on integrity/execution routes, and serve-api.md names the wrong health route
- **Severity:** medium  |  **Area:** docs-spec-drift  |  **Location:** `docs/api.md:166`

docs/api.md documents only "start_us must be less than end_us" for GET /api/v1/integrity/summary (lines 161-167) and GET /api/v1/execution/orders (lines 179-184), but the server additionally rejects any window larger than 24 hours with 400 (`MAX_QUERY_WINDOW_US = 24 * 3_600 * 1_000_000`, /Users/weiming/Documents/GitHub/poly-book/crates/pb-api/src/server.rs:290-305, applied in both handlers at :312 and :352). An operator or client following the doc and querying a multi-day window gets an unexplained 400. The 400-error catalog in api.md:128-134 also omits this case. Additionally, /Users/weiming/Documents/GitHub/poly-book/docs/serve-api.md:201 states "`GET /api/v1/health` returns operational status" while the actual route is `/health` (server.rs:116) — contradicting serve-api.md's own route list at line 164; a health-check probe configured from line 201 would 404.

**Recommendation:** Document the 24h maximum window (and its 400 error) for both routes in docs/api.md, and correct serve-api.md:201 to `GET /health`.

### A.58 architecture.md combined-mode diagram shows a ParquetSink inside the serve-api process; serve-api spawns no storage sinks
- **Severity:** medium  |  **Area:** docs-spec-drift  |  **Location:** `docs/architecture.md:168`

/Users/weiming/Documents/GitHub/poly-book/docs/architecture.md:161-175 (Combined Mode diagram) shows "WsClient ──▶ Dispatcher ──▶ LiveReadModel" with a branch "▼ ParquetSink" inside the serve-api process. The actual serve-api command (/Users/weiming/Documents/GitHub/poly-book/crates/pb-bin/src/commands/serve_api.rs) never calls `start_storage_sinks` and creates no ParquetSink or WAL writer — the dispatcher channel feeds only the LiveReadModel consumer. docs/serve-api.md:102-106 and CLAUDE.md ("serve-api: combined mode ... no WAL") correctly state that API processes do not persist live data, so architecture.md contradicts both the code and its sibling docs. An operator reading the diagram could run serve-api expecting market data to be persisted to Parquet; it is silently not persisted.

**Recommendation:** Remove the ParquetSink box from the combined-mode diagram in docs/architecture.md (or annotate explicitly that serve-api performs no persistence and ingestion requires a separate ingest/auto-ingest process).

### A.59 Archived OpenSpec tasks marked complete for capabilities that never shipped: WalPruner (2.5) and CI benchmark regression gate (8.8)
- **Severity:** medium  |  **Area:** docs-spec-drift  |  **Location:** `openspec/changes/archive/clean-slate-serving-architecture/tasks.md:16`

All 72 tasks in /Users/weiming/Documents/GitHub/poly-book/openspec/changes/archive/clean-slate-serving-architecture/tasks.md are checked. At least two are verifiably not shipped: task 2.5 "[x] Implement `WalPruner`: removes sealed segments that all registered consumers have advanced past" — no `WalPruner` type exists anywhere (functionality landed as never-invoked methods on WalWriter, see the high-severity pruning finding), and the phantom name propagated into CLAUDE.md:37; task 8.8 "[x] Add benchmark regression gate for read model latency (p99 snapshot read)" — /Users/weiming/Documents/GitHub/poly-book/.github/workflows/ci.yml contains no benchmark job or threshold gate of any kind (grep for "bench" returns nothing); pb-api benches exist locally but nothing gates regressions. CLAUDE.md designates archived OpenSpec changes as the authoritative scope record, so falsely-checked tasks corrupt the project's record of what shipped.

**Recommendation:** Amend the archived tasks.md with a post-hoc note (or uncheck) for 2.5 and 8.8 documenting what actually shipped (prune methods exist but unwired; benches exist but ungated), and fix the `WalPruner` name in CLAUDE.md:37.

### A.60 Re-running execution-append silently duplicates events in ClickHouse: no idempotency, dedup key, or batch atomicity on the system's only write path
- **Severity:** medium  |  **Area:** execution-subsystem  |  **Location:** `/Users/weiming/Documents/GitHub/poly-book/crates/pb-bin/src/commands/execution_append.rs:209`

execution-append is the only operator-driven write path, yet it has zero idempotency. The ClickHouse table is plain MergeTree with no version column, no ReplacingMergeTree, no insert dedup token, and no uniqueness on (order_id, kind, event_timestamp_us). Any operator retry — the natural response to a timeout, ambiguous error, or partial Parquet failure (write_batch writes one file per (dataset,asset,hour) group sequentially with no rollback, so a mid-batch failure leaves a partially applied batch) — inserts every event a second time. Duplicate fill rows directly double-count executed quantity in any PnL or execution-quality analysis built on execution_events. The Parquet path has the opposite failure (same-path overwrite, previously reported); the two sinks thus fail in opposite directions on retry and neither is safe.

**Recommendation:** Add a content-derived event_id (e.g. hash of order_id+kind+event_timestamp_us+payload) to ExecutionEvent; use ReplacingMergeTree keyed on it (or ClickHouse insert_deduplication_token per batch) and have the Parquet path embed the event_id in the filename. Alternatively, query for existing (order_id, kind, event_timestamp_us) rows before insert and fail loudly on duplicates unless --force.

### A.61 event_timestamp_us accepts any u64 with no unit or range validation — a ms- or s-resolution timestamp lands in a 1970 partition and becomes permanently invisible to every query
- **Severity:** medium  |  **Area:** execution-subsystem  |  **Location:** `/Users/weiming/Documents/GitHub/poly-book/crates/pb-bin/src/commands/execution_append.rs:179`

The feed path defends against unit mistakes (dispatcher.rs:423-430 multiplies 13-digit ms values by 1000), but execution-append performs no such normalization and no plausibility bound. If an operator supplies milliseconds (1.7e12) or seconds (1.7e9), Parquet partitioning via DateTime::from_timestamp_micros files the record under 1970/01/... and ClickHouse's materialized event_date lands in 1970. The Parquet reader only lists hour directories inside the queried µs range (reader.rs:69-99) and the ClickHouse WHERE compares raw event_timestamp_us, so a correctly-specified 2026 query can never retrieve the record. The command still prints 'Appended 1 execution event(s)' — success reported, record unreachable. event_timestamp_us=0 and u64::MAX are likewise accepted.

**Recommendation:** Reject (or warn-and-confirm) timestamps outside a sane window, e.g. [2020-01-01, now + 1 day] in microseconds, in ExecutionAppendInput validation; apply the same check to every LatencyTrace stage. Reuse the dispatcher's 13-digit heuristic only as a diagnostic ('looks like milliseconds') rather than silent conversion on a manual-entry path.

### A.62 LatencyTrace stages have no monotonicity or unit validation; the web waterfall silently renders negative stage durations
- **Severity:** medium  |  **Area:** execution-subsystem  |  **Location:** `/Users/weiming/Documents/GitHub/poly-book/crates/pb-types/src/event.rs:208`

LatencyTrace::from_optional_timestamps stores the six *_us stages verbatim — exchange_fill_us earlier than order_submit_us, a ms-unit value mixed with µs-unit values, or all-equal values are accepted at append time with no check anywhere in pb-bin, pb-types, or pb-service. No Rust code computes stage deltas (so no u64 wrap/panic today), but the Execution page waterfall computes end - start in JS and for inverted stages renders a 2px bar labelled with a negative duration (formatDurationUs(-500) returns "-500µs"), and the 'Total end-to-end' figure (maxTs - minTs) masks the inversion entirely. Any future Rust-side latency aggregation over these fields would wrap on u64 subtraction. docs/latency.md describes pipeline latency but defines no contract for these persisted stages.

**Recommendation:** Validate at append time that present stages are non-decreasing in pipeline order (recv ≤ normalization ≤ strategy ≤ submit ≤ ack ≤ fill) and within the same plausibility window as event_timestamp_us; reject otherwise. In the waterfall, detect end < start and render an explicit 'inverted trace' warning instead of a negative label.

### A.63 Timeline ordering has no tie-break: equal-timestamp events return in nondeterministic order, and Parquet vs ClickHouse backends diverge on ties and on single-sink appends
- **Severity:** medium  |  **Area:** execution-subsystem  |  **Location:** `/Users/weiming/Documents/GitHub/poly-book/crates/pb-replay/src/reader.rs:1020`

The Parquet reader concatenates events from files in unspecified read_dir order then stable-sorts by event_timestamp_us alone; the ClickHouse query uses ORDER BY event_timestamp_us with no secondary key, so ties are returned in arbitrary (storage-key-influenced) order. Manually journaled events frequently share timestamps (operators enter second-resolution values ×1e6). build_execution_timeline then applies truncate(limit), so when ties straddle the limit boundary the two backends — or two runs of the same backend — return different event sets, breaking the 'deterministic replay' standard and the cross_backend_execution_equivalence guarantee for tied data. Independently, execution-append writes to exactly one sink (--source parquet|clickhouse), while api.historical_backend selects the read backend with auto-fallback: events appended only to Parquet are simply absent when the API serves from ClickHouse, so 'cross-backend parity' does not hold even for untied data unless operators remember to append twice.

**Recommendation:** Sort by (event_timestamp_us, order_id, kind-rank) in both readers (add the same keys to the ClickHouse ORDER BY clause) so output is total-ordered and backend-independent. Either make execution-append dual-write like the ingest pipeline, or document loudly that --source must match api.historical_backend and add a CLI warning when they differ.

### A.64 gRPC ExecutionTimeline bypasses the window and limit guards the HTTP route enforces; both backends buffer the entire window in memory before filtering
- **Severity:** medium  |  **Area:** execution-subsystem  |  **Location:** `/Users/weiming/Documents/GitHub/poly-book/crates/pb-grpc/src/lib.rs:187`

The HTTP route validates start<end, window ≤ 24h (MAX_QUERY_WINDOW_US), and 1 ≤ limit ≤ 1000. The gRPC execution_timeline RPC performs none of these: start_us/end_us are unbounded (against ClickHouse, fetch_all materializes every execution row in the range; limit is applied only after build_execution_timeline buffers everything), and req.limit is passed through unclamped (limit=0 silently returns zero events with a nonzero total_count). Compounding this, the asset_id filter is never pushed down in either backend — ParquetExecutionService reads execution files for all assets in the window and filters in memory — so a wide window over a busy journal is a memory-exhaustion vector on the serving process.

**Recommendation:** Apply the same validate_time_window and limit clamp in the gRPC handler (shared helper in pb-service so guards cannot drift). Push the asset_id predicate into extract_execution_events and the ClickHouse WHERE clause, and consider streaming/early-terminating once limit rows are collected.

### A.65 No server-side pagination: truncation keeps the oldest events, and the web UI hard-codes limit=200, making events beyond the first 200 of a window unreachable
- **Severity:** medium  |  **Area:** execution-subsystem  |  **Location:** `/Users/weiming/Documents/GitHub/poly-book/crates/pb-service/src/lib.rs:163`

build_execution_timeline sorts ascending and truncate(limit) keeps the earliest events of the window. ExecutionQuery exposes no offset/cursor, so a client cannot retrieve events after the cutoff except by guessing narrower windows. The Execution page queries 'last N minutes' with limit hard-coded to 200 and then paginates client-side over only the returned slice; for an active window the user sees 'Total events: 5000' alongside the oldest 200, with the most recent activity — usually what an execution inspector wants — unviewable. EXECUTION_MAX_LIMIT=1000 caps the workaround.

**Recommendation:** Add a cursor (e.g. after_timestamp_us + after_order_id matching the total order from the tie-break fix) or offset parameter to the service trait, HTTP route, and gRPC RPC; add an order=asc|desc option so 'most recent first' is expressible; surface 'results truncated' explicitly in the UI when total_count > events.length.

### A.66 Fill prices and sizes are silently rounded through f64 on the manual append path — journaled execution prices can be wrong by up to half a tick
- **Severity:** medium  |  **Area:** execution-subsystem  |  **Location:** `/Users/weiming/Documents/GitHub/poly-book/crates/pb-types/src/fixed.rs:112`

FixedPrice::try_from(&str) parses to f64 then rounds to the 1e-4 tick: "0.12345" becomes 0.1235 with no error, and FixedSize::from_f64 rounds to 1e-6 and saturates via `as u64`, losing exact integer precision above ~9e9 units. On the feed path this is tolerable (exchange data is tick-aligned), but execution-append is a human/operator entry path feeding fill records that downstream PnL and slippage analysis treat as ground truth: a transcribed average-fill price like 0.55125 is silently coerced, and the stored value no longer matches the venue's record. Nothing in execution_append.rs detects or reports the coercion.

**Recommendation:** For the append path, parse decimal strings exactly (digit-based, no f64) and reject values that are not exact multiples of the tick/size scale ('price 0.12345 is not representable at tick 0.0001'), or at minimum echo the stored canonical value and require --allow-rounding to accept coercion.

### A.67 No WS staleness/heartbeat detection, no recovery from permanent 'fallback', and frozen WS data preferred over fresh HTTP data
- **Severity:** medium  |  **Area:** frontend  |  **Location:** `web/src/shared/hooks/use-orderbook-stream.ts:116`

Three gaps for a trading UI: (1) After MAX_RETRIES=8 the hook enters 'fallback' permanently — no periodic WS retry, no `online` event listener — so a ~50s backend blip downgrades every long-lived session to 1s HTTP polling forever. (2) There is no heartbeat/last-message-age check: a half-open TCP connection keeps status 'connected', and OrderbookPage's merge prefers the frozen wsSnapshot over the fresh 1s HTTP snapshot, so stale prices are displayed under a green 'WebSocket (live)' badge with no client-side staleness indication. (3) Malformed/schema-failing messages are silently swallowed (`catch { // Ignore malformed messages }`) with no counter or log, so server schema drift is invisible. Also `retriesRef.current = 0` on every onopen means an open-then-immediate-close server loop reconnects forever at ~500ms with no circuit breaker.

**Recommendation:** Add a last-message timestamp and a watchdog (e.g. mark stream stale and fall back to the HTTP snapshot if no message for N seconds); periodically attempt WS re-promotion while in 'fallback' (and on window 'online'); reset the retry counter only after a stable-connection period; log/count Zod parse failures on WS messages.

### A.68 WS-to-TanStack-Query cache bridge writes to the wrong query key (bids.length used as depth)
- **Severity:** medium  |  **Area:** frontend  |  **Location:** `web/src/shared/hooks/use-orderbook-stream.ts:78`

Each WS message calls queryClient.setQueryData with key `queryKeys.orderbook(data.asset_id, data.bids.length)`, but the snapshot queries are keyed by the user-selected depth (5/10/25/50/100/200). The number of bid levels in a WS message is not the requested depth, so the 'unified data layer' update is almost always a silent no-op (the updater returns `prev` when no entry exists). When it does coincidentally match (e.g. a 10-level message while depth=10), the cache is notified on every WS message, un-throttled, bypassing the rAF coalescing and re-rendering every subscriber (including LiveFeedPage's AssetQuickView at depth 5 never benefits, OrderbookPage double-renders). The intended cross-page data unification does not work as designed.

**Recommendation:** Either pass the page's selected depth into useOrderBookStream and write to the exact active key (truncating bids/asks to that depth), or use queryClient.setQueriesData with a partial key ['orderbook', assetId] — and throttle the cache write with the same rAF coalescer used for local state.

### A.69 Vite dev proxy lacks ws:true — WebSocket streaming is dead in the default dev workflow
- **Severity:** medium  |  **Area:** frontend  |  **Location:** `web/vite.config.ts:28`

The dev server proxies '/api' to the backend but does not set `ws: true`, so WS upgrade requests to /api/v1/streams/orderbook are not proxied. In the README's documented dev flow (no VITE_API_BASE_URL, proxy to serve-api), the orderbook stream fails 9 connection attempts (~50-85s of retry churn per page visit) and silently lands in HTTP fallback. The primary real-time code path of the trading UI is therefore never exercisable locally or in Playwright e2e against the dev server, which is exactly how the lifecycle bugs above stay hidden.

**Recommendation:** Add `ws: true` to the '/api' proxy entry and add an e2e/manual smoke test that asserts the transport badge reaches 'WebSocket' against a running backend.

### A.70 Demo/API source mode is not part of query keys — cross-mode cache pollution; WS stream and SQL mutation ignore demo mode entirely
- **Severity:** medium  |  **Area:** frontend  |  **Location:** `web/src/shared/api/queries.ts:43`

All query keys (['feed-status'], ['orderbook', assetId, depth], ...) omit sourceMode while queryFn and staleTime switch on it. Switching Live API → Demo leaves API data in the cache under the same keys with demo's staleTime: Infinity, so 'Demo' mode keeps displaying the last live-API responses indefinitely and never fetches fixtures (and vice-versa demo rows can flash into live mode until refetch). Additionally, useOrderBookStream has no demo guard — in demo mode it still dials the real backend WS (mixing live data into demo view if a backend is up, or churning through 9 failed connections if not), and useQuerySql posts to the real /api/v1/query/sql in demo mode even though getDemoQueryResult exists unused.

**Recommendation:** Include sourceMode in every query key (e.g. [sourceMode, 'feed-status']) or call queryClient.clear() on mode switch; gate useOrderBookStream on sourceMode !== 'demo'; route useQuerySql through getDemoQueryResult in demo mode.

### A.71 Route-level ErrorBoundary never resets on navigation — one route error bricks all navigation
- **Severity:** medium  |  **Area:** frontend  |  **Location:** `web/src/app/error-boundary.tsx:29`

The single route-level ErrorBoundary wraps <Routes> (App.tsx:103). Once any page throws, hasError stays true; clicking nav links changes the location but the boundary keeps rendering its fallback for every route, so the entire app appears broken until the user discovers 'Try again'. There is no reset keyed on pathname, and the route fallback also does not reset associated query state.

**Recommendation:** Reset the boundary on navigation — simplest is keying it by location: <ErrorBoundary key={location.pathname}> via a small wrapper using useLocation, or add componentDidUpdate that clears hasError when a resetKey prop changes.

### A.72 Accessibility gaps: command palette lacks dialog semantics/focus trap; table sorting is mouse-only; lazy-route heading focus race
- **Severity:** medium  |  **Area:** frontend  |  **Location:** `web/src/shared/components/command-palette.tsx:35`

Consolidated a11y findings: (1) CommandPalette renders plain fixed divs — no role="dialog", no aria-modal, no focus trap and no background inert, so Tab walks out into the page behind the overlay and screen readers get no dialog announcement (unlike ShortcutHelp, which correctly uses Radix Dialog). (2) DataTable sortable headers are <th onClick=...> with no tabIndex, no keyboard handler and no <button>, so sorting is unreachable by keyboard despite aria-sort being set (data-table.tsx:41-51); SchemaBrowser expand toggles lack aria-expanded (schema-browser.tsx:47). (3) useFocusOnNavigate fires on pathname change, but routes are lazy — on first visit to a route the chunk hasn't rendered, getElementById('page-heading') is null and focus lands on #main-content instead of the heading (use-focus-on-navigate.ts:22).

**Recommendation:** Render CommandPalette inside the existing Radix Dialog primitives (cmdk composes with Dialog officially); make sort headers real <button>s inside <th> with Enter/Space handling; add aria-expanded to SchemaBrowser toggles; in useFocusOnNavigate, retry focus after the Suspense child commits (e.g. MutationObserver/requestAnimationFrame loop or move the focus call into a layout effect in each page).

### A.73 4-second hard timeout applied to the SQL workbench POST will abort legitimate analytic queries
- **Severity:** medium  |  **Area:** frontend  |  **Location:** `web/src/shared/api/client.ts:5`

REQUEST_TIMEOUT_MS = 4_000 is applied uniformly, including postAndValidate for POST /api/v1/query/sql. Ad-hoc SQL over Parquet/ClickHouse split datasets routinely exceeds 4s; the client aborts and surfaces 'Request timed out after 4000ms' while the backend keeps executing, making the Query page unusable for exactly the heavy queries it exists for. A duplicate, unused REQUEST_TIMEOUT_MS also lives in shared/lib/constants.ts:4, inviting drift.

**Recommendation:** Make the timeout a per-call option; keep 4s for 1s-cadence polling endpoints but give /query/sql a much larger budget (30-60s) aligned with the server-side query guard, and delete the dead duplicate constants in constants.ts.

### A.74 No venue-anchored sequence or hash verification — silent WS message loss is undetectable at ingest, and gap detection only runs against self-generated sequences during offline replay
- **Severity:** medium  |  **Area:** hft-gap  |  **Location:** `crates/pb-feed/src/dispatcher.rs:362`

Sequence numbers are fabricated locally by the Dispatcher (next_sequence_for increments a per-asset counter), so by construction they can never gap at ingest: a price_change frame dropped by the venue, the TCP stack, or the process is assigned no number and leaves no trace. Polymarket's book/price_change `hash` field — designed for clients to verify book state — is stored as source_event_id but never validated against the locally maintained book. IngestEventKind::SequenceGap is never emitted by any production ingest path; pb_gaps_detected_total only fires in the replay engine (engine.rs:243-259) when checking the same locally-fabricated sequences, which only catch persistence-layer loss, not feed loss. The gap-fill protocol on reconnect is implicit (venue resends a snapshot on resubscribe) with no REST gap-fill of the missed interval and no quantification of the data hole beyond a SourceReset marker.

**Recommendation:** Maintain the venue book hash locally after each applied delta and compare against the hash field on every message; on mismatch, emit a SequenceGap/BookMismatch ingest event, alert, and force a REST resnapshot. Treat reconnect windows as explicit data holes with start/end timestamps and persist them as queryable integrity records.

### A.75 auto-ingest (the production rotating-market mode) never writes the WAL, breaking the documented ingest→serve topology, and rotation drops the final 10 seconds of each expiring market
- **Severity:** medium  |  **Area:** hft-gap  |  **Location:** `crates/pb-bin/src/commands/auto_ingest.rs:25`

CLAUDE.md and docs/architecture.md define the separated topology as ingest writing the WAL that serve tails. But auto_ingest::run — the mode actually used for the rotating BTC 5-minute markets — constructs no WalWriter at all; events go only to Parquet/ClickHouse sinks. Running `auto-ingest` + `serve` yields a serve process that hydrates and then tails an empty/stale WAL forever with no error. Separately, the rotation logic wakes at `target_bucket - 10` and immediately cancels the old market's WebSocket/dispatcher before subscribing to the new one, so the last ~10 seconds before each market's expiry — precisely the highest-information window for a 5-minute binary market — are never captured, and no ingest event records this systematic hole.

**Recommendation:** Factor the WAL-write + fanout loop out of ingest.rs and reuse it in auto-ingest (or make WAL writing a property of the shared pipeline), and overlap subscriptions during rotation: subscribe to the new market while keeping the old market's feed alive until its expiry plus a grace period.

### A.76 Time discipline is wall-clock only: no monotonic source, replay order can diverge from live apply order, and clock-sync requirements are undocumented
- **Severity:** medium  |  **Area:** hft-gap  |  **Location:** `crates/pb-feed/src/ws.rs:235`

All recv timestamps come from SystemTime::now() (now_us), which is non-monotonic under NTP steps/slews. The live read model applies records in WAL arrival order, while the replay engine re-sorts by recv_timestamp_us (engine.rs:287-299); any backward clock step during capture makes replayed ordering differ from what the live system actually applied, breaking the deterministic-replay guarantee precisely when it matters (around anomalies). pb_ws_latency_us only records when recv > exchange (dispatcher.rs:410-413), so negative skew — the clearest signal of local clock error — is silently discarded instead of alarmed. parse_timestamp_us's ms-vs-us heuristic (dispatcher.rs:423-430) misclassifies seconds-resolution timestamps. Nothing in docs/architecture.md or docs/operations.md states clock-source requirements (chrony/PTP), acceptable skew, or how cross-host ordering works for the multi-host ingest/serve split.

**Recommendation:** Capture a monotonic-derived ingest sequence (single writer already exists — stamp a global monotonically increasing arrival sequence into provenance at the dispatcher) and make replay order by it; record negative skew as a clock-error gauge with an alert; document NTP/PTP requirements and acceptable skew in operations.md.

### A.77 No alerting layer and no live data-quality monitors: gap/staleness/crossed-book conditions are computed but nothing pages, and check_integrity is never run on the live path
- **Severity:** medium  |  **Area:** hft-gap  |  **Location:** `crates/pb-api/src/live_state.rs:258`

Prometheus counters/histograms exist, but the repository contains zero alert rules, dashboards, SLO definitions, or runbooks (no matches for alert/failover/recovery/RTO/runbook in docs/ or infra/). Market-data quality monitors are passive: staleness is computed only when an API client asks (is_stale in live_state.rs:910), L2Book::check_integrity (crossed-book detection) exists but has no caller on the live apply path (record_delta_event applies deltas unconditionally), and the 60-second REST checkpoints are persisted but never compared against the live WS-derived book in real time — cross-source reconciliation exists only as an offline `replay validate` flow. For a market-data system the bar is: crossed book, stale feed, sequence/hash mismatch, WAL lag, and sink failure each page within seconds.

**Recommendation:** Ship a prometheus-rules file with the deploy (feed staleness, pb_gaps_detected_total > 0, WAL lag, sink failure, crossed-book gauge); run check_integrity after each live delta apply and on each REST checkpoint compare top-N levels against the live book, emitting a divergence metric and persisted integrity event.

### A.78 No failover story: single feed, single writer, manual recovery on WAL resync, and explicitly deferred multi-replica support without an RTO statement
- **Severity:** medium  |  **Area:** hft-gap  |  **Location:** `crates/pb-bin/src/commands/serve.rs:189`

Every failure domain is a singleton: one WebSocket connection (no redundant A/B feed with arbitration), one ingest writer, one Fargate task (desired_count=1), one serve replica. When the serve tailer detects pruned segments (needs_resync), it logs a warning, sets an atomic for /health, and breaks out of the tail loop — the process then serves frozen data until a human restarts it; there is no automatic re-hydration despite the message "triggering re-hydration". Similarly, if the WAL writer fails to open, ingest "continues without WAL" on a warn (ingest.rs:81), silently disabling the serve topology. docs/serve-api.md defers "multi-replica WAL fan-out" but no document states recovery time objectives, failover procedure, or how a standby ingest would take over without overlap/gap on the shared WAL directory.

**Recommendation:** Implement in-process re-hydration on needs_resync (loop back into hydrate() instead of break), make WAL-open failure fatal in separated mode, and write a failure-domain document: feed redundancy plan, ingest writer leasing/lock for failover, and measured cold/warm start RTOs (cold start currently scales with full WAL size — see the wal_offset finding).

### A.79 Channel sizing and load-shedding policy are unstated: hard-coded 2048/10000 capacities with whole-pipeline head-of-line blocking as the only flow-control mode
- **Severity:** medium  |  **Area:** hft-gap  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:43`

All pipeline channels are hard-coded (2,048 for raw/event/fanout, 10,000 for sinks) with no documented rationale, no burst modeling (e.g., snapshot fan-out explodes one WS frame into hundreds of BookEvents — a 50-level snapshot for 10 assets at rotation is thousands of channel sends), and no queue-depth/saturation metrics. The backpressure policy is implicit total blocking: if any sink chain stalls, fanout_event awaits, the ingest loop stops draining, the dispatcher blocks, the WS task blocks on tx.send, and TCP backpressure eventually causes the venue to buffer/disconnect — so a slow analytics database can cause feed loss. ADR-0003 documents backpressure as OOM protection but never specifies which consumers are allowed to lag, what gets shed first, or alerting thresholds. A production design should protect the feed+WAL unconditionally and shed or buffer storage consumers explicitly.

**Recommendation:** Document a flow-control policy: WAL write is the only blocking consumer; sinks consume from the WAL (or a spillable queue) and may lag with alerting. Export channel-depth/blocked-send-time gauges, and size capacities from measured rotation-burst rates rather than constants.

### A.80 Change safety gaps: no replay-based regression against captured data, no shadow/canary deployment, :latest image deploys, and untested codec/schema evolution
- **Severity:** medium  |  **Area:** hft-gap  |  **Location:** `tests/integration/book_determinism.rs:1`

The determinism test applies 9 synthetic events twice and compares books — it does not validate that a code change reproduces byte-identical books from real captured WAL/Parquet data, which is the standard regression harness for book-building logic at trading firms (golden captures replayed in CI, diffing against stored reference states). The ReplayValidation machinery exists but runs only on demand via CLI, not as a continuous or CI gate. Deployment safety is similarly thin: the ECS task pins `image = ...:latest` (infra/ecs.tf:33) so deploys are not reproducible or roll-backable by digest, and there is no shadow/canary mode (e.g., run new build against the live feed and diff read models before promotion). Schema evolution has the right primitive (WAL codec version byte) but only version 1 exists, there is no test decoding a frame from an older version, and no documented procedure for evolving Parquet/ClickHouse schemas against years of accumulated data.

**Recommendation:** Check a small golden WAL capture into the repo (or fetch from S3 in CI) and add a CI job that replays it and asserts byte-identical book states and integrity-event counts against a stored reference; pin deploys to image digests with a staged rollout; add cross-version codec fixtures so frame-format evolution is provably backward compatible.

### A.81 Checkpoint WAL resume offset never populated and its virtual-offset arithmetic hardcodes default segment size in hydration
- **Severity:** medium  |  **Area:** numerics  |  **Location:** `crates/pb-api/src/hydration.rs:153`

The serve runtime's documented 'checkpoint hydration → resume WAL tail from wal_offset' flow is broken in two ways. (1) No production code ever stamps BookCheckpoint.wal_offset: checkpoint_from_rest sets it to None (backfill.rs:141) and the ingest loop never attaches WalWriter::global_offset() to checkpoint records before WAL append, so min_wal_offset is always None and hydration replays the entire WAL from the earliest segment on every restart. With WalPruner active, the earliest retained segment can begin with deltas whose preceding snapshot was pruned; those stale deltas are applied on top of a newer checkpoint-initialized book, serving a wrong live book until the next snapshot arrives. (2) The skip arithmetic itself is unit-inconsistent: global offsets are virtual (segment_id * segment_size + offset, writer.rs:65-69) and therefore only comparable for one fixed segment_size, but replay_wal_tail rebuilds WalConfig with `..Default::default()` (64 MB) instead of the configured wal.segment_size_mb, so the moment wal_offset is wired up, any non-default segment size silently mis-computes the skip boundary (skipping live records or double-applying old ones).

**Recommendation:** Stamp wal_offset = writer.global_offset() on checkpoint records at WAL-append time in the ingest loop. Pass the full WalConfig (not just base_path) into hydrate() so both sides use the same segment_size, or better, replace the virtual-offset scheme with the explicit WalPosition {segment_id, offset} pair already used for live handoff. Add an integration test that hydrates with a non-default wal.segment_size_mb and a pruned WAL.

### A.82 FixedSize conversions are unbounded and silently saturating through f64; precision cliff at 2^53
- **Severity:** medium  |  **Area:** numerics  |  **Location:** `crates/pb-types/src/fixed.rs:170`

FixedSize::from_f64 only rejects NaN/inf/negative, then does `(v * 1e6).round() as u64`, a saturating cast: any huge finite input (e.g. wire string "1e30") becomes FixedSize(u64::MAX) and is persisted as if real liquidity. This is asymmetric with FixedPrice, which range-checks against PRICE_SCALE. Additionally every string→FixedSize path (wire deserialization, TryFrom<&str>, WAL/JSON serde which round-trips through decimal strings) goes through f64, so raw values above 2^53 (~9.0e9 units) cannot round-trip exactly; the proptests only cover raw up to 1e11, leaving the cliff untested. Polymarket sizes are far below this today, but a venue glitch or format change would be silently normalized to garbage rather than rejected.

**Recommendation:** Add an explicit maximum (e.g. reject v > 1e12 units, far above any plausible Polymarket size) and return SizeParse on violation. For exactness, parse decimal strings by splitting integer/fraction parts into u64 arithmetic instead of routing through f64; this also makes WAL/checkpoint serde exact for all representable raws.

### A.83 SQL query guard max_rows is client-controlled with no server-side clamp
- **Severity:** medium  |  **Area:** numerics  |  **Location:** `crates/pb-api/src/server.rs:447`

POST /api/v1/query/sql accepts an optional max_rows in the request body and uses it directly as the QueryGuard limit, falling back to config only when absent. There is no upper clamp, so a client can send max_rows = 18446744073709551615 and inject_limit appends `LIMIT 18446744073709551615`, effectively disabling the row guard and allowing unbounded result sets (memory/bandwidth exhaustion on both ClickHouse and the API process). timeout_secs is server-fixed, but a fast full-table scan can still return enormous payloads within the timeout.

**Recommendation:** Clamp the client value: `req.max_rows.unwrap_or(default).min(state.config.query_max_rows)` and reject 0. Consider also enforcing ClickHouse-side `max_result_rows`/`max_result_bytes` settings on the query URL as defense in depth.

### A.84 Infra topology is a single Fargate Spot task and contradicts the documented multi-replica WAL architecture; no ClickHouse, no serve process, no health check, no circuit breaker
- **Severity:** medium  |  **Area:** ops  |  **Location:** `infra/ecs.tf:13`

The only compute is one ECS service (desired_count default 1, variables.tf:39) on FARGATE_SPOT exclusively with no on-demand fallback — Spot reclamation is a routine event and guarantees market-data capture gaps for a system whose purpose is gapless capture. The task definition has no container healthCheck and no deployment_circuit_breaker, container insights is disabled (ecs.tf:6), and the task is sized at 256 CPU/512 MB (variables.tf:26-34) while the Parquet sink buffers 5 minutes of events in memory — OOM risk. The WAL/multi-replica story in CLAUDE.md and docs (ingest + serve sharing WAL segments, consumer positions, backpressure pruning, gRPC surface) has zero infra backing: no EFS/shared volume exists, the WAL writes to ephemeral task storage that vanishes on every Spot reclaim, no `serve` service, no ALB, and no ClickHouse instance is provisioned anywhere despite it being half of the storage architecture. Scaling desired_count to 2 would produce two uncoordinated ingesters double-writing the same S3 prefix.

**Recommendation:** For capture continuity: run two ingest replicas across AZs with mixed FARGATE/FARGATE_SPOT strategy and de-duplicate downstream (seq+asset keyed), or at minimum FARGATE on-demand for the single ingester. Add deployment_circuit_breaker with rollback, a container health check, and right-size memory. Either provision the serve/ClickHouse/EFS pieces or update CLAUDE.md/docs to state the multi-replica WAL coordination is local-only and unproven in the deployed topology.

### A.85 Observability is insufficient for on-call: zero gauges, no feed-staleness/WAL-lag/disk metrics, and no alert rules or dashboards in the repo
- **Severity:** medium  |  **Area:** ops  |  **Location:** `crates/pb-metrics/src/recorder.rs:4`

pb-metrics defines only counters and histograms; `grep -rn "gauge!"` across all crates returns nothing. There is no last-message-age (feed staleness) gauge, no WAL consumer-lag gauge (serve.rs:197 stores lag_bytes into an AtomicU64 surfaced only via the /health JSON body, not Prometheus), no WAL disk/segment-count gauge, no channel-depth/backpressure gauge, no active-asset count, and no error counters for WAL append or sink flush failures. USE coverage is absent and RED coverage exists only for the HTTP API (pb_api_request_duration_ms with status label). The repo contains no Prometheus alerting rules, no Grafana dashboards, and no runbooks — `pb_gaps_detected_total` and `pb_reconnections_total` exist but nothing tells on-call what thresholds page. The ingest process exposes only the metrics port (no /health), so external liveness probing of the ingester is impossible. pb-metrics's own README says new metrics must be documented in a "docs/operations.md metrics section" that does not exist.

**Recommendation:** Add gauges: pb_feed_last_message_age_seconds, pb_wal_lag_bytes, pb_wal_disk_bytes, pb_channel_depth, plus counters for wal_append_failures and sink_flush_failures. Commit a `monitoring/` directory with Prometheus alert rules (feed stale > 30s, gaps rate > 0, reconnect storm, WAL lag > threshold, no parquet flush in 10 min) and a Grafana dashboard JSON, plus a RUNBOOK.md mapping each alert to actions.

### A.86 Prometheus metrics port is exposed to the public internet (0.0.0.0/0 ingress on a public-IP task)
- **Severity:** medium  |  **Area:** ops  |  **Location:** `infra/vpc.tf:53`

The ECS security group allows TCP 9090 from 0.0.0.0/0 and the service assigns a public IP (ecs.tf:89), so the unauthenticated /metrics endpoint — revealing internal operational state, message rates, asset IDs in labels, and a fingerprintable axum server — is reachable by anyone on the internet. There is no auth on the metrics server.

**Recommendation:** Restrict ingress to the scraper's CIDR/security group (or use a private subnet + NAT and pull-through via Prometheus in-VPC / CloudWatch agent sidecar). At minimum parameterize the allowed CIDR in variables.tf instead of hardcoding 0.0.0.0/0.

### A.87 `logging.format` config key is dead: JSON structured logging is impossible despite being documented config
- **Severity:** medium  |  **Area:** ops  |  **Location:** `crates/pb-bin/src/main.rs:166`

config/default.toml:52 ships `format = "pretty"` and docs/operations.md:63-65 documents the `[logging] format` key, but main.rs never reads it — tracing is initialized unconditionally with `fmt().with_env_filter(filter).init()`. There is no JSON output mode, so in the CloudWatch deployment (awslogs driver, logs.tf) operators get unstructured text that cannot be queried by field with Logs Insights, and the documented config knob silently does nothing. For 24/7 operation with on-call, machine-parseable logs are table stakes.

**Recommendation:** Read `logging.format` and branch to `fmt().json()` (tracing-subscriber json feature) vs pretty; default to json when not attached to a TTY. Remove the key from config/docs if not implemented.

### A.88 No data-retention policy anywhere: ClickHouse tables lack TTL, local Parquet/WAL never expire, and the S3 data bucket has force_destroy = true
- **Severity:** medium  |  **Area:** ops  |  **Location:** `crates/pb-store/src/writer.rs:36`

All six ClickHouse tables are created as plain `ENGINE = MergeTree()` with no TTL clause, so a 1-second batch cadence accumulates rows forever. Local Parquet output (default ./data) has no cleanup mechanism and the S3 lifecycle rule only transitions to STANDARD_IA at 30 days — there is no expiration tier (Glacier/expiry) and no documented retention decision. CloudWatch logs default to 7 days (reasonable). Most dangerous: the primary market-data bucket is declared with `force_destroy = true` (s3.tf:3), meaning a `terraform destroy` (or module refactor that replaces the bucket) irreversibly deletes the entire historical dataset with no versioning and no safeguard — for a capture system this is a one-command data-annihilation path.

**Recommendation:** Set force_destroy = false and add `prevent_destroy` lifecycle plus S3 versioning on the data bucket. Add explicit TTLs to ClickHouse DDL (or document indefinite retention as a decision), add a Glacier/expiration tier or documented forever-retention for S3, and a local Parquet/WAL retention story in docs/operations.md.

### A.89 Query-guard fuzz target removed from CI with known unfixed sanitizer edge cases in the SQL workbench guard
- **Severity:** medium  |  **Area:** ops  |  **Location:** `.github/workflows/ci.yml:129`

The CI comment explicitly states the query workbench's SQL sanitizer/inject_limit logic has multiple known edge cases — unclosed quotes, trailing comments, embedded semicolons with quoted content — and instead of fixing the guard, the fuzz target was removed from CI and relegated to optional local runs. This is the validation layer standing between user-supplied SQL (POST /api/v1/query/sql) and ClickHouse. The feature is off by default (query_workbench_enabled = false), which limits exposure, but shipping a guard with documented unfixed parsing defects and disabling the tool that finds them inverts the safety process; the next person enabling the workbench has no signal that the guard is known-weak.

**Recommendation:** Rewrite the query guard (proper tokenizer or use a SQL parser crate), re-enable fuzz_query_guard in CI, and until then document the known weakness prominently in docs/api.md and the pb-api README, and consider enforcing read-only at the ClickHouse user/role level as defense in depth rather than relying on the sanitizer.

### A.90 CI/release gaps: no docker-build gate, no coverage gate, no perf-regression gate vs Criterion baselines, 30s smoke-only fuzz, and releases ship no artifacts or smoke test
- **Severity:** medium  |  **Area:** ops  |  **Location:** `.github/workflows/release.yml:20`

(1) Nothing in CI builds the Dockerfile, so image breakage (see critical finding) merges green. (2) No code-coverage measurement or gate exists (no llvm-cov/tarpaulin job). (3) Criterion benchmarks exist in pb-types/pb-book but are never run in CI and no baseline comparison gates hot-path regressions — a 10x book-update slowdown would merge silently. (4) Fuzzing is a 30-second smoke per target, far too short to be more than a build check; there is no scheduled long-run fuzz job or corpus persistence. (5) release.yml only calls action-gh-release with generated notes — no binary artifacts are built, no docker image is tagged with the release version, and no smoke test (e.g. `poly-book --help`, replay of a golden WAL fixture) validates the released snapshot. docs/releasing.md honestly describes this but the bar is below a production trading-infra release process.

**Recommendation:** Add CI jobs: `docker build` (no push), cargo-llvm-cov with a ratchet threshold, `cargo bench` with criterion baseline comparison (e.g. critcmp or bencher) failing on >X% regression, and a nightly scheduled 1h+ fuzz run with cached corpus. Extend release.yml to build release binaries + the docker image tagged vX.Y.Z and run a smoke test before publishing.

### A.91 SQL workbench row cap is client-controlled and bypassable; unbounded response buffered in memory
- **Severity:** medium  |  **Area:** pb-api  |  **Location:** `crates/pb-api/src/server.rs:447`

The /api/v1/query/sql handler builds the QueryGuard from the client-supplied max_rows without clamping it to the configured api.query_max_rows: `max_rows: req.max_rows.unwrap_or(state.config.query_max_rows)`. A client can POST max_rows=18446744073709551615 and the injected LIMIT becomes that value. Independently, pb-service's inject_limit (crates/pb-service/src/query.rs:243) skips injection entirely if the user SQL already contains any LIMIT token, so `SELECT * FROM book_events LIMIT 999999999` bypasses the cap with no max_rows at all. Worse, the query timeout only wraps the HTTP send() (query.rs:327-333); the subsequent resp.json() that buffers the entire ClickHouse result into server memory is outside the timeout and has no byte cap. The configured row/time guard is therefore advisory; one request can pull an arbitrarily large result set into the API process (the same process serving the live read model), risking OOM.

**Recommendation:** Clamp: `req.max_rows.map(|n| n.min(state.config.query_max_rows)).unwrap_or(state.config.query_max_rows)` and reject 0. In pb-service, always enforce the cap (e.g. wrap user SQL as `SELECT * FROM (<sql>) LIMIT {max_rows}` or send ClickHouse settings `max_result_rows`/`max_result_bytes`/`max_execution_time` as URL params), extend the timeout to cover body download, and cap response bytes.

### A.92 Historical routes have no per-request timeout, concurrency limit, or response-size cap; 24h window is fully buffered in RAM
- **Severity:** medium  |  **Area:** pb-api  |  **Location:** `crates/pb-api/src/server.rs:290`

validate_time_window caps integrity/execution windows at 24 hours, but for a busy asset the Parquet backend loads every book/trade/ingest event in the window into Vecs in memory (pb-replay reader.rs read_market_data -> read_parquet_files -> rows.extend per batch) before summarizing. The router installs no tower timeout layer, no concurrency limit, and no response-size cap; integrity_summary additionally maps every ingest event in the window into the JSON `continuity_events` array with no truncation (pb-service/src/lib.rs:124-128), so a flapping-feed day yields a multi-MB response built entirely in memory. A handful of concurrent 24h requests (unauthenticated) can exhaust memory or starve the runtime serving the live read path. replay_reconstruct's `at_us` is bounded only by the engine's internal lookback.

**Recommendation:** Add tower-http TimeoutLayer (e.g. 30s) and a global ConcurrencyLimitLayer to the router; cap continuity_events (e.g. most recent 1,000 with a truncated flag); consider lowering MAX_QUERY_WINDOW_US for the Parquet backend or streaming/aggregating per file instead of buffering all events.

### A.93 No auth, default bind 0.0.0.0:3000, trust boundary not stated in API docs
- **Severity:** medium  |  **Area:** pb-api  |  **Location:** `config/default.toml:35`

The API has no authentication and config/default.toml binds it (and gRPC, metrics) to 0.0.0.0. Anyone with network reach can read all market data, stream books, and — if the workbench is enabled — execute arbitrary read-only SQL against ClickHouse (data exfiltration, plus the resource-exhaustion vectors above). docs/serve-api.md lists 'authentication and authorization' among capabilities the system does not own, which justifies read-only, but neither docs/api.md nor docs/operations.md states the operational trust boundary ("deploy only on loopback/trusted network behind a reverse proxy"). The query workbench is off by default, which mitigates the worst exposure. No CORS layer exists, which incidentally blocks browser-based cross-origin reads but does nothing against non-browser clients.

**Recommendation:** Change the default listen_addr to 127.0.0.1:3000 (same for gRPC/metrics), and add an explicit trust-boundary section to docs/api.md and docs/operations.md: unauthenticated, must sit on a trusted network or behind an authenticating proxy; query workbench must never be exposed publicly.

### A.94 WebSocket fan-out is unbounded: no connection cap, no idle timeout/heartbeat, default 64MB client message limit
- **Severity:** medium  |  **Area:** pb-api  |  **Location:** `crates/pb-api/src/streaming.rs:96`

ws_orderbook accepts unlimited concurrent upgrades — there is no per-IP or global WS connection cap, so each connection spawns a task and a broadcast receiver (each lagging receiver triggers a full snapshot rebuild send). The server never sends pings, so a half-open TCP peer subscribed to a quiet asset leaks its session indefinitely (no traffic ever errors the socket). The WebSocketUpgrade is used with defaults, so a client may send messages up to the default tungstenite limits (~64MB message) which the server buffers in socket.recv() only to discard at `Some(Ok(_)) => {}`. Also, if the initial snapshot is not ready the handler silently sends nothing (streaming.rs:169-171), leaving clients unable to distinguish a dead stream from a quiet one.

**Recommendation:** Set `ws.max_message_size(64 * 1024)` (or smaller) on the upgrade, track active WS session count with a cap (reject with 503 over the limit), and add a server-side ping on a tokio::time::interval (e.g. 30s) that closes the session after N missed pongs. Send an explicit status/initializing message when the initial snapshot is unavailable.

### A.95 Internal error details returned verbatim to clients; 500s never logged server-side; inconsistent 400 shape for malformed params
- **Severity:** medium  |  **Area:** pb-api  |  **Location:** `crates/pb-api/src/error.rs:39`

ApiError::into_response serializes the error message into the response body for all variants, including Internal. ServiceError::Internal carries raw upstream errors: reqwest failures embed the ClickHouse URL (`ClickHouse request failed: {e}`), ClickHouse HTTP error bodies are forwarded wholesale (query.rs:337), and parquet/arrow/io errors from pb-replay map straight through (pb-service/src/lib.rs:38 `other => ServiceError::Internal(other.to_string())`), leaking infrastructure topology and storage details to unauthenticated clients. Nothing in into_response emits a tracing event, so 500s are invisible in logs (only the status-code metric survives). Separately, malformed query params rejected by axum's Query extractor return text/plain bodies, not the documented {"error": ...} JSON shape.

**Recommendation:** In into_response, log Internal (and ServiceUnavailable) with tracing::error! including the full message, and return a generic body ("internal error") to clients — keep detailed messages only for BadRequest/NotFound. For the query workbench, sanitize ClickHouse errors to the exception line. Add a custom Query rejection mapper so 400s share the JSON error shape.

### A.96 /health returns HTTP 200 when not ready, breaking status-code-based readiness probes
- **Severity:** medium  |  **Area:** pb-api  |  **Location:** `crates/pb-api/src/server.rs:157`

The health handler always returns 200 OK with a JSON body containing ready/hydrated/needs_resync flags. docs/operations.md describes this endpoint as serving 'liveness and readiness checks', but any standard load balancer, Kubernetes probe, or uptime check keys off the status code and will route traffic to an unhydrated replica or one whose WAL reader detected a segment gap (needs_resync=true) — exactly the multi-replica WAL-coordination scenario the design supports. Tests explicitly pin this behavior (health_reports_not_ready_when_resync_needed asserts StatusCode::OK).

**Recommendation:** Split into /health/live (always 200) and /health/ready returning 503 when !ready, or return 503 with the same JSON body from /health when ready=false. Update docs/api.md, docs/operations.md, and the web client accordingly.

### A.97 Ingest drops queued events on graceful shutdown before WAL/sink write
- **Severity:** medium  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:120`

The ingest hot loop uses `biased; _ = shutdown.cancelled() => break` and exits immediately on SIGINT/SIGTERM. Up to 2,048 already-normalized events buffered in `event_rx` (plus up to 2,048 raw WS frames in the dispatcher's input channel, since pb-feed's Dispatcher also returns immediately on cancel without draining, dispatcher.rs:74-77) are silently dropped — never appended to the WAL and never fanned out to Parquet/ClickHouse. These are live venue messages that cannot be re-fetched, so every routine restart/deploy loses in-flight market data. Ironically, auto_ingest.rs:66-80 gets this right by draining the event channel to closure after cancelling the feed.

**Recommendation:** On shutdown, cancel only the WS client, let the dispatcher drain raw_rx and close event_tx, then continue consuming event_rx until it returns None (writing each record to WAL and fanning out) before flushing and exiting — mirror the auto_ingest fan-out drain pattern. Make pb-feed's Dispatcher drain its input channel on cancellation as well.

### A.98 auto-ingest (the continuous production mode) never writes the WAL
- **Severity:** medium  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/commands/auto_ingest.rs:66`

The architecture (CLAUDE.md, docs/serve-api.md:106 "responsibility of the ingest or auto-ingest processes") defines the ingest process as feed → WAL + sinks, and `serve` depends entirely on tailing that WAL. But auto_ingest.rs contains no WalWriter at all — its fan-out task routes events only to the Parquet/ClickHouse sink channels. Consequences: (a) the rotating BTC-5m production mode has durability only via the 5-minute Parquet flush, so a crash loses up to 5 minutes of data; (b) the separated `serve` process cannot be paired with auto-ingest because there is nothing to hydrate-tail from, silently breaking the documented multi-process topology in the primary operating mode.

**Recommendation:** Add the same WAL-append-before-fanout step that ingest.rs has (lines 132-148) to auto_ingest's fan-out loop, or extract a shared event-loop helper in pipeline.rs so ingest and auto-ingest cannot diverge on durability semantics.

### A.99 No task supervision: component panics/failures end in exit code 0
- **Severity:** medium  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:49`

Every spawned component task (WS client ingest.rs:49-53, dispatcher ingest.rs:57-61, sinks pipeline.rs:72-76/98-102, metrics server pipeline.rs:27-33, serve-api fixed runtime serve_api.rs:181-193) only logs its error; JoinHandles are dropped or only awaited at shutdown. Failure cascades terminate cleanly: a ParquetSink panic drops its receiver → forwarder send fails → fanout_event returns false → ingest breaks its loop, logs "graceful shutdown complete", and main returns Ok(()) — exit code 0 while the venue feed was healthy. A WS death similarly ends ingest with exit 0. Orchestrators (systemd Restart=on-failure, k8s) will not restart a process that exits 0, so ingestion silently stops. Additionally `shutdown_handles` (pipeline.rs:334-341) only checks for timeout, swallowing JoinError so task panics are never surfaced; and in serve_api fixed mode a WsClient construction failure leaves the API serving empty/stale data indefinitely. Separately, auto_ingest.rs:172 uses `?` on WsClient::new inside the rotation loop, aborting the process without awaiting sink flush.

**Recommendation:** Supervise component JoinHandles: select over them in the main loop (or use a JoinSet) and treat unexpected task exit/panic as fatal — cancel the token, run the drain/flush path, and return a non-zero exit. In shutdown_handles, inspect the Result for JoinError and propagate panics.

### A.100 serve WAL tailer has no recovery path; dead-open leaves /health reporting ready
- **Severity:** medium  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/commands/serve.rs:170`

If WalReader::open/open_at fails at tailer startup (e.g., serve started before ingest has created the WAL directory, or WAL wiped), the tailer logs one warn and returns — the task is dead for the process lifetime, the read model never receives live updates, and /health still reports ready=true with wal_lag_bytes=0 and needs_resync=false (server.rs:157-165), so orchestration sees a healthy replica serving frozen data. Similarly, on segment-gap detection the code logs "WAL segment gap detected, triggering re-hydration" (serve.rs:190) and sets needs_resync, but no re-hydration occurs anywhere — the task simply exits and only an external restart fixes it; the log message is misleading.

**Recommendation:** Retry reader open in a loop with backoff (the WAL dir appearing later is a normal startup ordering), and on needs_resync actually re-run hydrate() + reopen the tailer in-process instead of exiting. At minimum, set a health flag when the tailer task terminates for any reason so ready=false.

### A.101 Checkpoint wal_offset never populated — serve re-replays entire WAL every restart
- **Severity:** medium  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/commands/checkpoint_producer.rs:64`

The design (docs/operations.md:268) says serve "hydrates from the latest BookCheckpoint, replays WAL records from that offset". But the only checkpoint producer wired in pb-bin builds checkpoints from REST via checkpoint_from_rest, which hardcodes wal_offset: None (pb-replay/src/backfill.rs:141), and nothing ever stamps a WAL offset even though the ingest loop owns the WalWriter (which exposes global_offset() expressly "for checkpoint coordination"). So hydration's min_wal_offset is always None and replay_wal_tail re-reads the entire retained WAL from the earliest segment on every serve restart; combined with the unwired pruner this restart cost grows without bound. Additionally serve.rs:61-67 passes only `&wal_config.base_path` into hydrate(), so hydration internally rebuilds a WalConfig with `..Default::default()` (hydration.rs:128-131) and its offset-skip math `seg_id * config.segment_size + seg_offset` would be wrong whenever wal.segment_size_mb != 64 — currently latent only because wal_offset is always None.

**Recommendation:** In the ingest event loop, stamp Checkpoint records with wal_writer.global_offset() at append time (the writer is right there). Pass the full WalConfig into hydrate() so segment-size math matches the deployed configuration.

### A.102 Fail-silent config layer: missing file, parse errors, and negative values all swallowed
- **Severity:** medium  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/main.rs:146`

Multiple compounding issues make misconfiguration undetectable: (1) `config::File::with_name(&cli.config).required(false)` means an explicitly passed `--config /etc/pb/prod.toml` that is missing or typoed is silently ignored and the process runs on built-in defaults; (2) every single settings read across pipeline.rs/serve.rs/ingest.rs uses `.unwrap_or(default)`, which swallows not just missing keys but type/parse errors — `PB__WAL__SEGMENT_SIZE_MB=64x` silently becomes 64MB default; (3) negative values wrap via `as u64`/`as usize` casts (e.g., pipeline.rs:155 `get_int("wal.segment_size_mb").unwrap_or(64) as u64`, parquet_flush_interval_secs=-1 becomes ~1.8e19 seconds = never flush); (4) config/default.toml documents keys that no code reads — storage.clickhouse_batch_interval_secs, storage.clickhouse_batch_size, storage.parquet_row_group_size (grep: zero references), so operators tuning them get no effect; (5) `--log-level info` passed explicitly cannot override a config-file logging.level because main.rs:154 treats the default value as "not specified".

**Recommendation:** Make the config file required when --config is explicitly provided (detect via clap's value source). Deserialize the whole config into a typed struct with serde once at startup and fail fast on any parse/type/range error; validate non-negative values. Wire or delete the dead default.toml keys.

### A.103 Boolean CLI toggles cannot be disabled; documented two-process recipe collides on metrics port
- **Severity:** medium  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/main.rs:49`

`#[arg(long, default_value_t = true)]` on bool fields produces clap SetTrue flags that default to true — verified empirically: `--parquet=false` → "error: unexpected value 'false'" and `--parquet false` → "unexpected argument". So `--parquet` and `--metrics` (on ingest, auto-ingest, serve-api, serve) are permanently true: it is impossible to run ClickHouse-only ingestion, and impossible to disable the metrics server from the CLI (and there is no config key for enabling/disabling metrics). Concrete consequence: the documented separated mode (docs/operations.md:260-265, terminal 1 `ingest`, terminal 2 `serve`) has both processes bind metrics 0.0.0.0:9090; start_metrics_server's bind error propagates via `?` (pipeline.rs:24, serve.rs:33) so the second process aborts at startup unless the operator knows to override PB__METRICS__LISTEN_ADDR.

**Recommendation:** Use `#[arg(long, action = ArgAction::Set, default_value_t = true, num_args = 1)]` (accepting --parquet true|false) or paired --no-parquet/--no-metrics flags. Make metrics.listen_addr distinct per documented process pairing, or have each command default to different ports / document the override in operations.md.

### A.104 Shutdown abandons slow sink flushes after 10s and ignores further signals
- **Severity:** medium  |  **Area:** pb-bin  |  **Location:** `crates/pb-bin/src/commands/pipeline.rs:334`

shutdown_handles waits at most 10s per JoinHandle and then merely warns and moves on; when run() subsequently returns, the process exits, killing any sink task mid-flush. A final ClickHouse insert against a slow/unreachable server, or a large Parquet flush (up to 5 minutes of buffered records), that exceeds 10s is truncated — buffered data acknowledged into the sink channel is lost at shutdown despite the "graceful shutdown complete" log. Separately, the signal task (main.rs:174-206) fires once and exits; after the first SIGINT/SIGTERM, tokio's installed handler keeps swallowing signals with no listener, so a second Ctrl+C cannot force-exit a wedged shutdown — only SIGKILL works.

**Recommendation:** Make the shutdown timeout configurable and significantly larger for sink flush (or wait for sinks without timeout while a watchdog logs progress), and treat a timed-out sink flush as a non-zero-exit error. Re-arm the signal listener after first delivery so a second signal triggers immediate std::process::exit.

### A.105 check_integrity (crossed-book detection) is never called on any production path; crossed books are stored and served silently
- **Severity:** medium  |  **Area:** pb-book  |  **Location:** `crates/pb-book/src/book.rs:175`

grep across the workspace shows zero callers of check_integrity outside pb-book's own tests: the live read model publishes snapshots without it, the replay engine surfaces only SequenceGap continuity events (engine.rs:243-260), and pb-service's integrity surface has no crossed-book concept (no 'crossed' references in pb-service/src). Since apply_delta updates one (side, price) level at a time, a book can become crossed — transiently from paired bid/ask updates, or persistently from a missed removal delta — and will be served via /orderbooks/{id}/snapshot, WS broadcasts, and replay reconstruction with no flag, metric, or continuity event. The crate README ('check_integrity detects crossed books') and CLAUDE.md imply this check is part of the integrity story, but it is dead code in the running system, so a persistent crossed book (a strong signal of missed deltas, i.e. silent data loss) goes completely undetected.

**Recommendation:** Call check_integrity at replay target time and append a CrossedBook continuity event to the reconstruction result; in the live read model, check after applying each record batch (or on snapshot publication) and emit a Prometheus counter plus ContinuityWarning, with a debounce to tolerate the transient cross between paired bid/ask deltas.

### A.106 Reconnect attempt counter never resets after a successful session, degrading to a permanent 30s reconnect delay
- **Severity:** medium  |  **Area:** pb-feed  |  **Location:** `crates/pb-feed/src/ws.rs:88`

`attempt` is declared once outside the reconnect loop and only ever incremented (ws.rs:134), never reset after a connection succeeds or after a stable session. With base 100ms, exp exceeds the 30s cap from attempt 9 onward. A long-running `ingest`/`serve-api` process that accumulates 9+ disconnects over its lifetime (including graceful venue-side closes, which take the same path) will wait the full 30 s before every subsequent reconnect, even after days-long stable sessions. Each such reconnect is a guaranteed ~30 s market-data gap that the venue snapshot cannot backfill. Note auto-ingest masks this by building a fresh WsClient per 5-minute market, but single-feed modes do not.

**Recommendation:** Reset `attempt` to 0 after a session that survived past a threshold (e.g., connection held > 30-60 s, or after the first message is received). Standard pattern: record connect time in connect_and_listen and zero the counter when the session duration exceeds the max backoff.

### A.107 No feed-liveness watchdog: pongs are ignored and there is no read-idle timeout, so a half-open connection stalls the feed silently
- **Severity:** medium  |  **Area:** pb-feed  |  **Location:** `crates/pb-feed/src/ws.rs:186`

The client sends a WS ping every 10 s but never verifies a pong (or any traffic) comes back — `Message::Pong` is only debug-logged. If the TCP path is black-holed (NAT/middlebox drop, venue host dies without RST), `stream.next()` pends forever while tiny ping frames keep succeeding into the kernel send buffer; the connection only errors after TCP retransmission gives up (~15+ minutes on default Linux tuning). During that window the process looks healthy, emits no lifecycle events, and serves/persists stale books. Relatedly, there is no check that the subscription produced an initial `book` within a deadline — a silently-rejected subscribe (e.g., bad asset ids) leaves an idle connection kept alive by pings forever. Also, ReconnectSuccess is emitted before the subscribe frame is even sent (ws.rs:149-156).

**Recommendation:** Track last-received-frame (or last-pong) time and add a select! arm that aborts the session with an error if no traffic arrives within N seconds (e.g., 2-3 ping intervals). Additionally require the first `book`/message within a post-subscribe deadline, and emit ReconnectSuccess only after the subscribe frame is sent.

### A.108 Mid-message conversion failure emits a partial snapshot/delta batch and poisons the stale-snapshot tracker
- **Severity:** medium  |  **Area:** pb-feed  |  **Location:** `crates/pb-feed/src/dispatcher.rs:217`

In the Book arm, each level is converted and sent one at a time with `?` on `make_book_event` (dispatcher.rs:207-233). If level k fails FixedPrice/FixedSize conversion (non-numeric, price > 1.0), levels 0..k have already been sent: downstream receives a truncated snapshot (possibly bids only, asks entirely missing) that is indistinguishable from a complete one, silently corrupting reconstructed book state. Worse, `last_snapshot_ts` was already advanced before parsing (dispatcher.rs:200-202), so a venue retransmit of the same snapshot timestamp is then rejected by the `<=` stale check — the corruption persists until a strictly newer snapshot. The PriceChange arm has the same partial-batch abort (`?` at dispatcher.rs:256 inside the entry loop), and inconsistently handles bad sides with `continue` but bad prices/sizes with abort.

**Recommendation:** Make message emission atomic: convert all levels of a book/price_change message into a Vec<BookEvent> first, and only then update last_snapshot_ts, reset the sequence counter, and send. On conversion failure, emit an IngestEvent (e.g., a parse-failure/SourceReset-style record) for the asset instead of a partial snapshot.

### A.109 No venue continuity validation: book hash ignored, sequences are locally synthesized, so SequenceGap can never fire at ingest
- **Severity:** medium  |  **Area:** pb-feed  |  **Location:** `crates/pb-feed/src/dispatcher.rs:362`

Per-asset sequence numbers are generated by the dispatcher itself (`next_sequence_for`), so they are gap-free by construction; the `IngestEventKind::SequenceGap` branch and `record_gap_detected()` in `send()` (dispatcher.rs:384-386) are dead code at this layer, and downstream `check_sequence` only detects loss inside the local pipeline, not venue-side loss. The venue provides exactly the tools to detect drift — `book.hash` (orderbook state hash, captured only as source_event_id), `price_change` per-entry `hash`, and `best_bid`/`best_ask` (parsed in wire.rs:52-54 but never read) — yet none are validated. A message dropped by the venue or a delta misapplied mid-session produces an undetectably wrong book until the next trade-triggered snapshot, violating the deterministic-replay/integrity goal.

**Recommendation:** Cross-check `best_bid`/`best_ask` from price_change entries against the maintained book (cheap), and/or implement Polymarket's documented book-hash computation to verify state after each snapshot/delta; on mismatch emit a SequenceGap/SourceReset IngestEvent and trigger a REST snapshot resync for that asset.

### A.110 Unparseable and unknown WS messages are silently dropped at debug level with no metric
- **Severity:** medium  |  **Area:** pb-feed  |  **Location:** `crates/pb-feed/src/dispatcher.rs:156`

Any frame that fails to deserialize into `WsMessage` — corrupt JSON, an unknown `event_type` (e.g., premium V2 `best_bid_ask`/`new_market`/`market_resolved`), a future wire-format change, or array-framed batches (`[{...},{...}]`, which the test suite explicitly treats as garbage at dispatcher.rs:758-769 and which some Polymarket client libraries do receive) — returns Ok(()) with only a debug! log. No Prometheus counter increments and no IngestEvent is persisted. A venue format drift (the exact scenario the V2 cutover already posed) would silently zero out ingestion while the process reports healthy.

**Recommendation:** Add a `pb_messages_received_total{event_type="unparsed"}` (or dedicated parse-failure) counter with a warn-level sampled log including a payload prefix, and attempt a fallback parse as `Vec<WsMessage>` to tolerate array framing. Alert on a sustained nonzero unparsed rate.

### A.111 RestClient has no HTTP timeouts; a hung venue request stalls discovery/backfill indefinitely
- **Severity:** medium  |  **Area:** pb-feed  |  **Location:** `crates/pb-feed/src/rest.rs:32`

`Client::new()` creates a reqwest client with no total-request or connect timeout. `fetch_book`, `discover_markets`, `discover_by_slug`, and `get_clob_market_info` are awaited inline from auto-ingest market rotation and replay backfill loops; a black-holed connection or stalled response body hangs the calling loop forever (auto-ingest rotation would stop subscribing to new 5-minute markets — a total ingestion stall, not just a backfill stall). There is also no retry with backoff for transient 5xx/429 at this layer; FeedError::RateLimited is surfaced but the governor quota is never adapted to observed 429s.

**Recommendation:** Build the client with `Client::builder().connect_timeout(Duration::from_secs(5)).timeout(Duration::from_secs(15))` (config-driven), and add bounded retry with backoff for 429/5xx in callers or a thin retry wrapper.

### A.112 start_grpc_server logs "gRPC server bound" before binding and swallows bind failures; serve continues silently without gRPC
- **Severity:** medium  |  **Area:** pb-grpc-metrics  |  **Location:** `crates/pb-grpc/src/lib.rs:258`

The info log claims the server is bound, but serve_with_shutdown(addr, ...) performs the actual bind inside the spawned task. If port 50051 is already taken, the error is only logged at error level inside the task, start_grpc_server has already returned Ok, and the serve/serve-api process keeps running with gRPC silently absent. This contradicts the project's own pattern (pb-bin pipeline.rs:24 binds the metrics TcpListener eagerly precisely 'to catch bind errors early', and serve.rs:136 does the same for the API).

**Recommendation:** Bind a TcpListener (or TcpIncoming) eagerly in start_grpc_server and use serve_with_incoming_shutdown, returning bind errors to the caller so `serve` fails fast; move the "bound" log after the successful bind.

### A.113 WAL durability path is completely uninstrumented; no fsync latency, channel-depth gauges, drop counters, or end-to-end ingest latency
- **Severity:** medium  |  **Area:** pb-grpc-metrics  |  **Location:** `crates/pb-metrics/src/recorder.rs:4`

For a system whose stated bar is zero data loss with measured latency, the most important signals are missing: (1) crates/pb-wal has zero pb_metrics references — no append latency, no sync_data/fsync duration histogram (segment.rs:118 calls sync_data with no timing), no segment rotation counter, no WAL size gauge; (2) WAL consumer lag exists only as an AtomicU64 surfaced through the HTTP feed-status route (pb-bin/serve.rs:196-197), not as a Prometheus gauge, so it cannot be alerted on; (3) the bounded mpsc channels (ws→dispatcher, dispatcher→sinks at 10,000 capacity) have no depth/occupancy gauges, so backpressure onset is invisible until the feed stalls; (4) WS broadcast subscriber lag is logged (pb-api/streaming.rs:188-189) but never counted; (5) there is no end-to-end latency metric (frame recv → WAL durable, or recv → sink flush) — pb_ws_latency_us measures exchange→recv network delta and pb_message_processing_duration_us covers only Dispatcher::dispatch (dispatcher.rs:81-88), leaving WAL append, fsync, and queueing time unmeasured.

**Recommendation:** Add: pb_wal_append_duration_us and pb_wal_sync_duration_us histograms, pb_wal_segment_rotations_total, pb_wal_active_segment_bytes and pb_wal_consumer_lag_bytes gauges, pb_channel_depth{channel=...} gauges sampled periodically, pb_ws_broadcast_lagged_total counter, and an end-to-end pb_ingest_to_durable_us histogram (recv_timestamp_us → post-fsync). Update docs/operations.md per the README's propagation table.

### A.114 Latency 'histograms' are actually 60-second rolling summaries: no buckets configured, quantiles non-aggregatable and low-frequency metrics expire before scrape
- **Severity:** medium  |  **Area:** pb-grpc-metrics  |  **Location:** `crates/pb-metrics/src/server.rs:9`

install_recorder() uses PrometheusBuilder::new() with no set_buckets/set_buckets_for_metric. In metrics-exporter-prometheus 0.18, histogram! metrics are then rendered as Prometheus summaries backed by a RollingSummary with defaults of 3 buckets × 20s = a 60-second quantile window (distribution.rs DEFAULT_SUMMARY_BUCKET_COUNT=3, DEFAULT_SUMMARY_BUCKET_DURATION=20s). Consequences: (a) quantiles cannot be aggregated across the separate ingest and serve processes (histogram_quantile is unavailable); (b) pb_storage_flush_duration_ms is recorded once per 5 minutes for Parquet, so its samples expire from the 60s window long before/between scrapes — p50/p99 will read NaN or reflect a single sample; (c) the histogram-bucket question the latency budget in docs/latency.md implies (µs-scale processing vs ms-scale flush) is moot because no buckets exist at all.

**Recommendation:** Call set_buckets_for_metric with explicit bucket edges per metric family (e.g. µs-scale exponential buckets for pb_message_processing_duration_us/pb_ws_latency_us, ms-scale for pb_storage_flush_duration_ms/pb_api_request_duration_ms) so true Prometheus histograms are exported; alternatively lengthen the summary window deliberately and document the tradeoff.

### A.115 No run_upkeep task for the Prometheus recorder: memory growth bounded only by scrape cadence
- **Severity:** medium  |  **Area:** pb-grpc-metrics  |  **Location:** `crates/pb-bin/src/commands/pipeline.rs:20`

metrics-exporter-prometheus explicitly documents that when using install_recorder() (rather than install()), no upkeep task is spawned and 'users are responsible for ... calling run_upkeep at a regular interval' to drain histogram sample buckets that 'can grow over time and consume a large amount of memory'. pipeline::start_metrics_server installs the recorder and serves the endpoint but never calls handle.run_upkeep(). If Prometheus stops scraping (misconfigured target, network partition, or a deployment where nothing scrapes the box), every histogram! sample in the always-on ingest process accumulates indefinitely — slow memory growth in the exact process that must never die.

**Recommendation:** In start_metrics_server, clone the PrometheusHandle and spawn a tokio interval task (e.g. every 5s) calling handle.run_upkeep(); alternatively switch to PrometheusBuilder::build() which returns an exporter future with upkeep included.

### A.116 Replay sort key (recv_ts, sequence) reorders pre-snapshot deltas after the snapshot and is nondeterministic on ties
- **Severity:** medium  |  **Area:** pb-replay  |  **Location:** `crates/pb-replay/src/engine.rs:287`

The dispatcher assigns sequences from a per-asset counter that is reset to 0 on every snapshot (dispatcher.rs:205, next_sequence_for at 362-371). Replay sorts by (recv_timestamp_us, sequence). If a delta (carrying a large pre-reset sequence, e.g. 4711) and the subsequent snapshot (sequences 0..N-1) are received in the same microsecond — realistic during TCP buffer drains after a stall — the sort places the snapshot before the older delta. Replay then applies the stale delta on top of the fresh snapshot, while live (WAL/wire order) applies delta-then-snapshot. The replayed book diverges from live and a spurious SequenceGap continuity event is emitted. Separately, ParquetReader merges per-file results via buffer_unordered(8) (reader.rs:178-186), so concatenation order varies run-to-run; the subsequent stable sort leaves any (recv_ts, sequence) ties in nondeterministic relative order — replay is not reproducible byte-for-byte across invocations. No persisted total-order tiebreaker (WAL offset, ingest ordinal) exists in the row schema.

**Recommendation:** Persist a non-resetting per-asset (or global) monotonic ingest ordinal — or the WAL offset — on every BookEvent and use it as the authoritative replay sort key. Short term: treat a sequence decrease at equal recv_ts as a snapshot boundary and order the snapshot last, and replace buffer_unordered with ordered `buffered()` over path-sorted files so tie order is at least deterministic.

### A.117 Checkpoint boundary mixes exchange-clock and recv-clock domains, silently skipping or double-applying deltas
- **Severity:** medium  |  **Area:** pb-replay  |  **Location:** `crates/pb-replay/src/engine.rs:181`

All production checkpoints are REST-derived (backfill.rs / pb-bin checkpoint_producer) with checkpoint_timestamp_us = the exchange-reported ms timestamp (backfill.rs:105,130), while provenance.recv_timestamp_us is the local fetch time. reconstruct_at uses checkpoint_timestamp_us both as the floor of the recv-domain market-data query (engine.rs:50-56; both readers filter on recv_timestamp_us) and as the strict-`>` skip threshold against event_ordering_ts, which in RecvTime mode is the local recv clock. Comparing an exchange-clock value to local recv timestamps means: with exchange clock ahead of local clock (NTP skew), deltas received after the snapshot but recv-stamped <= checkpoint_timestamp_us are permanently skipped — the replayed book misses level updates until the level is next touched, diverging from live; with exchange clock behind, deltas already incorporated in the snapshot are re-applied (benign only because deltas are absolute level sets). REST checkpoints also carry no sequence, and apply_checkpoint seeds Sequence::default() (engine.rs:331), which disables gap detection until the first post-checkpoint delta, so a missed delta in this overlap window is undetectable.

**Recommendation:** Use a single time domain per mode: in RecvTime mode, window and skip from checkpoint.provenance.recv_timestamp_us minus a small overlap margin (re-applying overlapping absolute deltas is idempotent and safe); in ExchangeTime mode keep checkpoint_timestamp_us. Document the residual REST-snapshot/WS-delta alignment ambiguity, and prefer erring toward re-application over skipping.

### A.118 ParquetReader::read_latest_checkpoint expands its scan to the epoch with full rescans when no checkpoint exists
- **Severity:** medium  |  **Area:** pb-replay  |  **Location:** `crates/pb-replay/src/reader.rs:962`

The lookback window starts at 6h and doubles until start_us saturates to 0, re-running latest_checkpoint_in_range over the entire [start, at_us] range each iteration (not just the newly extended portion). For an asset with no checkpoints (backfill disabled, new asset, or persistent REST failures), the terminal iterations enumerate every hour directory since 1970 — roughly 490k tokio read_dir calls in the final pass and a similar number across earlier passes — on every reconstruct_at call. Since reconstruct_at backs the public GET /api/v1/replay/reconstruct route (pb-service/parquet.rs:41), a single request for a checkpoint-less asset turns into ~1M filesystem operations, a latency blowup and a cheap DoS vector. Additionally, latest_checkpoint_in_range fully JSON-parses bids/asks for every checkpoint row in range (extract_checkpoints, reader.rs:690-691) only to pop the last one — with 60s checkpoint cadence that is ~360 full-book JSON parses per 6h window per request.

**Recommendation:** Cap the checkpoint lookback at a configured retention horizon (or earliest existing partition discovered via a single directory listing), scan hour directories newest-first with early exit, only scan the newly extended range on each widening, and defer bids/asks JSON parsing until the winning row is selected (parse timestamps only for the max-scan).

### A.119 Backfill has no retry/backoff/metrics, drifting cadence, and a silent 1970-era fallback on seconds-resolution timestamps
- **Severity:** medium  |  **Area:** pb-replay  |  **Location:** `crates/pb-replay/src/backfill.rs:59`

run_backfill fetch failures are logged at error level and skipped with no retry, no backoff/jitter, and no pb_metrics counter — a persistently failing token (rate-limited, delisted) silently stops producing checkpoints, which both degrades replay (forces the expensive snapshot-scan path and the epoch-scan in finding 5) and goes unalerted. Checkpoint cadence also drifts: each cycle takes fetch RTTs + token_count * rate_limit_pause + interval, so the effective checkpoint period grows with the asset set rather than holding the configured 60s, widening the delta-replay span and replay gap risk over time. Separately, parse_timestamp_us classifies any value < 1e13 as milliseconds; a seconds-resolution timestamp (~1.75e9) would be multiplied by 1000 into a 1970-era microsecond value, producing a checkpoint partitioned into a 1970 hour directory that no recent-lookback search will ever find — a silent format change by the venue degrades checkpoints without any error.

**Recommendation:** Add bounded retry with jittered backoff per token, a pb_metrics counter for backfill fetch failures/successes with alerting, schedule cycles on an absolute interval (tokio::time::interval) rather than sleep-after-work, and sanity-check parsed timestamps against now (reject/clamp anything more than e.g. 24h from local time, logging at warn with the raw value).

### A.120 Row cap is not actually enforced: user-controlled max_rows is unclamped and any LIMIT token (even in a subquery/CTE) suppresses injection
- **Severity:** medium  |  **Area:** pb-service  |  **Location:** `crates/pb-service/src/query.rs:243`

The advertised guard rail 'inject LIMIT if missing' does not bound result size. Two independent bypasses: (1) inject_limit scans the entire sanitized query for any `LIMIT` token and skips injection if one exists anywhere — including inside a subquery or CTE. `WITH x AS (SELECT 1 LIMIT 1) SELECT * FROM big_table` returns big_table fully unbounded; likewise `SELECT * FROM big LIMIT 999999999` keeps the caller's huge limit. (2) The pb-api handler takes max_rows straight from the request body with no upper clamp (server.rs:447 `max_rows: req.max_rows.unwrap_or(state.config.query_max_rows)`), so a client can request `LIMIT 18446744073709551615`. No server-side max_result_rows/max_result_bytes/max_memory_usage is sent either. Result: unbounded result sets buffered fully into memory via resp.json(), enabling memory exhaustion / OOM of the API process and the ClickHouse server.

**Recommendation:** Clamp request max_rows to a hard server maximum. Enforce the cap at the top level rather than skipping on any nested LIMIT: wrap the validated query as `SELECT * FROM ( <user sql> ) LIMIT N` or detect only a top-level LIMIT. Additionally send ClickHouse settings `max_result_rows`, `max_result_bytes`, and `result_overflow_mode=throw` so the server enforces the cap regardless of the textual guard.

### A.121 Query timeout covers only the response-headers phase, not body download; reqwest client has no overall timeout and no server-side max_execution_time
- **Severity:** medium  |  **Area:** pb-service  |  **Location:** `crates/pb-service/src/query.rs:327`

tokio::time::timeout wraps only `client.post(...).send()`, which resolves when response headers arrive. The body is read afterwards by `resp.json().await` (line 342) and the error path's `resp.text().await` (line 336), both outside the timeout and with no per-request reqwest timeout (client is `reqwest::Client::new()`). A query that streams a large or slow body therefore hangs past timeout_secs. Because no `max_execution_time`/`cancel_http_readonly_queries_on_client_close` setting is sent to ClickHouse, the server keeps executing even if the client eventually gives up, wasting cluster resources. The guard's timeout is thus not a real wall-clock bound on query cost.

**Recommendation:** Configure the reqwest client with `.timeout(...)` covering the full request/response, or wrap the entire send+json future in tokio::time::timeout. Also pass ClickHouse `max_execution_time={timeout_secs}` and enable client-close cancellation so the server aborts when the client disconnects.

### A.122 Deterministic Parquet file names with unconditional overwrite put() silently destroy previously flushed files on collision
- **Severity:** medium  |  **Area:** pb-store  |  **Location:** `crates/pb-store/src/writer.rs:181`

Files are named {base}/{dataset}/{YYYY/MM/DD/HH}/{asset}_{first_ts_us}.parquet where first_ts_us is the partition timestamp of the first record in the group, and store.put() uses the default PutMode::Overwrite. If two different flushes produce the same key, the earlier file is silently replaced. This is realistic for book_checkpoints: partition_timestamp_us is checkpoint_timestamp_us = the exchange-side book timestamp (backfill.rs:130), which does not advance for a quiet book, so consecutive 5-minute flush windows can both start with the same timestamp and the second flush erases the first window's checkpoints. It also affects execution-append CLI runs with operator-supplied event_timestamp_us values (two appends whose first record shares a timestamp), and any re-ingest/backfill overlapping live data. Related robustness gap at writer.rs:159-161: from_timestamp_micros(...).unwrap_or_default() silently files records with out-of-range timestamps under 1970/01/01, where time-windowed replay reads will never find them.

**Recommendation:** Add a uniqueness discriminator to the filename (writer instance UUID + monotonic flush counter, or min_ts-max_ts range), and use put_opts with PutMode::Create so an unexpected collision fails loudly instead of destroying data. Reject or quarantine records whose timestamps cannot be partitioned instead of defaulting to epoch.

### A.123 ParquetSink buffering is unbounded between flushes, and synchronous flush stalls intake (backpressure couples WAL latency to sink I/O)
- **Severity:** medium  |  **Area:** pb-store  |  **Location:** `crates/pb-store/src/parquet_sink.rs:46`

Unlike ClickHouseSink (10k row trigger), ParquetSink has no size-based flush: the Vec grows for the full 5-minute window at whatever rate the feed produces (each PersistedRecord carries multiple heap Strings - 70+ char asset ids, session/event ids), so memory is unbounded under bursty multi-asset load. Additionally flush() runs inline in the select loop, so while a large buffer is being encoded and uploaded the sink does not recv(); the 10k sink channel and 2,048-slot fanout channels then fill, fanout_event blocks, and the ingest main loop stalls - which also delays WAL appends for subsequent events since WAL writes happen in the same loop (ingest.rs:132-147). A slow S3 flush therefore directly degrades durability latency of the WAL, not just storage freshness.

**Recommendation:** Add a max-buffered-rows (and/or bytes) trigger to ParquetSink mirroring ClickHouseSink. Consider moving encode+put onto a spawned task with buffer swap (double-buffering) so the sink keeps draining its channel during uploads, keeping backpressure away from the WAL path.

### A.124 ClickHouse multi-table flush is non-atomic and has no idempotency mechanism - any retry strategy on these MergeTree tables duplicates rows
- **Severity:** medium  |  **Area:** pb-store  |  **Location:** `crates/pb-store/src/writer.rs:534`

write_batch opens up to six independent inserts and calls .end() on them sequentially. If book_insert.end() succeeds and trade_insert.end() fails, the batch is torn across tables: book rows persisted, trade rows lost, and the whole function returns Err (killing the sink, which still holds all records). The tables are plain MergeTree (not ReplacingMergeTree), rows carry no dedup key, and non-replicated insert deduplication is off by default, so the obvious fix for the no-retry defect (re-running write_batch on the retained buffer) would duplicate the rows from the tables that already succeeded - i.e. the current design cannot deliver at-least-once without duplicates or at-most-once without loss. A single insert can also span multiple event_date partitions, so even one table's insert is not guaranteed atomic server-side.

**Recommendation:** Decide on semantics explicitly: for at-least-once, add a deterministic dedup identity (e.g. insert_deduplication_token per (batch_id, table) plus non_replicated_deduplication_window, or switch to ReplacingMergeTree keyed on (asset_id, recv_timestamp_us, sequence, side, price, source_event_id) with FINAL/argMax on read). Track per-table success within a batch so a retry only re-sends tables that failed.

### A.125 FixedSize string serde/parse routes through f64 and silently loses precision above 2^53 raw; WAL codec and checkpoint JSON depend on this path
- **Severity:** medium  |  **Area:** pb-types  |  **Location:** `crates/pb-types/src/fixed.rs:210`

TryFrom<&str> for FixedSize parses via f64 then multiplies and rounds (fixed.rs:210-215 -> from_f64 at :164-171). f64 has 53 mantissa bits, so any raw value above 2^53 (~9.007e15 raw = ~9.007 billion size units) cannot roundtrip: Serialize emits the exact decimal string, Deserialize parses it back through f64 and gets a different raw. Empirically verified: FixedSize::new(9007199254740993) serializes to "9007199254.740993" and deserializes back as 9007199254740994. This is not just a JSON-API concern: pb-wal's bincode codec (crates/pb-wal/src/codec.rs) serializes PersistedRecord through these same custom serde impls, and pb-store stores checkpoint levels as JSON (writer.rs:487 bids_json = serde_json::to_string(&event.bids)). So a sufficiently large size silently corrupts on WAL tail/replay — a direct violation of the zero-data-loss mandate. The Parquet/ClickHouse columnar paths use raw() and are safe. Practical likelihood is low (Polymarket sizes don't reach 9 billion shares) but nothing in the type enforces that bound, and the suite never tests the region (see separate finding on proptest ranges).

**Recommendation:** Replace the f64 round-trip with pure integer decimal parsing: split on '.', parse integer and fraction parts with checked_mul/checked_add against SIZE_SCALE, error on overflow and on more than 6 fraction digits. Apply the same to FixedPrice for symmetry. Then extend roundtrip proptests to the full u64 domain.

### A.126 Frame length field is not covered by any checksum; corruption recovery trusts the corrupt length, silently dropping or misparsing all valid records after it
- **Severity:** medium  |  **Area:** pb-wal  |  **Location:** `crates/pb-wal/src/segment.rs:153`

CRC32C covers only the payload (segment.rs:87, 174). A single bit flip in the 4-byte length field is undetectable directly: if the corrupted len overruns the file, read_record_at returns TruncatedRecord and the reader treats it as end-of-segment (reader.rs:121-133), silently discarding every valid record after that point in the segment; if it stays in-bounds, the CRC-mismatch skip path advances by FRAME_HEADER_LEN + corrupted_len (reader.rs:113-118), landing misaligned inside a valid record and cascading. Additionally, all corruption handling is a warn! log only — no counter, no metric, no signal to the consumer to trigger re-hydration — so a skipped book delta leaves the live book silently wrong until the next snapshot.

**Recommendation:** Extend the CRC to cover the header (or add a separate header CRC / magic byte per frame), and on corruption perform byte-wise scan-forward resynchronization to the next valid frame instead of trusting len. Return a skip count / expose a corruption counter so the serve process can set needs_resync and emit metrics.

### A.127 Pruner and retention are never invoked in production and max_segments is dead config — unbounded WAL disk growth; pruner safety also depends on a manually supplied consumer list and ignores malformed position files
- **Severity:** medium  |  **Area:** pb-wal  |  **Location:** `crates/pb-wal/src/writer.rs:75`

prune() and prune_with_backpressure() have zero callers outside pb-wal tests — no pruning task exists in the ingest process — and WalConfig::max_segments (documented as 'Maximum number of retained segments... Default: 16', parsed from config in pipeline.rs:156) is read by no code path in the crate. The WAL therefore grows until the disk fills, which would take down the ingest process. When pruning is eventually wired up, two safety gaps exist: (1) the caller must enumerate every consumer's position file; a consumer not in the list has its segments pruned out from under it (the gap_detection_after_pruning test demonstrates exactly this) even though the consumer_*.pos naming convention would allow automatic discovery; (2) min_consumer_segment silently ignores unparseable position-file contents — if all supplied files are malformed, min_seg stays u64::MAX and the function returns active.id, pruning every sealed segment despite live consumers (writer.rs:177-187).

**Recommendation:** Add a periodic pruning task in the ingest runtime using prune_with_backpressure; implement max_segments enforcement or delete the field; auto-discover consumer_*.pos files in the WAL directory instead of taking a caller list; treat an unreadable/unparseable position file as segment 0 (full retention), never as no-constraint.

### A.128 No writer mutual exclusion: two processes on the same WAL directory interleave appends, and Segment::create(truncate=true) can wipe an existing segment
- **Severity:** medium  |  **Area:** pb-wal  |  **Location:** `crates/pb-wal/src/segment.rs:33`

WalWriter::open takes no advisory lock (flock) on the WAL directory. Two ingest processes (operator error, supervisor double-start, stale process during deploy) both open_append the last segment with independent in-memory write_offsets and BufWriters, interleaving frame bytes and corrupting the log. Segment::create opens with .create(true).truncate(true), so any segment-id collision (e.g., second writer rotating to an id the first already created) silently truncates a segment containing data.

**Recommendation:** Acquire an exclusive advisory lock on a lock file (e.g., wal.lock via fs2/rustix flock) in WalWriter::open and fail fast if held. Use create_new(true) for new segments so an unexpected collision is an error rather than silent truncation.

### A.129 Ingest treats WAL open/append failure as a warning and continues, silently disabling the durability backbone
- **Severity:** medium  |  **Area:** pb-wal  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:75`

If WalWriter::open fails (bad path, permissions), ingest logs a warn and runs with wal_writer = None — every event flows only to in-memory-buffered sinks (Parquet buffers 5 minutes, ClickHouse 1 s) with no WAL, and the serve process's live tail and hydration silently see nothing new. Per-event append failures (e.g., disk full, since pruning never runs — see related finding) are likewise warn-and-continue with no metric, no escalation, and no backpressure, so a persistent failure produces an unbounded stream of dropped-from-WAL events while the process reports healthy.

**Recommendation:** Make WAL open failure fatal in ingest (or gated behind an explicit --allow-no-wal flag). On append failure, increment a Prometheus counter, attempt rotation/retry, and escalate to process shutdown after a bounded number of consecutive failures so supervision restarts into a clean state.

### A.130 SQL query workbench allows SSRF / arbitrary-file-read via ClickHouse table functions (guard is keyword-only, no readonly mode)
- **Severity:** medium  |  **Area:** security  |  **Location:** `crates/pb-service/src/query.rs:181-226`

POST /api/v1/query/sql forwards user SQL to ClickHouse. The guard (validate_read_only) only checks the root keyword is in {SELECT,WITH,SHOW,DESCRIBE,EXPLAIN} and rejects write keywords. It does NOT block ClickHouse table functions usable inside a SELECT: url(), file(), s3(), remote(), mysql(), postgresql(), hdfs(). The HTTP request also injects no readonly=1 setting and uses no restricted ClickHouse user (ClickHouseQueryService::new builds '{url}/?database=..&default_format=JSONCompact' and POSTs the body). On the ECS deployment this yields SSRF to the IMDS endpoint and theft of the task-role credentials, e.g. SELECT * FROM url('http://169.254.169.254/latest/meta-data/iam/security-credentials/<role>'), or local file read via SELECT * FROM file('...'). The endpoint has no authentication. Disabled by default (query_workbench_enabled=false), but when enabled the blast radius is credential exfiltration and internal-network pivot.

**Recommendation:** Run the workbench against a dedicated ClickHouse user with readonly=2 and table-function/remote access disabled (server-side profile), and append &readonly=1&max_execution_time=.. to the HTTP query URL. In the guard, deny a denylist of table functions (url, file, s3, remote, remoteSecure, mysql, postgresql, hdfs, jdbc, odbc, executable) and the SETTINGS clause. Require authentication on the API before enabling the workbench in any networked environment.

### A.131 Unauthenticated Prometheus metrics endpoint exposed to 0.0.0.0/0 on a public-IP Fargate task
- **Severity:** medium  |  **Area:** security  |  **Location:** `infra/vpc.tf:53-59`

The ECS security group opens port 9090 (the unauthenticated /metrics endpoint) to the entire internet, and the task runs in a public subnet with a public IP (assign_public_ip=true, map_public_ip_on_launch=true). Prometheus metrics leak operational topology, asset coverage, latency/lag, and error rates, and the endpoint has no auth or rate limiting. There is no ALB/WAF in front. This is a direct internet exposure of an internal observability surface.

**Recommendation:** Restrict the 9090 ingress to the VPC CIDR or a monitoring SG/security-group reference only; move the task to private subnets with a NAT gateway and set assign_public_ip=false; scrape metrics over the private network. Never expose /metrics to 0.0.0.0/0.

### A.132 GitHub Actions pinned to mutable tags and moving branch refs instead of commit SHAs
- **Severity:** medium  |  **Area:** security  |  **Location:** `.github/workflows/deploy.yml:29`

All workflows reference actions by mutable tag (actions/checkout@v6, Swatinem/rust-cache@v2, rustsec/audit-check@v2, softprops/action-gh-release@v3, aws-actions/configure-aws-credentials@v6, aws-actions/amazon-ecr-login@v2) and dtolnay/rust-toolchain@stable / @nightly are branch refs that move on every push. The deploy workflow holds id-token:write and assumes a privileged AWS role, so a compromised/retagged third-party action could mint AWS credentials and push images. Top-tier supply-chain practice pins third-party actions to full commit SHAs.

**Recommendation:** Pin every third-party action to a full 40-char commit SHA with a version comment; enable Dependabot for actions to bump pins. Reserve SHA pinning especially for any job carrying id-token:write or secrets.

### A.133 GitHub Actions deploy role grants ECS UpdateService/RegisterTaskDefinition on Resource="*"
- **Severity:** medium  |  **Area:** security  |  **Location:** `infra/iam.tf:116-125`

The CI/CD IAM policy allows ecs:UpdateService, ecs:RegisterTaskDefinition, ecs:DescribeServices, ecs:DescribeTaskDefinition on Resource="*". RegisterTaskDefinition + UpdateService account-wide means a compromised pipeline could redeploy or repoint any ECS service in the account (and RegisterTaskDefinition can specify arbitrary task roles, bounded only by the separately-scoped PassRole). Least privilege would scope these to the poly-book cluster/service ARNs.

**Recommendation:** Scope ecs:UpdateService/DescribeServices to the cluster/service ARN and condition RegisterTaskDefinition (e.g. ecs:family) to the poly-book family. Keep only ecr:GetAuthorizationToken on "*" (which AWS requires).

### A.134 S3 data bucket has force_destroy=true, no versioning, and SSE-S3 (not KMS)
- **Severity:** medium  |  **Area:** security  |  **Location:** `infra/s3.tf:1-4`

For a system whose stated bar is zero data loss, the market-data bucket sets force_destroy=true (a terraform destroy or recreate silently deletes all objects), has no versioning (no protection against accidental/malicious overwrite or delete - the ECS task role also holds s3:DeleteObject), and uses SSE-S3 (AES256) rather than a customer-managed KMS key (no key-level access control or audit). There is also no access logging.

**Recommendation:** Set force_destroy=false for the data bucket, enable versioning (plus MFA-delete or lifecycle-protected retention), switch to SSE-KMS with a CMK, and enable server access logging. Scope the task role to least privilege (drop s3:DeleteObject unless required).

### A.135 Entire integration test package is excluded from CI; ClickHouse tests are double-gated and never run anywhere
- **Severity:** medium  |  **Area:** testing  |  **Location:** `.github/workflows/ci.yml:33`

The test job runs `cargo test --workspace --exclude pb-integration-tests` and no other workflow (ci/codeql/deploy/release/supply-chain) ever runs the pb-integration-tests package. That means the system's only end-to-end coverage — checkpoint+WAL hydration (checkpoint_wal_hydration.rs), Parquet sink/reader roundtrip, replay engine reconstruction, dispatcher pipeline, book determinism, schema conversion, and cross-backend service parity — executes only when a developer remembers to run it locally. The ClickHouse tests are additionally `#[ignore]`d (clickhouse_roundtrip.rs:199, cross_backend_service.rs:226/285/358) even though ubuntu-latest runners support testcontainers Docker. Regressions in hydration, replay determinism, or storage roundtrips will pass CI green.

**Recommendation:** Add a CI job that runs `cargo test -p pb-integration-tests` (the non-Docker tests need nothing special), and a second job or nightly that runs the `--ignored` testcontainers tests using the runner's Docker. Replace the sleep(200ms)-based flush waits in those tests with explicit flush signals to avoid flakiness once they run in CI.

### A.136 Ingest durability failure paths untested: WAL append errors are warn-and-continue and the hot loop never flushes
- **Severity:** medium  |  **Area:** testing  |  **Location:** `crates/pb-bin/src/commands/ingest.rs:133`

In the ingest event loop, a WAL append or encode failure logs a warning and continues fanning out to sinks — the 'WAL-first durability' guarantee silently degrades to none (e.g., disk full), and no test exercises this path or asserts an operator-visible signal (metric/ingest event). Separately, `wal.flush()` is called only at graceful shutdown; with a 64 KiB BufWriter, a SIGKILL loses up to 64 KiB of acknowledged events with zero tests measuring or bounding that window, and no `sync()` (fsync) is ever called on the hot path. There is also no test reconciling Parquet's 5-minute in-memory flush buffer against the WAL after an ingest crash — the kill-and-restart end-to-end scenario ('crash mid-write, restart, verify zero loss beyond the documented window') does not exist anywhere in the test suite.

**Recommendation:** Add a periodic flush/sync policy (config-driven interval or per-N-records) with tests, emit a metric + IngestEvent on WAL append failure with a test asserting it, and add a process-level crash test (spawn ingest fixture, SIGKILL, restart, diff WAL contents against sent events).

### A.137 WS reconnect and backfill network loops have no integration tests — only backoff arithmetic is tested
- **Severity:** medium  |  **Area:** testing  |  **Location:** `crates/pb-feed/src/ws.rs:275`

pb-feed's WsClient tests cover only the backoff_ms math (5 tests, ws.rs:275-340). There is no test driving WsClient against a local mock WebSocket server for: connection drop and reconnect, re-subscription payload, lifecycle event emission ordering, or the critical 'reconnect with gap' scenario where deltas were missed during disconnect and correctness depends on the venue sending a fresh book snapshot that the dispatcher applies after `reset_continuity_state`. The dispatcher side is unit-tested (reconnect clears sequence/snapshot state), but the composed path WsClient→Dispatcher→book under a real disconnect is never executed by any test. Similarly, `run_backfill_with_token` (pb-replay/src/backfill.rs:31) — the loop, rate-limit pause handling, and channel-closed shutdown — is untested; only the pure `checkpoint_from_rest` conversion has unit tests.

**Recommendation:** Add an integration test using tokio-tungstenite's server side (or axum ws) that accepts a connection, sends a snapshot+deltas, drops the socket, accepts the reconnect, sends a diverged snapshot, and asserts the downstream book equals the post-reconnect snapshot state. Add a run_backfill test against a local HTTP stub covering success, 429 pause, and shutdown.

### A.138 Fuzz coverage stops at serde: dispatcher normalization, WAL codec decode, and config parsing are unfuzzed; no persistent corpus
- **Severity:** medium  |  **Area:** testing  |  **Location:** `fuzz/fuzz_targets/fuzz_ws_deser.rs:5`

fuzz_ws_deser only calls `serde_json::from_str::<WsMessage>` and discards the result — it never drives `Dispatcher::dispatch_raw`, so the normalization layer (timestamp parsing, side parsing, FixedPrice/FixedSize conversion, per-asset sequence assignment) is outside all fuzz input spaces. Notably, a malformed price mid-snapshot makes `make_book_event(...)?` abort the bids/asks loop (dispatcher.rs:208-217) after already emitting some Snapshot events with the sequence counter reset — a partial-snapshot persistence behavior no test covers. `pb_wal::codec::decode` is never fuzzed on arbitrary bytes (fuzz_wal_corruption fuzzes framing, but decode runs on every hydration record), and config/TOML+env parsing is unfuzzed. The CI fuzz job also rebuilds from an empty corpus each run (no corpus cache/artifact), so 30s smoke runs barely explore beyond trivial inputs.

**Recommendation:** Add a fuzz target that feeds arbitrary JSON through a Dispatcher with an open channel and asserts no panic plus invariants on emitted records (sequences contiguous per snapshot, no partial-snapshot emission — or make dispatch_raw validate all levels before emitting any). Add a fuzz_codec_decode target over raw bytes. Cache a fuzz corpus across CI runs via actions/cache.

### A.139 WAL codec roundtrip tests use single hand-picked values with Debug-string equality and no version-compat golden fixture
- **Severity:** medium  |  **Area:** testing  |  **Location:** `crates/pb-wal/src/codec.rs:286`

All six PersistedRecord variants do roundtrip (good), but each via exactly one hand-constructed value with every Option populated as Some, compared by `format!("{:?}")` string equality. None/Some combinations, empty strings, zero/max timestamps, and empty checkpoint bid/ask vectors are never exercised; a serde field attribute that drops data symmetrically on encode+decode would also pass Debug-equality. More importantly for a versioned codec: there is no golden-bytes fixture test pinning the v1 wire format, so an accidental field reorder or type change in PersistedRecord (which silently changes bincode layout) would still pass every roundtrip test while making all existing WAL segments and consumer replays undecodable — exactly the failure the version byte is meant to manage.

**Recommendation:** Derive PartialEq for the event types (or compare encoded bytes), add a proptest with Arbitrary-style strategies over all variants/Option combinations, and check in a golden fixture file of v1-encoded bytes with a test asserting decode produces known values — failing whenever the wire layout changes without a version bump.


## Severity: LOW

### A.140 overflow-checks not enabled in release: integer wrap is a panic in every tested build but silent corruption in production
- **Severity:** low  |  **Area:** build-dependency-hygiene  |  **Location:** `Cargo.toml:104`

`[profile.release]` does not set `overflow-checks`, so it defaults to off while dev/test builds check. For a fixed-point system (FixedSize u64 scaled by 1e6, FixedPrice u32 scaled by 1e4) where aggregate quantities like total_bid_size/total_ask_size and notional math accumulate across levels, an overflow in production wraps silently and poisons book state, checkpoints, and replay output — while the identical input in dev/test would panic loudly. The numerics dimension owns the specific arithmetic sites; the profile-level point is that this is another case (alongside panic=abort) where production executes different semantics than anything CI runs. The cost of `overflow-checks = true` on this workload (BTreeMap-walk dominated, no tight arithmetic kernels) is typically low single-digit percent, and given panic=abort the failure becomes a clean fail-stop instead of corrupted data — squarely the right trade for a zero-data-loss system.

**Recommendation:** Add `overflow-checks = true` to [profile.release] (and [profile.bench] so benchmarks measure the shipped configuration). Validate the cost with the existing Criterion suites; if any hot kernel measurably regresses, use explicit checked/saturating ops there rather than disabling checks globally.

### A.141 Production binary carries two complete HTTP stacks and two TLS implementations; the latency-critical WS feed alone rides OpenSSL; deny.toml allows unlimited duplicate versions
- **Severity:** low  |  **Area:** build-dependency-hygiene  |  **Location:** `Cargo.toml:43`

cargo tree on pb-bin shows reqwest 0.12.28 (via object_store 0.13) and reqwest 0.13.2 (direct) both compiled in — two hyper/h2/quinn stacks — plus two TLS implementations: rustls 0.23 (both reqwests, metrics-exporter-prometheus) and native-tls/OpenSSL (pb-feed's direct `native-tls = "0.2"` and tokio-tungstenite's `native-tls` feature, Cargo.toml:43). So the single most availability-critical connection (the Polymarket WebSocket feed) is the only thing using system OpenSSL, which (a) adds a dynamic libssl runtime dependency the Docker image doesn't satisfy, (b) means feed TLS behavior varies with the host's OpenSSL version, and (c) doubles the TLS audit surface. The lockfile (523 packages) also carries duplicate majors that `cargo tree -d` confirms: getrandom 0.2/0.3/0.4, hashbrown 0.14/0.15/0.16, rand 0.9+0.10, base64 x2, lz4_flex 0.11/0.13, itertools x2, foldhash x2, core-foundation x2. deny.toml sets `multiple-versions = "allow"` (deny.toml:36), so none of this can ever regress visibly in CI.

**Recommendation:** Switch tokio-tungstenite to `rustls-tls-native-roots` and drop the direct native-tls dependency from pb-feed — this removes OpenSSL from the binary entirely, makes the runtime image self-contained, and unifies TLS on rustls. Track object_store's reqwest 0.13 upgrade to collapse the duplicate HTTP stack. Tighten deny.toml bans to at least `multiple-versions = "warn"` with a skip list, and deny duplicates for crypto/TLS crates specifically.

### A.142 Resetting synthetic sequence makes event order ambiguous: 316 pre-snapshot deltas share recv_timestamp_us with snapshot batches and RecvTime replay sorts them AFTER the snapshot
- **Severity:** low  |  **Area:** data-artifact-forensics  |  **Location:** `crates/pb-replay/src/engine.rs:287`

The persisted `sequence` is a dispatcher-local counter that resets to 0 on every snapshot (dispatcher.rs:46,205), so it is not venue continuity data (true venue gap detection is impossible from stored data — confirming the prior finding) and values repeat within a session (e.g. asset 1060308395... has two deltas with sequence=99 35us apart on 2026-05-12). Concretely harmful case found in the data: 316 delta rows (288 distinct asset/instant pairs; 11 unambiguous with seq > snapshot_max+20) carry a pre-reset sequence but share recv_timestamp_us exactly with a snapshot batch. sort_book_events in RecvTime mode orders by (recv_ts, 0, sequence), so these stale deltas — generated BEFORE the snapshot — are replayed ON TOP of the fresh snapshot, corrupting one or more levels of any reconstruction whose window includes the tie. The WAL preserves true write order (replaying it positionally shows zero sequence gaps), but Parquet rows lose physical order once sorted by timestamp.

**Recommendation:** Make the per-asset sequence monotonic across the session (do NOT reset on snapshot) or persist a separate monotonic per-asset record index; then sort replay ties by that index. Until then, sort RecvTime ties so snapshot rows always come last within an equal recv_ts group only when their sequence epoch is newer — or simply break ties by file/row order within a flush.

### A.143 Documented `just replay` recipe always fails (missing required --mode), and the documented DuckDB inspection helpers are broken against split-dataset schemas
- **Severity:** low  |  **Area:** docs-spec-drift  |  **Location:** `justfile:49`

README.md:121 documents `just replay <TOKEN_ID> <TIMESTAMP_US>` as the canonical replay workflow, but the recipe (/Users/weiming/Documents/GitHub/poly-book/justfile:49-50) runs `cargo run -- replay --token X --at Y` without `--mode`, which clap requires with no default (/Users/weiming/Documents/GitHub/poly-book/crates/pb-bin/src/main.rs:69-71), so the command always errors. Separately, operations.md:236-244 and README.md:177-183 recommend `just parquet-count/peek/schema/stats`, which glob `'./data/**/*.parquet'` across all six split datasets that have different Arrow schemas (e.g. book_events vs trade_events in /Users/weiming/Documents/GitHub/poly-book/crates/pb-store/src/schema.rs:15-38), which DuckDB rejects without union_by_name once more than one dataset exists; `parquet-stats` (justfile:75-85) additionally aggregates a nonexistent `event_type` column with magic values 1/2/3 — the current schema has `event_kind` and split datasets, so the documented inspection workflow predates the dataset split.

**Recommendation:** Add `--mode recv_time` (or a mode parameter) to the just replay recipe; rewrite the DuckDB recipes to target per-dataset globs (e.g. data/book_events/**/*.parquet) and the `event_kind` column; update operations.md Local Inspection and README Common Workflows accordingly.

### A.144 No order-lifecycle state-machine validation: fill-after-cancel, ack-before-submit, oversized fills, empty order_id, and intra-order timestamp inversions are all accepted silently
- **Severity:** low  |  **Area:** execution-subsystem  |  **Location:** `/Users/weiming/Documents/GitHub/poly-book/crates/pb-bin/src/commands/execution_append.rs:103`

TryFrom<ExecutionAppendInput> validates only that event_kind and side parse as enums and price/size parse as numbers. There is no per-order coherence checking at append time (no read-before-write against existing history) and none at read time (build_execution_timeline only filters and truncates). The journal will happily record a Fill dated before SubmitIntent, a Fill after CancelAck, a PartialFill whose size exceeds the submitted size, an order with two Terminal events, or order_id = "" (only presence of the flag is enforced, not non-emptiness). For a dataset whose stated purpose is order-lifecycle inspection, incoherent lifecycles are silently presented to the UI/gRPC consumers as fact.

**Recommendation:** At minimum validate within a submitted batch: order events per order_id by timestamp and reject illegal transitions (fill/ack after Terminal/CancelAck, ack before submit, cumulative fill size > submitted size) and empty order_id. Ideally also read existing history for the touched order_ids and validate against it, with a --skip-lifecycle-checks escape hatch for backfilling partial histories.

### A.145 The CLI append/replay layer has zero tests and the execution write→read round-trip integration tests are excluded from CI
- **Severity:** low  |  **Area:** execution-subsystem  |  **Location:** `/Users/weiming/Documents/GitHub/poly-book/crates/pb-bin/src/commands/execution_append.rs:1`

execution_append.rs (the only write path: dual input modes, payload parsing, TryFrom validation, sink selection, base-path resolution) and execution_replay.rs contain no #[test] at all — JSON vs flag parity, deny_unknown_fields behavior, batch semantics, and error paths are unverified. Round-trip coverage does exist in tests/integration (clickhouse_roundtrip.rs:253 clickhouse_execution_event_roundtrip, cross_backend_service.rs:359 cross_backend_execution_equivalence), but CI runs `cargo test --workspace --exclude pb-integration-tests` and no other workflow job runs that crate, so the execution write→read path (including the ClickHouse DDL/row-struct compatibility) is never exercised in CI. pb-service unit tests and the pb-api route test cover the read side only with hand-constructed events, never data produced by the append command.

**Recommendation:** Add unit tests for ExecutionAppendPayload parsing (single/array/invalid-element/unknown-field/empty-array) and TryFrom validation in pb-bin; add an end-to-end test invoking run() against a tempdir Parquet store and reading back via ParquetExecutionService. Add a CI job (testcontainers, perhaps nightly or label-gated) that runs the pb-integration-tests crate so the ClickHouse execution round-trip is actually enforced.

### A.146 Release profile lacks overflow-checks; L2Book running totals use unchecked add/sub reachable with feed-controlled u64::MAX sizes
- **Severity:** low  |  **Area:** numerics  |  **Location:** `crates/pb-book/src/book.rs:61`

[profile.release] (Cargo.toml:104-108) does not enable overflow-checks, so all unchecked integer arithmetic wraps silently in production while tests/fuzz (debug-assertions on) would panic. The book's O(1) running totals are maintained with raw `-` and `+`. A hostile or corrupted feed message with size "1e30" parses via FixedSize::from_f64 to u64::MAX (saturating float→int cast); two such levels wrap total_bid_raw/total_ask_raw modulo 2^64, after which the 'old_raw <= total' assumption can break and subsequent removals underflow-wrap, permanently corrupting total_bid_size()/total_ask_size(). The fuzz target caps sizes at u32 (fuzz_book_delta.rs: `size_raw: u32`), so this region is outside fuzz coverage. Today the totals are only consumed by benches/tests, which limits blast radius, but the invariant is silently corruptible from network input.

**Recommendation:** Enable `overflow-checks = true` in [profile.release] (negligible cost off the hot loop for this workload, and aligned with zero-corruption goals), or use checked_add/checked_sub with an explicit invariant-violation log. Also widen the fuzz delta size to u64 so saturation-range sizes are exercised.

### A.147 Duplicated, divergent ms/µs timestamp heuristics mishandle seconds/nanosecond inputs and zero timestamps
- **Severity:** low  |  **Area:** numerics  |  **Location:** `crates/pb-feed/src/dispatcher.rs:423`

Two copies of parse_timestamp_us multiply any value < 1e13 by 1000 (assume ms) and pass larger values through (assume µs). A seconds-resolution venue timestamp (~1.7e9) is multiplied by only 1000, yielding a 1970-era µs value that silently corrupts exchange_timestamp_us across all persisted records, ws-latency metrics, staleness checks, and ExchangeTime replay ordering; a nanosecond timestamp passes through 1000x too large. The copies also diverge: the dispatcher guards `raw > 0`, but backfill.rs maps the string "0" to Some(0), so a venue "0" timestamp becomes checkpoint_timestamp_us = 0 instead of falling back to now_us, and the Parquet writer (writer.rs:160 `partition_timestamp_us() as i64` with unwrap_or_default) then files it under the 1970/01/01 hour partition where read_latest_checkpoint's bounded lookback will never find it.

**Recommendation:** Extract one shared pb-types function that classifies seconds/ms/µs/ns by magnitude bands and validates the converted result against a sane epoch range (e.g. 2020..2100), returning None and emitting a metric/ingest event on out-of-range input instead of silently storing a wrong timestamp.

### A.148 check_sequence's zero-sentinel disables gap detection in legitimate sequence-0 states (post-snapshot, post-checkpoint)
- **Severity:** low  |  **Area:** pb-book  |  **Location:** `crates/pb-book/src/book.rs:160`

check_sequence skips all validation when self.sequence.raw() == 0, using 0 to mean 'uninitialized'. But 0 is a legitimate sequence in this system: the dispatcher resets the per-asset counter to 0 on every snapshot (crates/pb-feed/src/dispatcher.rs:205) and emits Sequence(0) for the first event per asset (dispatcher.rs:362-369); replay applies checkpoints with Sequence::default() == 0 (crates/pb-replay/src/engine.rs:331); and a single-level snapshot leaves book.sequence == 0. In every such state, the next delta is accepted unconditionally — a dropped delta #1 after a snapshot or any number of dropped deltas after a checkpoint produce no SequenceGap continuity event, so the integrity summary under-reports real data loss. BookCheckpoint (crates/pb-types/src/event.rs:160-170) also does not persist the book's sequence, making cross-checkpoint continuity validation impossible.

**Recommendation:** Replace the 0 sentinel with explicit state: store sequence as Option<Sequence> (or a has_sequence flag) on L2Book, set None on construction and on checkpoint hydration, Some(n) after snapshots/deltas that carry a sequence. Persist the book sequence in BookCheckpoint so replay-from-checkpoint can validate the first tailed delta.

### A.149 Unchecked u64 arithmetic: running totals wrap in release (panic in debug) on saturated feed sizes; sequence increments overflow at u64::MAX
- **Severity:** low  |  **Area:** pb-book  |  **Location:** `crates/pb-book/src/book.rs:97`

total_bid_raw/total_ask_raw are updated with raw unchecked arithmetic (book.rs:61, 67, 90, 97, 102, 106-107). FixedSize::new accepts any u64 and FixedSize::from_f64 saturates the float-to-int cast to u64::MAX (crates/pb-types/src/fixed.rs:170), and the dispatcher parses sizes directly from untrusted feed strings via FixedSize::try_from (dispatcher.rs:317), so a malformed/malicious message like "1e300" yields a u64::MAX-raw level. Two such levels overflow the running sum: the workspace release profile (Cargo.toml [profile.release], no overflow-checks) wraps silently, permanently corrupting total_bid_size/total_ask_size and enabling subsequent underflow on removal (book.rs:90), while debug builds panic. The same unchecked '+ 1' pattern exists in check_sequence (book.rs:160-161), Sequence::next (crates/pb-types/src/newtype.rs: Self(self.0 + 1)), and the replay engine's expected-sequence computation (crates/pb-replay/src/engine.rs:255) — theoretical for locally-synthesized sequences, but the totals path is reachable from network input. Blast radius is currently limited because the totals are not consumed by production DTOs (see info finding), but the README advertises them as a feature.

**Recommendation:** Bound FixedSize at parse time (reject sizes above a sane market cap, mirroring FixedPrice::new validation) and use checked/saturating arithmetic for the running totals with a tracing warning on saturation. Use saturating_add for sequence+1 computations.

### A.150 Backoff jitter is nullified at the cap, synchronizing reconnects exactly when thundering herd matters
- **Severity:** low  |  **Area:** pb-feed  |  **Location:** `crates/pb-feed/src/ws.rs:224`

`backoff_ms` computes `exp.saturating_add(jitter).min(max)`. Once `exp >= reconnect_max_delay_ms` (attempt >= 9 with defaults, the steady state during any venue outage), the min() clamp erases the jitter entirely and every client reconnects at exactly 30,000 ms in lockstep — precisely the venue-restart scenario the README claims the jitter prevents. Additionally `fastrand_jitter`'s comment claims it hashes "nanosecond timestamp plus thread id" but the code uses only subsec_nanos (ws.rs:246-255), so replicas whose backoff timers fire in the same nanosecond bucket pattern get correlated jitter.

**Recommendation:** Cap first, then jitter: `let capped = exp.min(max); capped/2 + jitter(capped/2)` (equal-jitter) or full jitter `rand(0..=capped)`. Use a real PRNG seed (fastrand or process-unique seed) and fix the stale comment.

### A.151 gRPC server has no TLS, no auth, no request timeout, no concurrency or message-size limits, and defaults to 0.0.0.0:50051
- **Severity:** low  |  **Area:** pb-grpc-metrics  |  **Location:** `crates/pb-grpc/src/lib.rs:255`

tonic::transport::Server::builder() is used bare: no .timeout() (a slow Parquet/ClickHouse scan runs forever even after the client gives up — tonic does not enforce client grpc-timeout server-side), no .concurrency_limit_per_connection(), no max_encoding_message_size on WorkstationServiceServer (responses are unbounded; pairs with the missing limit validation), no TLS or authentication interceptor. config/default.toml:48 defaults listen_addr to "0.0.0.0:50051", so when enabled the full market-data and execution read surface is exposed unauthenticated on all interfaces. The same default-exposure pattern applies to the metrics endpoint (0.0.0.0:9090, config/default.toml:23) and the API (0.0.0.0:3000).

**Recommendation:** Default all listen addresses to 127.0.0.1; add Server::builder().timeout(Duration) and concurrency_limit_per_connection, set max_encoding_message_size/max_decoding_message_size on the service, and either add token-based auth via an interceptor or document the trust boundary (private network only) in an ADR.

### A.152 Replay reconstruction increments the live gap-detection metric and duplicates persisted gap events
- **Severity:** low  |  **Area:** pb-replay  |  **Location:** `crates/pb-replay/src/engine.rs:259`

reconstruct_book calls pb_metrics::record_gap_detected() every time a historical sequence discontinuity is re-derived during replay. Since reconstruct_at runs on every replay API request, repeatedly replaying a window that contains one historical gap inflates the operational gap counter on each request, corrupting dashboards and any alerting derived from it — operators cannot distinguish live feed gaps from read-path replays. The synthesized SequenceGap IngestEvents are also appended to continuity_events, which already contains the ingest_events persisted for that window, so the same discontinuity can be reported twice (once from storage, once re-derived) in API responses and CLI output.

**Recommendation:** Remove the live-feed metric from the replay path (or introduce a separate replay_gap_detected counter), and dedupe re-derived gaps against persisted ingest events (match on recv_timestamp_us + observed_sequence) before appending to continuity_events.

### A.153 Cancellation path flushes the local buffer but abandons records still queued in the mpsc channel
- **Severity:** low  |  **Area:** pb-store  |  **Location:** `crates/pb-store/src/parquet_sink.rs:52`

In both sinks, the token.cancelled() select arm flushes only the already-received buffer and returns - it never drains rx. Up to channel-capacity records (10,000 in pipeline.rs) that producers successfully sent before cancellation are silently dropped. The ingest command avoids this by relying on sender-drop/channel-close (which does drain), but backfill.rs:52 uses run_with_token with a child of the global shutdown token, so Ctrl+C during a backfill discards queued snapshots. The unit test parquet_sink_flushes_on_cancellation (tests.rs:609) only covers a record that was already received into the buffer (50ms sleep before cancel), so the gap is untested.

**Recommendation:** On cancellation, first drain the channel non-blockingly (while let Ok(r) = self.rx.try_recv() { buffer.push(r) } or rx.close() followed by recv-until-None) and then flush. Add a test that sends records, cancels immediately without yielding to the sink, and asserts all records are persisted.

### A.154 FixedPrice range invariant violable via public tuple field / new_unchecked; invalid values serialize successfully but fail to deserialize, poisoning persisted records
- **Severity:** low  |  **Area:** pb-types  |  **Location:** `crates/pb-types/src/fixed.rs:44`

FixedPrice is declared `pub struct FixedPrice(pub u32)`, so any code can construct FixedPrice(50_000) bypassing the <= 10_000 validation in new(); new_unchecked(:52) does the same deliberately but without even a debug_assert. The custom Serialize happily emits the out-of-range value ("5.0000", verified), while Deserialize rejects it. Because the WAL bincode codec uses these same serde impls, a record carrying an invalid price is accepted at write time and becomes unreadable at read time (WalError::Codec on tail/replay) — write-OK/read-FAIL asymmetry on the persistence path. AssetId(pub Arc<str>) and Sequence(pub u64) also expose pub fields, though they carry no invariants.

**Recommendation:** Make the tuple field private (raw()/new()/new_unchecked already cover all legitimate access), add debug_assert!(raw <= PRICE_SCALE) to new_unchecked, and consider validating in Serialize so an invalid value fails loudly at write time instead of silently producing unreadable bytes.

### A.155 String parsing silently rounds sub-tick price digits and silently saturates oversized sizes to u64::MAX
- **Severity:** low  |  **Area:** pb-types  |  **Location:** `crates/pb-types/src/fixed.rs:71`

FixedPrice::from_f64 computes `(v * PRICE_SCALE as f64).round() as u32` and FixedSize::from_f64 computes `(v * SIZE_SCALE as f64).round() as u64` (:170). Two silent behaviors verified: (1) sub-scale digits round without error — try_from("0.12345") yields raw 1235, and "1.00004" is accepted as exactly 1.0 even though it exceeds the valid price range; (2) Rust's saturating float-to-int cast means try_from("99999999999999999999999.0") yields u64::MAX with no error. The ingest hot path (pb-feed dispatcher.rs:316-317) parses every wire price/size through these functions, so if the venue ever emits finer precision or a corrupt huge number, data is silently altered rather than flagged — at odds with a faithful-persistence mandate.

**Recommendation:** In strict integer-based parsing (per finding 1), return an error when the input has more fraction digits than the scale supports or exceeds the representable range. If the venue legitimately sends sub-tick digits someday, surface it as an IngestEvent rather than silently rounding.

### A.156 Hydration computes WAL global offsets with default segment_size instead of the configured value, and global_offset is non-monotonic for records larger than a segment
- **Severity:** low  |  **Area:** pb-wal  |  **Location:** `crates/pb-api/src/hydration.rs:128`

replay_wal_tail builds `WalConfig { base_path, ..Default::default() }` and computes `seg_id * config.segment_size + seg_offset` for the checkpoint-skip comparison, while the writer's global_offset (writer.rs:65-70) uses the operator-configured segment_size from wal.segment_size_mb. If an operator sets any non-default segment size, hydration would skip the wrong record range — including silently skipping records that were not in the checkpoint, leaving gaps in the rebuilt book. Today this is latent because nothing ever populates BookCheckpoint.wal_offset (checkpoint producer, dispatcher, and backfill all set None, and WalWriter::global_offset has no production callers), which itself means hydration always replays the entire retained WAL on every serve restart. Separately, WalWriter::append admits records up to MAX_RECORD_SIZE (256 MB) into a fresh segment even when frame_size > segment_size (writer.rs:40-46), making write_offset exceed segment_size so global_offset can go backwards across the next rotation, violating its documented monotonicity for checkpoint coordination.

**Recommendation:** Pass the full WalConfig (not just base_path) into hydrate(), or better, store (segment_id, offset) pairs in checkpoints instead of a derived global offset so the math is independent of config. Reject or chunk records whose frame exceeds segment_size, or document and cap MAX_RECORD_SIZE at segment_size - FRAME_HEADER_LEN. Wire writer.global_offset() into the checkpoint producer so checkpoint-based WAL skipping actually functions.

### A.157 No authentication/authorization on any HTTP, WS, or gRPC surface; services bind 0.0.0.0 with no body/rate limits
- **Severity:** low  |  **Area:** security  |  **Location:** `crates/pb-api/src/server.rs:112-135`

The axum router applies only a metrics middleware - no auth, CORS allowlist, body-size limit, or rate limiting. gRPC (tonic) and metrics likewise have no interceptor/TLS. Default config binds api 0.0.0.0:3000, metrics 0.0.0.0:9090, grpc 0.0.0.0:50051. The trust boundary is documented as 'read-only, auth deferred' (docs/serve-api.md:58), but nothing enforces network isolation, so any deployment that exposes these ports is fully open, including unbounded POST bodies to /api/v1/query/sql and unlimited concurrent WS subscriptions (DoS).

**Recommendation:** Default-bind to 127.0.0.1 in config (override explicitly for deployment), add tower_http DefaultBodyLimit and a concurrency/rate limit on the SQL and WS routes, and document that the API must only run behind an authenticating reverse proxy or private network. Consider a static bearer-token gate even for read-only access.

### A.158 Runtime Docker image runs as root with tag-only (non-digest) base images
- **Severity:** low  |  **Area:** security  |  **Location:** `Dockerfile:20`

The final stage (debian:bookworm-slim) defines no USER, so poly-book runs as UID 0; a process compromise (e.g. via the SSRF/file-read path above) has root in the container and write access to the mounted /data volume. Base images are pinned only by floating tag (node:22-slim, rust:1.93-slim, debian:bookworm-slim), not by digest, so builds are not reproducible and are exposed to upstream tag mutation.

**Recommendation:** Add a non-root user (e.g. useradd app && USER app) and chown /data accordingly; pin base images by sha256 digest and update via automation. Consider distroless/static base for the runtime stage.

### A.159 Crossed-book invariant is vacuously tested: proptests constrain bids and asks to disjoint ranges, and the fuzz target never checks integrity
- **Severity:** low  |  **Area:** testing  |  **Location:** `crates/pb-book/src/book.rs:1148`

The properties named 'spread_never_negative', 'mid_price_between_best_bid_and_ask', and 'weighted_mid_bounded' generate bids in 1..=4999 and asks in 5001..=10000, so the book cannot cross by construction — the assertions can never fail and verify nothing about delta sequences. `integrity_detects_crossed_book` only tests a hand-built single-level crossed book. `fuzz_book_delta` checks per-side ordering and depth deltas but never calls `check_integrity`, so the relationship between arbitrary delta sequences and integrity detection (the headline 'book never crossed / crossing is always detected' invariant) is untested for multi-level books, books crossed via deltas rather than snapshots, and books that cross then un-cross. book_determinism.rs:146-149 even comments '(may be crossed due to random deltas, which is fine — just verify no panic)', acknowledging crossing happens in test workloads without asserting detection. Book-level behavior on out-of-order/regressing sequence numbers passed to apply_delta is also unasserted.

**Recommendation:** Add a property over UNCONSTRAINED snapshots+deltas asserting `check_integrity().is_err()` if and only if `best_bid >= best_ask`, and add the same assertion to fuzz_book_delta after every delta. Add a property for apply_delta with non-monotonic sequences documenting/asserting intended behavior.


---

# Appendix B — Refuted Claims (9)

Raised by reviewers but rejected by the verification panel; listed for transparency.

- EventProvenance.exchange_timestamp_us is non-optional, conflating 'missing' with epoch 0 and skewing ExchangeTime replay ordering (pb-types, `crates/pb-types/src/event.rs:50`)
- apply_delta applies stale/duplicate/out-of-order updates unconditionally and silently regresses sequence and last_update_us; live path never validates sequence (pb-book, `crates/pb-book/src/book.rs:112`)
- Snapshot grouping logic duplicated from live with weaker key: timestamp-equality merging can fuse distinct snapshots (pb-replay, `crates/pb-replay/src/engine.rs:199`)
- Graceful shutdown hangs indefinitely while any WebSocket client is connected (pb-api, `crates/pb-api/src/streaming.rs:174`)
- Live read model grows unboundedly with non-active assets seen in the record stream (pb-api, `crates/pb-api/src/live_state.rs:258`)
- fuzz_query_guard removed from CI with known unfixed SQL-guard bugs on a live endpoint (testing, `.github/workflows/ci.yml:129`)
- deny.toml ignores file-smuggling/symlink advisories and downgrades yanked/unmaintained checks (security, `deny.toml:5-16`)
- 2026-03-06 capture stores milliseconds in exchange_timestamp_us for all 1,690,687 rows — exchange-time unit bug confirmed in real data (data-artifact-forensics, `data/2026/03/06/07/events_52064299772353288812823375225002582079650622890558935845183759172433424203691_1772781592109667.parquet`)
- Same-millisecond snapshot drops confirmed in production data: 7 stale_snapshot_skip events, all on exact timestamp equality (data-artifact-forensics, `crates/pb-feed/src/dispatcher.rs:172`)