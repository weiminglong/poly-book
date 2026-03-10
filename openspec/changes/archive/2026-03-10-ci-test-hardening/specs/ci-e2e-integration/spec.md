## ADDED Requirements

### Requirement: Playwright E2E job in CI
The CI workflow SHALL include a separate `e2e` job that runs Playwright end-to-end
tests against a production build of the web application.

#### Scenario: E2E tests run after web build succeeds
- **WHEN** the `web` job completes successfully
- **THEN** the `e2e` job starts
- **THEN** it installs Playwright browsers, serves the built application, and runs
  all Playwright test files

#### Scenario: E2E job uses build artifact from web job
- **WHEN** the `e2e` job starts
- **THEN** it downloads the build artifact produced by the `web` job
- **THEN** it does not rebuild the application

#### Scenario: E2E failure does not block unrelated jobs
- **WHEN** the `e2e` job fails
- **THEN** other CI jobs (check, test, clippy, fmt, audit, fuzz, miri) are
  unaffected and report their own status independently
