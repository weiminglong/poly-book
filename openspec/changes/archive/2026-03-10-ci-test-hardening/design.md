## Context

The CI pipeline runs 8 parallel jobs (check, test, clippy, fmt, audit, web, fuzz,
miri) with solid Rust and web quality gates. A team audit identified gaps:

- No SAST or secret scanning
- `protobuf-compiler` installed identically in 3 jobs
- Playwright E2E tests exist but are not run in CI
- pb-api has zero unit tests (relies entirely on integration tests)
- 6 custom React hooks have no unit tests
- No test coverage reporting

The codebase is small enough that adding these now is straightforward.

## Goals / Non-Goals

**Goals:**

- Add CodeQL and secret scanning to catch vulnerabilities before merge
- Consolidate protobuf setup to reduce CI maintenance
- Run Playwright E2E in CI to catch UI regressions
- Add pb-api unit tests for route error paths
- Add web hook unit tests for behavioral correctness
- Enable coverage reporting so gaps are visible

**Non-Goals:**

- Rewriting existing tests or changing test frameworks
- Adding coverage thresholds that block merges (reporting only for now)
- Adding load testing or performance benchmarks to CI
- Changing branch protection rules (document expectations only)
- Adding SBOM generation (low priority, separate change)

## Decisions

### 1. CodeQL over Semgrep for SAST

CodeQL is free for public repos on GitHub, integrates natively with PR checks, and
supports both Rust and TypeScript. Semgrep would require a separate service account
and configuration. CodeQL runs as a separate workflow on `codeql.yml` to avoid
slowing down the main CI pipeline.

### 2. Composite action for protobuf

A composite action at `.github/actions/setup-protobuf/action.yml` replaces the
three identical `sudo apt-get install -y protobuf-compiler` lines. This is simpler
than a reusable workflow (no `workflow_call` overhead) and keeps the change local
to one file.

### 3. Playwright as a separate CI job (not inside the web job)

E2E tests need a built app served on localhost. Running them inside the existing
web job would add significant time to a job that currently finishes fast. A separate
`e2e` job depends on the `web` job's build artifact, keeping the fast feedback loop
intact for lint/type/unit checks.

### 4. pb-api unit tests using axum test utilities

pb-api route handlers delegate to `pb-service` traits. Unit tests will construct
handlers with mock service implementations (the traits already support this) and
use `axum::test::TestClient` or direct `tower::ServiceExt::oneshot` calls. This
avoids spinning up a full server.

### 5. Hook tests with renderHook from Testing Library

Custom hooks will be tested with `@testing-library/react`'s `renderHook` utility.
This aligns with the existing test patterns and avoids adding new dependencies.
Hooks that use TanStack Query will be wrapped in a `QueryClientProvider`.

### 6. Coverage via Vitest's built-in v8 provider

Vitest supports v8 coverage natively. Adding `coverage.provider: 'v8'` and
`coverage.reporter: ['text', 'lcov']` to the Vitest config enables both terminal
output and CI-compatible LCOV reports without adding Istanbul or a separate tool.

## Risks / Trade-offs

- **CodeQL adds ~5-10 min to CI** → Runs as a separate workflow; does not block
  the main CI pipeline. Developers see results as a non-blocking check.
- **E2E tests are flaky by nature** → Start with the 2 existing smoke tests only.
  Run with `--retries 1` in CI. Do not add `forbidOnly` enforcement yet.
- **Mock-heavy pb-api tests may drift from real behavior** → Integration tests
  remain the source of truth. Unit tests focus on error paths and input validation
  that integration tests don't cover.
- **Coverage reporting without thresholds is informational only** → Intentional.
  Enforcing thresholds on a codebase mid-build creates friction. Thresholds can
  be added once the team agrees on a baseline.
