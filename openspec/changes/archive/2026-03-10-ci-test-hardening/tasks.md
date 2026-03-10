## 1. CI Protobuf Consolidation

- [x] 1.1 Create `.github/actions/setup-protobuf/action.yml` composite action that installs protobuf-compiler
- [x] 1.2 Replace inline `sudo apt-get install -y protobuf-compiler` in check, test, and clippy jobs with `uses: ./.github/actions/setup-protobuf`

## 2. CI Security Scanning

- [x] 2.1 Create `.github/workflows/codeql.yml` workflow with Rust and TypeScript analysis on PR and push to main
- [x] 2.2 Enable GitHub secret scanning in repository settings (document the step if it requires manual action)

## 3. CI E2E Integration

- [x] 3.1 Update the `web` job in ci.yml to upload the build artifact (`web/dist/`) after `npx vite build`
- [x] 3.2 Add an `e2e` job in ci.yml that depends on `web`, downloads the build artifact, installs Playwright browsers, and runs `npx playwright test`

## 4. Rust API Test Coverage

- [x] 4.1 Add unit tests for feed status and active assets handlers in pb-api (mock BookService, verify 200 response shape)
- [x] 4.2 Add unit tests for orderbook snapshot handler error paths (non-existent asset returns 404)
- [x] 4.3 Add unit tests for replay reconstruct handler (missing/invalid query params return 400)
- [x] 4.4 Add unit tests for query SQL handler (missing body returns 400, valid query returns 200)

## 5. Web Hook Test Coverage

- [x] 5.1 Add unit tests for `useTheme` hook (toggle between dark/light, persistence across renders)
- [x] 5.2 Add unit tests for `useKeyboardShortcut` hook (callback fires on key press, suppressed when input focused)
- [x] 5.3 Add unit tests for `useOrderBookStream` hook (initial loading state, data update on message)
- [x] 5.4 Add unit tests for `useSourceMode` hook (toggle between live and demo)
- [x] 5.5 Add unit tests for `useThrottledState` hook (rapid updates throttled, final value applied)

## 6. Web Coverage Reporting

- [x] 6.1 Add `@vitest/coverage-v8` dev-dependency and configure coverage in `vite.config.ts` (provider: v8, reporters: text + lcov, exclude test files)
- [x] 6.2 Add `test:coverage` script to `web/package.json`
- [x] 6.3 Add `coverage/` to `.gitignore` in web directory

## 7. Documentation

- [x] 7.1 Document branch protection expectations (required checks list) in a section of the project README or CLAUDE.md
- [x] 7.2 Document how to run integration tests locally (`cargo test -p pb-integration-tests`) in the project README
