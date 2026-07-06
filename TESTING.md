# Testing

This repository does not chase a coverage percentage. It assigns a defense to
each failure mode that matters for a market-data capture and replay system:
corrupted bytes on disk, malformed bytes off the wire, silent format drift
between builds, divergence between storage backends, non-deterministic replay,
and monitoring that rots until the night it is needed. Every layer below exists
because it catches a class of bug the other layers cannot.

All counts on this page were measured against this revision of the repository;
the [Verifying the numbers](#verifying-the-numbers) section gives the exact
commands so they can be re-checked instead of trusted.

## Failure-mode → defense matrix

| # | Failure mode | Primary defense | Backstop | Runs in CI? |
|---|--------------|-----------------|----------|-------------|
| 1 | Numeric precision loss or rounding drift in prices/sizes | 16 proptest properties on `FixedPrice`/`FixedSize` (roundtrip, ordering, serde) | `fuzz_fixed_price` (parse + serde roundtrip assert), 170 unit tests in pb-types | yes (`test`, `fuzz`) |
| 2 | Order-book invariant violations (mis-sorted levels, wrong size accounting) | 17 proptest properties on `L2Book` | `fuzz_book_delta` (arbitrary snapshots + deltas, ordering asserted after every step), 74 unit tests in pb-book | yes (`test`, `fuzz`) |
| 3 | Malformed venue input crashing ingest | `fuzz_ws_deser` (arbitrary bytes into the wire parser: must error, never panic) | 67 unit tests in pb-feed (dispatcher normalization, reconnect paths) | yes (`fuzz`, `test`) |
| 4 | WAL corruption on disk (bit flips, torn writes, zero-filled tails) | `fuzz_wal_corruption` (write, corrupt bytes, assert reader never panics and returns only CRC-valid records) | pb-wal unit tests for truncated-tail recovery, CRC/length corruption, prune safety (80 test attributes) + 2 proptest properties | yes (`fuzz`, `test`) |
| 5 | Silent WAL byte-format drift between builds | Golden-bytes fixture `golden_codec_book_v2_bytes_are_stable` (pb-wal): any codec change that alters encoded bytes fails the build | `fuzz_codec_decode` (arbitrary bytes into `codec::decode`: error, never panic); version byte rejected on mismatch | yes (`test`, `fuzz`) |
| 6 | Non-deterministic replay (same events, different book) | Determinism fixture `tests/integration/book_determinism.rs` (3 tests) | Golden replay regression `golden_replay_produces_expected_book` (pb-replay), 47 pb-replay test attributes | yes (`test`) |
| 7 | Parquet and ClickHouse answering the same query differently | 3 cross-backend equivalence tests (replay, integrity, execution) in `tests/integration/cross_backend_service.rs` | 5 ClickHouse round-trip tests, 1 S3/MinIO round-trip; `reconcile` rebuilds Parquet from the WAL when they do diverge | **no — `#[ignore]`d, Docker-backed, run locally** |
| 8 | SQL escaping the read-only query workbench | `fuzz_query_guard` (guarded SQL must be a fixed point of the guard) | Guard unit tests in pb-service/pb-api; server-side readonly + LIMIT enforcement | yes (`fuzz`, `test`) |
| 9 | Crash/restart data loss across the ingest/serve boundary | 2 checkpoint + WAL hydration integration tests (`checkpoint_wal_hydration.rs`) | pb-wal reopen-recovery and position-file tests; standby-writer flock takeover test | yes (`test`) |
| 10 | Undefined behavior / memory unsafety | Miri on pb-types and pb-book unit tests | Workspace is overwhelmingly safe Rust; `clippy -D warnings` | yes (`miri`) |
| 11 | Latency-harness rot and perf regressions | `cargo bench --workspace --no-run` compiles all 8 Criterion benchmarks every CI run | Statistical regression gating against a committed baseline — local only (shared runners are too noisy) | compile: yes (`bench`); gating: **no** |
| 12 | Alert rules that no longer fire, or page the wrong channel | `promtool test rules`: 8 simulated-incident scenarios with 10 fire/no-fire assertions against the 13 rules in `monitoring/alerts.yml` | `promtool check rules`, `amtool check-config`, and 3 severity→receiver routing assertions | yes (`monitoring`) |
| 13 | Vulnerable dependencies, license violations, IaC misconfig | `rustsec/audit-check` on every push/PR | `cargo-deny` (advisories/bans/licenses/sources), CodeQL SAST, `tfsec` + `tflint` + `terraform validate` over `infra/` | yes (`audit`, separate workflows) |
| 14 | Frontend regressions | 140 vitest cases across 16 test files; `tsc -b` strict typecheck; `biome check` | 5 Playwright end-to-end tests (3 specs) against the built bundle | yes (`web`, `e2e`) |
| 15 | Release-profile-only breakage (panic=abort, LTO) and Docker image rot | `cargo build --release` in CI | `docker build` of the production image in CI | yes (`release-build`, `docker-build`) |
| 16 | Documentation and comment hygiene drift | `hygiene` job greps for internal tracking references that resolve to nothing in a public repo | Per-crate "Docs to Update After Changes" tables | yes (`hygiene`) |

## Layer detail

### Unit and integration tests

```bash
cargo test --workspace --exclude pb-integration-tests   # unit tests, no Docker
cargo test -p pb-integration-tests                      # integration package
cargo test -p pb-integration-tests -- --ignored         # Docker-backed suites
```

Test attributes (`#[test]` + `#[tokio::test]`) per crate at this revision:

| Crate | Tests | Crate | Tests |
|-------|------:|-------|------:|
| pb-types | 170 | pb-replay | 47 |
| pb-api | 81 | pb-store | 36 |
| pb-wal | 80 | pb-grpc | 25 |
| pb-book | 74 | pb-metrics | 14 |
| pb-bin | 74 | pb-service | 72 |
| pb-feed | 67 | `tests/integration` | 22 |

Total: 762 test attributes. These counts include `#[ignore]`d tests: one in
pb-wal (a failover timing drill, run via `just failover-drill`) and nine in the
integration package (see below).

The integration package (`tests/integration/`) splits cleanly:

- **Run in CI** (13 tests): book determinism (3), checkpoint + WAL hydration
  (2), dispatcher pipeline (2), Parquet round-trip (2), replay engine (2),
  schema conversion (2).
- **`#[ignore]`d, Docker-backed, not run in CI** (9 tests): ClickHouse
  round-trips (5), cross-backend Parquet/ClickHouse equivalence for replay,
  integrity, and execution (3), and an S3/MinIO round-trip (1). They use
  `testcontainers` and require a local Docker daemon:
  `cargo test -p pb-integration-tests -- --ignored`.

### Property-based tests

35 proptest properties across three crates, each exercised over randomized
inputs on every `cargo test` run:

- **pb-types** (16 properties, `src/fixed.rs`): fixed-point construction and
  f64 roundtrip, total ordering, serde roundtrip consistency.
- **pb-book** (17 properties, `src/book.rs`): bid/ask ordering after arbitrary
  snapshot/delta sequences, non-negative spread on non-crossed books, mid and
  weighted-mid bounded by best bid/ask, sequence-gap detection soundness,
  snapshot idempotency, total-size accounting.
- **pb-wal** (2 properties, `src/lib.rs`): arbitrary payloads survive the
  write→read roundtrip; segment rotation preserves record ordering.

### Fuzzing

Six libFuzzer targets under `fuzz/fuzz_targets/`. CI runs each for 30 seconds
as a smoke test on every push/PR (`fuzz` job); longer local runs use the same
commands without the time cap.

```bash
cargo +nightly fuzz run fuzz_ws_deser        # venue wire frames: error, never panic
cargo +nightly fuzz run fuzz_fixed_price     # fixed-point parse + serde roundtrip
cargo +nightly fuzz run fuzz_book_delta      # book ordering invariants under arbitrary deltas
cargo +nightly fuzz run fuzz_wal_corruption  # corrupted segments: reader survives, skips bad frames
cargo +nightly fuzz run fuzz_codec_decode    # WAL codec decode of arbitrary bytes
cargo +nightly fuzz run fuzz_query_guard     # guarded SQL is a fixed point of the guard
```

### Golden fixtures and format gates

Two frozen fixtures turn accidental format drift into a test failure, so a
version bump is always a deliberate act:

- `golden_codec_book_v2_bytes_are_stable` (`crates/pb-wal/src/codec.rs`) pins
  the exact encoded bytes of a WAL frame at codec version 2
  (`pb_wal::codec::CURRENT_VERSION`).
- `golden_replay_produces_expected_book` (`crates/pb-replay/src/tests.rs`) pins
  the reconstructed book for a fixed record set.

Both persisted formats fail closed on version mismatch: the WAL codec rejects
unknown version bytes, and the Parquet reader rejects files whose
`pb_schema_version` metadata differs from `pb_store::schema::PB_SCHEMA_VERSION`
(currently `"2"`). The migration procedure is in
[docs/operations.md](docs/operations.md).

### Miri

```bash
cargo +nightly miri test -p pb-types -- --test-threads=1 --skip proptests
cargo +nightly miri test -p pb-book  -- --test-threads=1 --skip proptests
```

CI runs the unit tests of the two foundation crates under Miri to catch
undefined behavior. Proptest suites are skipped under Miri (interpreter
overhead), and Miri does not cover the tokio-heavy crates.

### Monitoring rule tests

```bash
promtool check rules monitoring/alerts.yml
promtool test rules monitoring/alerts_test.yml
amtool check-config monitoring/alertmanager.yml
```

`monitoring/alerts.yml` defines 13 alert rules. `monitoring/alerts_test.yml`
replays 8 simulated incidents offline (WAL append failure, WAL decode error,
silent feed, consumer lag, and so on) with 10 assertions covering both that
the alert fires with the expected labels and that a healthy signal does not
fire. CI additionally asserts the Alertmanager severity→receiver
routing (`critical` → PagerDuty, `warning`/`info` → Slack) so routing cannot
drift from the rules' severity labels. Live delivery (a real PagerDuty page)
is environment setup, not covered here.

### Benchmarks

Eight Criterion benchmarks across six crates cover the latency-relevant paths:
fixed-point ops and wire deserialization (pb-types), book ops and depth
iteration (pb-book), dispatcher normalization (pb-feed), WAL append/encode
(pb-wal), read-model publish (pb-api), and cross-backend query latency
(pb-service).

```bash
cargo bench --workspace --no-run   # what CI does: compile, don't measure
cargo bench                        # local measurement runs
```

CI compiles every benchmark so the harness cannot rot, but does not gate on
timings: statistical regression detection needs quiet, dedicated hardware, not
a shared runner.

### Web and end-to-end

```bash
cd web
npx biome check . && npx tsc -b && npx vitest run   # what the web job runs
npx vite build && npx playwright test               # what the e2e job runs
```

140 vitest cases across 16 test files, plus 5 Playwright tests in 3 specs
(live feed, replay, order-book demo) that run against the built bundle.

### Static analysis and supply chain

- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check`
  on every push/PR.
- `hygiene` job: rejects internal tracking references in comments and docs.
- `rustsec/audit-check` (`audit` job) plus `cargo deny check advisories bans
  licenses sources` (`supply-chain.yml`, also weekly).
- CodeQL SAST over the GitHub Actions workflows and the TypeScript frontend
  (`codeql.yml`).
- `tfsec`, `tflint` (AWS ruleset), and `terraform fmt`/`validate` over
  `infra/` (`iac-scan` job in `supply-chain.yml`).

## Known gaps

Being explicit about what is *not* covered:

- **No kill -9 crash-recovery end-to-end test.** Torn-tail truncation, CRC
  skipping, and reopen recovery are unit-tested by constructing damaged
  segments directly, and the flock standby-takeover path has a test — but no
  harness SIGKILLs a live `ingest` mid-append and asserts the full
  recover-and-reconcile path end to end.
- **No soak or load testing.** There is no sustained multi-hour run under
  production-like message rates; channel capacities are sized by reasoning
  plus the exported depth gauge, not by a measured burst profile.
- **No chaos injection.** Disk-full, ClickHouse partitions mid-batch, and
  clock skew are handled by code paths with unit tests, but nothing injects
  those faults into a running topology.
- **Cross-backend equivalence is not in CI.** The three equivalence tests and
  the ClickHouse/MinIO round-trips need Docker and are `#[ignore]`d; they run
  locally and are the main guard against warm/cold divergence (matrix row 7).
- **Performance regressions are not gated in CI** — the harness is
  compile-checked only; measurement is a local, manual step.
- **Miri covers only pb-types and pb-book**, and skips their proptest suites.

## Verifying the numbers

```bash
# Per-crate test attribute counts
for d in crates/*/; do
  printf '%s %s\n' "$d" \
    "$(grep -rE '#\[test\]|#\[tokio::test\]' "$d" --include='*.rs' | wc -l)"
done
grep -rE '#\[test\]|#\[tokio::test\]' tests/ --include='*.rs' | wc -l

# Ignored (Docker-backed) integration tests
grep -rn '#\[ignore' tests/integration/

# Proptest properties (count fn items inside proptest! blocks)
grep -rln 'proptest!' crates/

# Fuzz targets
ls fuzz/fuzz_targets/

# Alert rules and offline incident scenarios
grep -c 'alert:' monitoring/alerts.yml
grep -c '^  - interval:' monitoring/alerts_test.yml

# CI jobs
grep -E '^  [a-z-]+:' .github/workflows/ci.yml
```
