# ADR-0007: Release Profile — panic=abort with Symbolizable Backtraces

## Status
Accepted

## Context
The release profile sets `panic = "abort"`. This changes failure semantics
relative to the default unwinding profile used by `cargo test`/CI: a panicking
tokio task is no longer isolated and unwound — the entire process aborts
immediately (ingest, WAL writer, sinks, API all go down at once).

For a durability- and correctness-critical system this is a deliberate and
defensible choice — fail-stop is preferable to a process limping on in a
partially-broken state with a dead component silently dropping data. But it was
previously undocumented, and the profile also set `strip = "symbols"` with no
debug info, so a production abort emitted an unsymbolizable backtrace of raw
addresses. A system whose bar is "correctness under every
failure mode" must be able to diagnose the failures it does fail-stop on.

The native-CPU build flags that were checked into `.cargo/config.toml`
(`target-cpu=native`) are removed for the same family of reasons — see ADR
context in `.cargo/config.toml`.

## Decision
- Keep `panic = "abort"` in `[profile.release]` as the intended fail-stop
  behavior, and document it here.
- Stop stripping the binary (`strip = "none"`) and emit line-table debug info
  (`debug = "line-tables-only"`) so a crash produces a `file:line` backtrace.
- Enable `overflow-checks = true` in release so integer overflow traps (a wrap
  in fixed-point book state is data corruption, not an acceptable optimization).

## Consequences
- **Triageability**: production aborts now yield symbolizable backtraces with
  source locations, at the cost of a larger binary (line tables are far smaller
  than full DWARF, so the increase is modest).
- **Failure semantics divergence remains**: tests/CI run with unwinding, release
  aborts. Mitigate by building `--release` in CI so
  release-only breakage surfaces before deploy, and by supervising the process so
  an abort is restarted.
- **Reproducibility**: with `target-cpu=native` removed, release builds are
  portable and reproducible across hosts; pin an explicit microarch floor in the
  Docker build if fleet-specific tuning is wanted.
