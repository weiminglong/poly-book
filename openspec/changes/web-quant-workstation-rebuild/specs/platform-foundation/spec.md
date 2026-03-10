## ADDED Requirements

### Requirement: Feature-based project structure
The web source tree SHALL be organized into `app/`, `features/`, `shared/`, and `types/` directories. Each feature directory SHALL contain its page component, feature-specific components, hooks, and a barrel `index.ts`. Shared code SHALL live in `shared/` and MUST be used by at least two features before being promoted there.

#### Scenario: New feature added
- **WHEN** a developer creates a new feature (e.g., `features/query/`)
- **THEN** the feature directory contains its page component, any feature-specific components, hooks, and an `index.ts` barrel export, without modifying other feature directories

#### Scenario: Cross-feature utility extraction
- **WHEN** a utility function is used by two or more features
- **THEN** the function lives in `shared/lib/` or `shared/hooks/` and is imported from there by both features

### Requirement: TanStack Query data layer
All HTTP API data fetching SHALL use TanStack Query hooks. Each API endpoint SHALL have a corresponding query hook factory in `shared/api/queries.ts`. The `QueryClient` SHALL be configured with sensible defaults: `staleTime` of 5 seconds for live data, `gcTime` of 5 minutes, and `retry` count of 2.

#### Scenario: Feed status polling
- **WHEN** the Live Feed page is mounted and visible
- **THEN** feed status data is fetched via a TanStack Query hook with `refetchInterval` matching the foreground polling cadence (1 second)
- **AND** switching the browser tab to background automatically reduces the refetch interval to the background cadence (5 seconds)

#### Scenario: Query deduplication
- **WHEN** two components on the same page request the same API endpoint with the same parameters
- **THEN** only one HTTP request is made, and both components receive the same cached data

#### Scenario: Stale-while-revalidate
- **WHEN** cached data exists for an API endpoint and a refetch is triggered
- **THEN** the stale data is displayed immediately while the fresh data loads in the background

### Requirement: Zod runtime validation
Every API response SHALL be validated against a Zod schema before entering the TanStack Query cache. The Zod schemas SHALL be defined in `shared/api/schemas.ts` and SHALL mirror the TypeScript types from the `pb-api` contract. The validated output SHALL be the source of truth for TypeScript types (inferred from Zod via `z.infer`).

#### Scenario: Valid API response
- **WHEN** the backend returns a response that conforms to the expected schema
- **THEN** the data passes Zod validation and enters the query cache as a fully typed object

#### Scenario: Malformed API response
- **WHEN** the backend returns a response missing a required field or with an incorrect type
- **THEN** the Zod parse throws a `ZodError` which is caught by TanStack Query's error handling
- **AND** the error is displayed to the user via an error banner without crashing the app

#### Scenario: Schema round-trip with demo data
- **WHEN** demo fixture data is loaded
- **THEN** every demo fixture SHALL pass Zod validation, confirming that demo data stays in sync with the schema

### Requirement: Error boundaries
The application SHALL include React error boundaries at two levels: (1) a root-level boundary wrapping the entire app that shows a "something went wrong" fallback with a reload button, and (2) a route-level boundary wrapping each page's content area that isolates page-level crashes without affecting the navigation shell.

#### Scenario: Page component throws during render
- **WHEN** a page component throws an error during rendering
- **THEN** the route-level error boundary catches the error and displays an error message with a "try again" button within the page content area
- **AND** the navigation bar and app shell remain functional

#### Scenario: Unrecoverable error
- **WHEN** the app shell itself throws an error (e.g., routing failure)
- **THEN** the root-level error boundary catches the error and displays a full-page error screen with a "reload page" button

### Requirement: Biome for linting and formatting
The project SHALL use Biome as the sole linter and formatter, replacing ESLint and any separate formatting tool. The `biome.json` configuration SHALL enforce consistent code style across all `.ts` and `.tsx` files. The `lint` script in `package.json` SHALL invoke `biome check`.

#### Scenario: Lint check passes
- **WHEN** a developer runs `npm run lint`
- **THEN** Biome checks all TypeScript and TSX files for lint violations and formatting issues in a single pass

#### Scenario: CI validation
- **WHEN** the CI pipeline runs the web validation step
- **THEN** `biome check` runs and fails the build on any lint error or formatting violation

### Requirement: Test infrastructure
The project SHALL support three tiers of testing: unit tests (Vitest), component tests (Vitest + Testing Library), and end-to-end tests (Playwright). Test files SHALL be colocated with the code they test (e.g., `useOrderBookStream.test.ts` next to `useOrderBookStream.ts`). The `npm test` script SHALL run unit and component tests. E2E tests SHALL be runnable via a separate `npm run test:e2e` script.

#### Scenario: Unit test for Zod schema
- **WHEN** a developer writes a Zod schema for `FeedStatusResponse`
- **THEN** a corresponding test validates that valid demo data parses successfully and that intentionally malformed data throws a `ZodError`

#### Scenario: Component test for error banner
- **WHEN** a component test renders an `ErrorBanner` with a message prop
- **THEN** the rendered output contains the error message text and has `role="alert"` for accessibility

#### Scenario: E2E test for demo mode navigation
- **WHEN** a Playwright test loads the app with `?source=demo`
- **THEN** the test navigates through all pages (Live Feed, Orderbook, Replay, Execution, Integrity, Query) and verifies that each page renders demo data without errors

### Requirement: Demo data lazy loading
Demo fixture data SHALL be loaded lazily via dynamic `import()` only when `sourceMode === 'demo'`. The demo data module SHALL NOT be included in the initial bundle when the app starts in `api` mode.

#### Scenario: API mode bundle
- **WHEN** the app is built and loaded in `api` mode
- **THEN** the demo data chunk is not downloaded or parsed

#### Scenario: Switching to demo mode
- **WHEN** the user toggles from `api` to `demo` mode
- **THEN** the demo data chunk is fetched on demand, and the UI displays demo fixtures after a brief loading state
