# Contributing to poly-book

Thanks for contributing. This repository is a Rust workspace for Polymarket market-data ingestion, checkpointing, and replay tooling.

## Before You Start

- Read the [README](README.md) for project scope and the fastest local workflow.
- Use the current stable Rust toolchain or Rust 1.75+.
- Install Docker if you want to run the full integration suite. The ClickHouse integration tests use `testcontainers`.
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
- ClickHouse for manual warm-storage testing

## Test Matrix

Use the smallest command that validates your change:

```bash
cargo test --workspace --exclude pb-integration-tests
```

Runs unit tests and fast workspace checks without the integration package.

```bash
cargo test -p pb-integration-tests
```

Runs integration tests, including the ClickHouse round-trip coverage. Docker must be available.

```bash
just ci
```

Runs the same formatting, lint, and test flow used in CI.

## Development Workflow

1. Open an issue for non-trivial work before you start, especially if it changes storage layout, replay semantics, or CLI behavior.
2. Keep changes scoped. Separate refactors from behavior changes when possible.
3. Add or update tests for behavior changes.
4. Update docs when flags, workflows, or architecture assumptions change.

This repository uses OpenSpec for larger changes. If you are making a meaningful feature or architecture change, inspect the relevant materials under `openspec/changes/archive/` and follow the established pattern for proposals and tasks.

## Style

- Run `cargo fmt --all`.
- Run `cargo clippy --workspace -- -D warnings`.
- Prefer explicit, typed interfaces over stringly-typed plumbing.
- Keep hot-path code simple and measurable.
- Preserve split dataset semantics across `book_events`, `trade_events`, `ingest_events`, `book_checkpoints`, `replay_validations`, and `execution_events`.

## Pull Requests

- Target `main`.
- Include a short problem statement, the approach, and how you validated it.
- Call out operational impact if the change affects storage schema, replay ordering, or deployment.
- Keep PRs reviewable. If the diff is large, explain why.

## Reporting Bugs

Use the GitHub issue templates for normal bugs and feature requests.

If you find a security issue, do not open a public issue. Follow [SECURITY.md](SECURITY.md).
