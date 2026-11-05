# Contributing to poly-book

Thanks for contributing. This repository is a Rust workspace for Polymarket market-data ingestion, checkpointing, and replay tooling.

## Before You Start

- Read the [README](README.md) for project scope and the fastest local workflow.
- Use the pinned Rust toolchain or Rust 1.94+.
- Install Docker if you want to run the full integration suite. The S3 round-trip test needs an S3-compatible endpoint (CI uses `adobe/s3mock`).
- Install `just` if you want the shortcut commands. Cargo commands work directly too.

## Local Setup

```bash
git clone https://github.com/weiminglong/poly-book.git
cd poly-book
cargo build
```

Optional tools:

- `just` for task aliases
- `duckdb` for local Parquet inspection

## Test Matrix

[TESTING.md](TESTING.md) maps each failure mode to the layer that catches it
(unit/property tests, fuzzing, golden fixtures, monitoring rule tests)
and lists what is deliberately not covered. For day-to-day work, use the
smallest command that validates your change:

```bash
cargo test --workspace --exclude pb-integration-tests
```

Runs unit tests and fast workspace checks without the integration package.

```bash
cargo test -p pb-integration-tests
```

Runs integration tests. The `#[ignore]`d S3 round-trip additionally needs Docker and `PB_TEST_S3_ENDPOINT` (`-- --ignored`).

```bash
just ci
```

Runs the same formatting, lint, and test flow used in CI.

## Development Workflow

1. Open an issue for non-trivial work before you start, especially if it changes storage layout, replay semantics, or CLI behavior.
2. Keep changes scoped. Separate refactors from behavior changes when possible.
3. Add or update tests for behavior changes.
4. Update docs when flags, workflows, or architecture assumptions change.

For a meaningful architecture change, add or amend an ADR under `docs/adr/` so the decision and its rejected alternatives are recorded with the code.

## Style

- Run `cargo fmt --all`.
- Run `cargo clippy --workspace -- -D warnings`.
- Prefer explicit, typed interfaces over stringly-typed plumbing.
- Keep hot-path code simple and measurable.
- Preserve split dataset semantics across `book_events`, `trade_events`, `ingest_events`, `book_checkpoints`, and `replay_validations`.

## Pull Requests

- Target `main`.
- Include a short problem statement, the approach, and how you validated it.
- Call out operational impact if the change affects storage schema, replay ordering, or deployment.
- Keep PRs reviewable. If the diff is large, explain why.

## Reporting Bugs

Use the GitHub issue templates for normal bugs and feature requests.

If you find a security issue, do not open a public issue. Follow [SECURITY.md](SECURITY.md).
