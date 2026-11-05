# Testing

This repository does not chase a coverage percentage. It assigns a defense to
each failure mode that matters for a market-data capture and replay system:
corrupted bytes on disk, malformed bytes off the wire, silent format drift
between builds, storage that does not survive a round-trip, non-deterministic
replay, and monitoring that rots until the night it is needed. Every layer below exists
because it catches a class of bug the other layers cannot.

All counts on this page were measured against this revision of the repository;
the [Verifying the numbers](#verifying-the-numbers) section gives the exact
commands so they can be re-checked instead of trusted.

## Failure-mode → defense matrix

| # | Failure mode | Primary defense | Backstop | Runs in CI? |
|---|--------------|-----------------|----------|-------------|
| 1 | Numeric precision loss or rounding drift in prices/sizes | 16 proptest properties on `FixedPrice`/`FixedSize` (roundtrip, ordering, serde) | `fuzz_fixed_price` (parse + serde roundtrip assert), 114 unit tests in pb-types | yes (`test`, `fuzz`) |
| 2 | Order-book invariant violations (mis-sorted levels, wrong size accounting) | 17 proptest properties on `L2Book` | `fuzz_book_delta` (arbitrary snapshots + deltas, ordering asserted after every step), 66 unit tests in pb-book | yes (`test`, `fuzz`) |
| 3 | Malformed venue input crashing ingest | `fuzz_ws_deser` (arbitrary bytes into the wire parser: must error, never panic) | 69 unit tests in pb-feed (dispatcher normalization, reconnect paths) | yes (`fuzz`, `test`) |
| 4 | WAL corruption on disk (bit flips, torn writes, zero-filled tails) | `fuzz_wal_corruption` (write, corrupt bytes, assert reader never panics and returns only CRC-valid records) | pb-wal unit tests for truncated-tail recovery, CRC/length corruption, prune safety (81 test attributes) + 2 proptest properties | yes (`fuzz`, `test`) |
| 5 | Silent WAL byte-format drift between builds | Golden-bytes fixture `golden_codec_book_v3_bytes_are_stable` (pb-wal): any codec change that alters encoded bytes fails the build | `fuzz_codec_decode` (arbitrary bytes into `codec::decode`: error, never panic); version byte rejected on mismatch | yes (`test`, `fuzz`) |
| 6 | Non-deterministic replay (same events, different book) | Determinism fixture `tests/integration/book_determinism.rs` (3 tests) | Golden replay regression `golden_replay_produces_expected_book` (pb-replay), 51 pb-replay test attributes | yes (`test`) |
| 7 | Parquet written locally or to S3 reading back differently | Parquet round-trip integration tests (`tests/integration/parquet_roundtrip.rs`); S3/MinIO write + production-reader round-trip (`s3_minio_roundtrip.rs`) | Manifest-switch and partial-hour refusal tests for `reconcile`; `pb_schema_version` rejection on mismatch | yes (`test`; `integration-docker` runs the `#[ignore]`d S3 case) |
| 8 | Crash/restart data loss across the ingest/serve boundary | 2 checkpoint + WAL hydration integration tests (`checkpoint_wal_hydration.rs`) | pb-wal reopen-recovery and position-file tests; standby-writer flock takeover test | yes (`test`) |
| 9 | Undefined behavior / memory unsafety | `unsafe_code = "forbid"` on every crate (compile-time) | `clippy -D warnings` | yes (`clippy`) |
| 10 | Latency-harness rot and perf regressions | `cargo check --all-targets` compiles all 8 Criterion benchmarks every CI run | Statistical regression gating against a committed baseline — local only (shared runners are too noisy) | compile: yes (`check`); gating: **no** |
| 11 | Alert rules that no longer fire | `promtool test rules`: 11 simulated-incident scenarios with fire/no-fire assertions against the 15 rules in `monitoring/alerts.yml` | `promtool check rules` | yes (`monitoring`) |
| 12 | Vulnerable dependencies, license violations | `cargo-deny` (advisories/bans/licenses/sources) on every push/PR and weekly | — | yes (`supply-chain.yml`) |
| 13 | Frontend regressions | vitest suites per feature; `tsc -b` strict typecheck; `biome check` | Playwright end-to-end specs (live feed, replay, order-book demo) against the built bundle | yes (`web`, `e2e`) |
| 14 | Release-profile-only breakage (panic=abort, LTO) and Docker image rot | `docker build` of the production image in CI (a release-profile build) | — | yes (`docker-build`) |

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
| pb-types | 114 | pb-replay | 51 |
| pb-api | 70 | pb-store | 38 |
| pb-wal | 81 | pb-service | 19 |
| pb-book | 66 | pb-metrics | 8 |
| pb-bin | 50 | `tests/integration` | 14 |
| pb-feed | 69 | | |

Total: 580 test attributes. These counts include `#[ignore]`d tests: one in
pb-wal (a failover timing drill, run via `just failover-drill`) and one in the
integration package (see below).

The integration package (`tests/integration/`) splits cleanly:

- **Run in CI** (13 tests): book determinism (3), checkpoint + WAL hydration
  (2), dispatcher pipeline (2), Parquet round-trip (2), replay engine (2),
  schema conversion (2).
- **`#[ignore]`d, Docker-backed, run by the `integration-docker` CI job** (1 test): an
  S3 round-trip through the production `ParquetReader`. It requires a
  running S3-compatible endpoint in `PB_TEST_S3_ENDPOINT` (CI starts
  `adobe/s3mock`). Run it locally with:
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

Five libFuzzer targets under `fuzz/fuzz_targets/`. `fuzz.yml` runs each for 30
seconds as a smoke test nightly and on every push to `main`; longer local runs
use the same commands without the time cap.

```bash
cargo +nightly fuzz run fuzz_ws_deser        # venue wire frames: error, never panic
cargo +nightly fuzz run fuzz_fixed_price     # fixed-point parse + serde roundtrip
cargo +nightly fuzz run fuzz_book_delta      # book ordering invariants under arbitrary deltas
cargo +nightly fuzz run fuzz_wal_corruption  # corrupted segments: reader survives, skips bad frames
cargo +nightly fuzz run fuzz_codec_decode    # WAL codec decode of arbitrary bytes
```

### Golden fixtures and format gates

Two frozen fixtures turn accidental format drift into a test failure, so a
version bump is always a deliberate act:

- `golden_codec_book_v3_bytes_are_stable` (`crates/pb-wal/src/codec.rs`) pins
  the exact encoded bytes of a WAL frame at codec version 3
  (`pb_wal::codec::CURRENT_VERSION`).
- `golden_replay_produces_expected_book` (`crates/pb-replay/src/tests.rs`) pins
  the reconstructed book for a fixed record set.

Both persisted formats fail closed on version mismatch: the WAL codec rejects
unknown version bytes, and the Parquet reader rejects files whose
`pb_schema_version` metadata differs from `pb_store::schema::PB_SCHEMA_VERSION`
(currently `"2"`). The migration procedure is in
[docs/operations.md](docs/operations.md).

### Monitoring rule tests

```bash
promtool check rules monitoring/alerts.yml
promtool test rules monitoring/alerts_test.yml
```

`monitoring/alerts.yml` defines 15 alert rules. `monitoring/alerts_test.yml`
replays 11 simulated incidents offline (target down, absent ingest series,
WAL append failure, WAL decode error, silent feed, consumer lag, and so on)
with assertions covering both that
the alert fires with the expected labels and that a healthy signal does not
fire. Live delivery (a real PagerDuty page)
is environment setup, not covered here.

### Benchmarks

Eight Criterion benchmarks across five crates cover the latency-relevant paths:
fixed-point ops and wire deserialization (pb-types), book ops, depth iteration,
and the dense-array alternative (pb-book), dispatcher normalization (pb-feed),
WAL append/encode (pb-wal), and read-model publish (pb-api).

```bash
cargo check --all-targets          # what CI does: compile the harness, don't measure
cargo bench                        # local measurement runs
```

CI compiles every benchmark (they are targets of `cargo check --all-targets`)
so the harness cannot rot, but does not gate on timings: statistical regression
detection needs quiet, dedicated hardware, not a shared runner.

### Web and end-to-end

```bash
cd web
npx biome check . && npx tsc -b && npx vitest run   # what the web job runs
npx vite build && npx playwright test               # what the e2e job runs
```

vitest suites under `web/src`, plus Playwright specs (live feed, replay,
order-book demo) that run against the built bundle.

### Static analysis and supply chain

- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check`
  on every push/PR.
- `cargo deny check advisories bans licenses sources` (`supply-chain.yml`, on
  every push/PR and weekly).

## Known gaps

Being explicit about what is *not* covered:

- **No kill -9 crash-recovery end-to-end test.** Torn-tail truncation, live-reader
  CRC skipping, strict recovery rejection, manifest publication, and reopen
  recovery are unit-tested by constructing damaged segments and storage views
  directly, and the flock standby-takeover path has a test — but no
  harness SIGKILLs a live `ingest` mid-append and asserts the full
  recover-and-reconcile path end to end.
- **Recovery is intentionally scoped.** Crash-consistent replacement is tested
  for receive-time-partitioned book/trade/ingest hours. Checkpoint and
  validation partitions are rejected because WAL endpoint timestamps cannot
  prove those independently partitioned datasets complete.
- Recovery reader coverage includes an asset containing `/` and `%`, catching
  double-encoding bugs across object filenames, manifests, and cleanup paths.
- Reconcile validates the whole retained WAL without retaining it, rejects an
  eligible hour above 2,000,000 records or 128 MiB of encoded WAL payload, then
  bounds decoded recovery memory to one eligible hour at a time.
- **No soak or load testing.** There is no sustained multi-hour run under
  production-like message rates; channel capacities are sized by reasoning
  plus the exported depth gauge, not by a measured burst profile.
- **No chaos injection.** Disk-full, object-store failures mid-flush, and
  clock skew are handled by code paths with unit tests, but nothing injects
  those faults into a running topology.
- **Performance regressions are not gated in CI** — the harness is
  compile-checked only; measurement is a local, manual step.

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
