## Why

The CI pipeline and test infrastructure are functional and well-structured but have
gaps in security scanning, test coverage for key crates, and E2E integration. An
audit identified missing SAST tooling, untested API route handlers, custom hooks
without unit tests, and Playwright E2E tests that exist but are not wired into CI.
Addressing these now — while the surface area is still small — prevents technical
debt from compounding as the workstation grows.

## What Changes

- Add CodeQL static analysis for Rust and TypeScript to catch logic-level
  vulnerabilities on every PR.
- Enable GitHub secret scanning to prevent accidental credential commits.
- Consolidate the repeated `protobuf-compiler` installation into a reusable
  composite action, reducing CI maintenance burden.
- Wire the existing Playwright E2E tests into the CI workflow as a separate job.
- Add unit tests for `pb-api` HTTP route handlers covering error paths (invalid
  asset IDs, malformed replay parameters, missing query bodies).
- Add unit tests for untested web custom hooks (`useTheme`, `useKeyboardShortcut`,
  `useOrderBookStream`, `useSourceMode`, `useThrottledState`).
- Add Vitest coverage reporting so test gaps are visible on every PR.
- Document branch protection expectations and integration test setup in the repo.

## Capabilities

### New Capabilities

- `ci-security-scanning`: CodeQL SAST analysis and GitHub secret scanning
  configuration for Rust and TypeScript codebases.
- `ci-e2e-integration`: Playwright E2E tests wired into the CI workflow with
  proper build artifact caching and failure reporting.
- `ci-protobuf-consolidation`: Reusable composite action for protobuf-compiler
  setup, eliminating repeated installation across CI jobs.
- `rust-api-test-coverage`: Unit tests for pb-api route handlers covering error
  paths, invalid inputs, and edge cases.
- `web-hook-test-coverage`: Unit tests for custom React hooks ensuring correct
  behavior for theme switching, keyboard shortcuts, WebSocket streaming, source
  mode toggling, and throttled state updates.
- `web-coverage-reporting`: Vitest coverage configuration with threshold
  enforcement and PR-visible reports.

### Modified Capabilities

(none — these changes add new CI jobs and test files without altering existing
spec-level behavior)

## Impact

- **`.github/workflows/`**: New `codeql.yml` workflow. Modified `ci.yml` with
  consolidated protobuf action and new E2E job.
- **`.github/actions/setup-protobuf/`**: New composite action.
- **`crates/pb-api/src/`**: New `#[cfg(test)]` modules for route handler tests.
- **`web/src/**/__tests__/`**: New test files for custom hooks.
- **`web/vite.config.ts`**: Coverage configuration added to Vitest settings.
- **`web/package.json`**: Coverage script added.
- **`docs/`**: Branch protection and integration test documentation.
- No runtime behavior changes. No API contract changes. No dependency additions
  to production code.
