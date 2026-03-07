# Quant Workstation Platform -- Tasks

## Phase 1: Spec The Current Workstation

- [x] Add a new active OpenSpec change for the quant workstation platform
- [x] Write proposal, design, and tasks documents for the serving and frontend architecture
- [x] Define `live-market-observability` capability scenarios
- [x] Define `replay-integrity-workbench` capability scenarios
- [x] Define `execution-observability-console` capability scenarios
- [x] Define `query-workbench` capability scenarios
- [x] Keep the v1 product read-only in all docs and interfaces

## Phase 2: Spec Future Module Boundaries

- [x] Define `live-trading-control-plane` capability boundaries for authenticated trading workflows
- [x] Define `backtest-workbench` capability boundaries for replay-backed research workflows
- [x] Define `strategy-control-plane` capability boundaries for strategy lifecycle and tuning workflows
- [x] Document that future control-plane features require auth, risk, audit, and environment scoping before implementation

## Phase 3: Serving Layer Implementation

- [x] Add a Rust API crate that exposes the first versioned REST endpoints for the workstation
- [x] Add a server-side live read model fed from the ingest or persisted-record path without coupling the browser to ingest internals
- [ ] Add WebSocket order book streaming for workstation clients
- [ ] Add guarded query adapters for ClickHouse and local DuckDB workflows
- [x] Add stable response types for feed status, live book, and replay results
- [ ] Add stable response types for integrity, latency, execution, and query results
- [x] Add API tests for current route contracts and replay semantics
- [ ] Add API tests for deferred integrity, execution, query, and streaming routes

## Phase 4: Web Application Implementation

- [x] Add a separate TypeScript SPA for workstation views
- [x] Implement Live Feed and Replay Lab routes against the current `pb-api` contracts
- [ ] Implement Integrity, Latency, Execution Timeline, and Query Workbench routes
- [x] Add seeded sample data so the UI can be reviewed without live infrastructure
- [x] Add web unit tests and smoke coverage for Live Feed and Replay Lab

## Phase 4.5: Trader-Grade Performance Hardening

- [x] Add adaptive foreground/background polling with stale request cancellation
- [x] Split the shipped routes into lazy-loaded bundles
- [ ] Replace HTTP polling with WebSocket order book streaming once the backend route exists
- [ ] Add virtualization and render throttling for deeper books and future streams
- [ ] Add frontend performance budgets and production profiling hooks

## Phase 5: CI, Docs, And Packaging

- [ ] Add frontend lint, typecheck, test, and build jobs to CI
- [x] Document the serving topology and the separation between read-only workstation surfaces and future control-plane surfaces
- [x] Add local development instructions for running the API locally
- [ ] Add local development instructions for running the API and web app together
- [ ] Define later deployment packaging for the Rust API and static web assets

---

## Implementation Plan for Remaining Tasks

### Dependency graph

```text
Phase 3.3 (response types)
  ├─> Phase 3.4 (API tests)
  └─> Phase 4 (frontend routes)
       └─> Phase 4.5.2 (virtualization)
Phase 3.2 (query adapters)
  └─> Phase 3.3 (query response types use adapters)
Phase 3.1 (WebSocket streaming backend)
  └─> Phase 4.5.1 (frontend WebSocket replacement)
Phase 5.1 (frontend CI) ── independent
Phase 5.2 (dev docs) ── independent
Phase 5.3 (deployment packaging) ── independent, last
```

### Suggested implementation order

| Step | Task | Phase | Depends on |
|------|------|-------|------------|
| 1 | Frontend CI | 5 | nothing |
| 2 | Response types for integrity, latency, execution, query | 3 | nothing |
| 3 | Integrity and execution timeline API routes | 3 | step 2 |
| 4 | Guarded query adapters (ClickHouse + DuckDB) | 3 | nothing |
| 5 | Query workbench API route | 3 | steps 2, 4 |
| 6 | WebSocket order book streaming backend | 3 | nothing |
| 7 | API tests for integrity, execution, query, streaming routes | 3 | steps 3, 5, 6 |
| 8 | Integrity, Latency, Execution Timeline, Query Workbench frontend | 4 | steps 3, 5 |
| 9 | Replace HTTP polling with WebSocket streaming in frontend | 4.5 | step 6 |
| 10 | Virtualization and render throttling | 4.5 | step 9 |
| 11 | Frontend performance budgets and profiling hooks | 4.5 | nothing |
| 12 | Local dev docs for API + web together | 5 | steps 3, 8 |
| 13 | Deployment packaging for API + static web assets | 5 | steps 8, 9 |

---

### Step 1: Add frontend lint, typecheck, test, and build jobs to CI

**Phase**: 5  
**Depends on**: nothing  
**Risk**: low

This is independent work and can land first without blocking anything else.

**Files to change**:
- `.github/workflows/ci.yml` — add a new `web` job

**Implementation**:

Add a `web` job to the existing CI workflow. The job should:

1. Check out the repo.
2. Set up Node (use `actions/setup-node@v4` with `node-version: 22` and
   `cache: npm` with `cache-dependency-path: web/package-lock.json`).
3. Run `npm ci` in `web/`.
4. Run the following checks sequentially:
   - `npm run lint` — ESLint
   - `npx tsc -b` — TypeScript type checking
   - `npm run test` — Vitest unit tests
   - `npm run build` — Vite production build

All four checks must pass for the CI job to succeed. The job should run on
`ubuntu-latest` alongside the existing Rust jobs, not gated behind them.

**Validation**: push a branch and confirm the workflow triggers with the new
`web` job alongside the existing `check`, `test`, `clippy`, `fmt`, `audit`,
and `miri` jobs.

---

### Step 2: Add stable response types for integrity, latency, execution, and query results

**Phase**: 3  
**Depends on**: nothing  
**Risk**: low

Define the DTO structs and serde representations that the new API routes will
return. These types should follow the same conventions as the existing DTOs in
`crates/pb-api/src/dto.rs`: `Serialize + Deserialize`, fixed-point string
encoding for prices/sizes, microsecond timestamps.

**Files to change**:
- `crates/pb-api/src/dto.rs` — add new response structs
- `crates/pb-api/src/lib.rs` — re-export new public types

**New types to add**:

```text
IntegritySummaryResponse
├── asset_id: String
├── start_us: u64
├── end_us: u64
├── total_book_events: u64
├── total_ingest_events: u64
├── reconnect_count: u32
├── gap_count: u32
├── stale_snapshot_skip_count: u32
├── validation_count: u32
├── validations_matched: u32
├── validations_mismatched: u32
├── completeness: CompletenessLabel      (enum: Complete | BestEffort)
└── continuity_events: Vec<ContinuityWarning>

LatencySummaryResponse
├── window_start_us: u64
├── window_end_us: u64
├── sample_count: u64
├── ws_latency_p50_us: Option<u64>
├── ws_latency_p99_us: Option<u64>
├── processing_p50_us: Option<u64>
├── processing_p99_us: Option<u64>
├── storage_flush_p50_us: Option<u64>
└── storage_flush_p99_us: Option<u64>

ExecutionTimelineResponse
├── events: Vec<ExecutionEventView>
└── total_count: u64

ExecutionEventView
├── event_timestamp_us: u64
├── asset_id: Option<String>
├── order_id: String
├── client_order_id: Option<String>
├── venue_order_id: Option<String>
├── kind: String
├── side: Option<String>
├── price: Option<FixedPrice>
├── size: Option<FixedSize>
├── status: Option<String>
├── reason: Option<String>
└── latency: LatencyTraceView

LatencyTraceView
├── market_data_recv_us: Option<u64>
├── normalization_done_us: Option<u64>
├── strategy_decision_us: Option<u64>
├── order_submit_us: Option<u64>
├── exchange_ack_us: Option<u64>
└── exchange_fill_us: Option<u64>

QueryResultResponse
├── columns: Vec<QueryColumn>
├── rows: Vec<Vec<serde_json::Value>>
├── row_count: u64
├── truncated: bool
└── execution_time_ms: u64

QueryColumn
├── name: String
└── data_type: String

DatasetSchemaResponse
├── datasets: Vec<DatasetInfo>

DatasetInfo
├── name: String
├── description: String
└── columns: Vec<QueryColumn>
```

Also add a `CompletenessLabel` enum (`Complete`, `BestEffort`) for integrity
responses.

**Validation**: `cargo check` passes. Existing tests still pass unchanged.

---

### Step 3: Add integrity and execution timeline API routes

**Phase**: 3  
**Depends on**: step 2  
**Risk**: medium

Wire up the new HTTP handlers in `crates/pb-api/src/server.rs` for integrity
summary and execution timeline queries. Both routes read from the existing
`ParquetReader` through `pb-replay` (Parquet-first, same as replay).

**Files to change**:
- `crates/pb-api/src/server.rs` — add route registrations and handler functions
- `crates/pb-api/Cargo.toml` — no new dependencies needed; `pb-replay` already
  provides `EventReader` with `read_validations`, `read_execution_events`, and
  `read_market_data`

**New routes**:

`GET /api/v1/integrity/summary`

Query params: `asset_id`, `start_us`, `end_us`.

Handler logic:
1. Validate params.
2. Read `market_data` (for ingest events), `validations`, and `checkpoints`
   from `ParquetReader` over the given window.
3. Count reconnects, gaps, stale skips, and validation outcomes from the
   returned events.
4. Derive `CompletenessLabel` based on the presence of any continuity
   boundaries.
5. Return `IntegritySummaryResponse`.

`GET /api/v1/execution/orders`

Query params: `order_id` (optional), `asset_id` (optional), `start_us`,
`end_us`, `limit` (optional, default 100, max 1000).

Handler logic:
1. Validate params.
2. Read `execution_events` from `ParquetReader`. Use `order_id` filter when
   provided.
3. Post-filter by `asset_id` if provided.
4. Truncate to `limit`.
5. Map each `ExecutionEvent` to `ExecutionEventView`.
6. Return `ExecutionTimelineResponse`.

Both routes should use the existing `validate_depth`-style validation pattern
and map `ReplayError` to `ApiError` with the existing `map_replay_error`
helper.

**Validation**: `cargo check` passes. Manually test with `curl` against
Parquet data on disk, or verify via the new API tests in step 7.

---

### Step 4: Add guarded query adapters for ClickHouse and local DuckDB workflows

**Phase**: 3  
**Depends on**: nothing  
**Risk**: medium-high (new dependency: `duckdb`)

Create a query adapter abstraction that supports both ClickHouse and DuckDB
backends with read-only, row-limited, time-bounded query execution.

**Files to change**:
- `crates/pb-api/Cargo.toml` — add `duckdb` (with `bundled` feature) and
  `clickhouse` as optional dependencies behind a `query-workbench` feature flag
- `crates/pb-api/src/query_adapter.rs` — new module
- `crates/pb-api/src/lib.rs` — conditionally export query adapter module

**Design**:

```rust
pub struct QueryGuard {
    pub max_rows: usize,           // default 10_000
    pub timeout: Duration,         // default 30s
    pub enabled: bool,             // default false
}

#[async_trait]
pub trait QueryAdapter: Send + Sync {
    async fn execute_sql(
        &self,
        sql: &str,
        guard: &QueryGuard,
    ) -> Result<QueryResultResponse, ApiError>;

    async fn list_datasets(&self) -> Result<DatasetSchemaResponse, ApiError>;
}
```

Implement `ClickHouseQueryAdapter` wrapping the existing `clickhouse::Client`
and `DuckDbQueryAdapter` wrapping `duckdb::Connection` opened in read-only mode
over the Parquet base path.

Key safeguards:
- Reject any SQL containing write keywords (`INSERT`, `UPDATE`, `DELETE`,
  `DROP`, `ALTER`, `CREATE`, `TRUNCATE`) at the adapter level.
- Apply `LIMIT` injection if not already present and `max_rows` is set.
- Apply `tokio::time::timeout` around query execution.
- Return `ApiError::BadRequest` for policy violations, `ApiError::Internal`
  for backend errors.

The `enabled` flag defaults to `false`. The `serve-api` runtime only activates
query execution when an explicit config flag (`api.query_workbench_enabled`)
is set to `true`.

`DuckDbQueryAdapter::list_datasets` should read the Parquet directory structure
and infer schemas via `DESCRIBE SELECT * FROM read_parquet(...)`. The
`ClickHouseQueryAdapter` variant queries `information_schema.columns`.

**Validation**: unit tests within the module using DuckDB over temp Parquet
files. `cargo check --features query-workbench` passes.

---

### Step 5: Add query workbench API route

**Phase**: 3  
**Depends on**: steps 2, 4  
**Risk**: medium

**Files to change**:
- `crates/pb-api/src/server.rs` — add route registrations
- `crates/pb-api/src/server.rs` — add handler functions

**New routes**:

`GET /api/v1/query/datasets`

Handler: delegate to `QueryAdapter::list_datasets()`. Returns
`DatasetSchemaResponse`. Returns `503` if query workbench is disabled.

`POST /api/v1/query/sql`

Request body (JSON):
```json
{ "sql": "SELECT ...", "max_rows": 100 }
```

Handler logic:
1. Check if query workbench is enabled; return `503` if not.
2. Parse request body.
3. Apply `QueryGuard` from config merged with per-request `max_rows`.
4. Delegate to `QueryAdapter::execute_sql()`.
5. Return `QueryResultResponse`.

The `AppState` should gain an `Option<Arc<dyn QueryAdapter>>` field. When
`None`, both routes return `503`.

**Router changes**: use `routing::post` for the SQL endpoint. The router
registration is conditional on the `query-workbench` feature flag so it
compiles cleanly without it.

**Config changes**:
- `config/default.toml` — add `[api]` keys: `query_workbench_enabled = false`,
  `query_max_rows = 10000`, `query_timeout_secs = 30`.
- `crates/pb-bin/src/commands/serve_api.rs` — read the new config keys and
  construct the query adapter if enabled.

**Validation**: `cargo check` with and without `--features query-workbench`.
API tests in step 7 cover the route contracts.

---

### Step 6: Add WebSocket order book streaming for workstation clients

**Phase**: 3  
**Depends on**: nothing  
**Risk**: medium-high

**Files to change**:
- `crates/pb-api/Cargo.toml` — add `tokio-tungstenite` (already a workspace
  dep from `pb-feed`)
- `crates/pb-api/src/streaming.rs` — new module
- `crates/pb-api/src/live_state.rs` — add a broadcast channel for book updates
- `crates/pb-api/src/server.rs` — register the WebSocket upgrade route
- `crates/pb-api/src/lib.rs` — export streaming types

**Design**:

The live read model currently writes to an internal `RwLock<LiveState>`. To
support streaming, add a `tokio::sync::broadcast::Sender<BookUpdateMessage>`
alongside the existing state. Whenever `apply_record` processes a delta or
materializes a snapshot, it also sends a `BookUpdateMessage` on the broadcast
channel.

```rust
pub struct BookUpdateMessage {
    pub asset_id: String,
    pub sequence: u64,
    pub last_update_us: u64,
    pub bids: Vec<PriceLevelView>,
    pub asks: Vec<PriceLevelView>,
    pub mid_price: Option<f64>,
    pub spread: Option<f64>,
}
```

Route: `GET /api/v1/streams/orderbook?asset_id=<ID>`

Handler:
1. Validate `asset_id` is an active asset.
2. Upgrade the HTTP connection to a WebSocket via axum's `WebSocket` extractor.
3. Subscribe to the broadcast channel.
4. Send an initial full snapshot message on connect.
5. Filter subsequent broadcast messages to the requested `asset_id` and forward
   as JSON text frames.
6. Handle client ping/pong and graceful close.
7. Drop the broadcast receiver on disconnect.

The broadcast channel should have a capacity of 256 messages. If a slow
consumer falls behind, it receives a `Lagged` error and should be sent a
fresh full snapshot to re-sync.

**Validation**: manual test using `websocat` or a small script. Automated
tests in step 7.

---

### Step 7: Add API tests for deferred integrity, execution, query, and streaming routes

**Phase**: 3  
**Depends on**: steps 3, 5, 6  
**Risk**: low

**Files to change**:
- `crates/pb-api/src/server.rs` — add tests in the existing `#[cfg(test)]`
  module

**Tests to add**:

Integrity:
- `integrity_summary_returns_counts_from_parquet` — write sample ingest and
  validation records to temp Parquet, request the summary, assert counts and
  completeness label.
- `integrity_summary_returns_400_for_missing_params` — omit required params,
  assert 400.

Execution:
- `execution_orders_returns_timeline_from_parquet` — write sample execution
  events to temp Parquet, request the timeline, assert events are ordered and
  view fields are correct.
- `execution_orders_filters_by_order_id` — write events for multiple orders,
  filter by one, assert only matching events returned.

Query (behind `#[cfg(feature = "query-workbench")]`):
- `query_sql_returns_503_when_disabled` — default config, assert 503.
- `query_sql_executes_read_only_query` — enable DuckDB adapter over temp
  Parquet, submit `SELECT count(*) FROM read_parquet(...)`, assert result.
- `query_sql_rejects_write_statement` — submit `DROP TABLE`, assert 400.

Streaming:
- `ws_orderbook_receives_initial_snapshot` — connect via WebSocket test client,
  assert first message is a full snapshot.
- `ws_orderbook_receives_delta_updates` — apply a delta record after connect,
  assert the client receives the update message.

For WebSocket tests, use `tokio-tungstenite` as a test client connecting to
the test server via `TcpListener::bind("127.0.0.1:0")`.

**Validation**: `cargo test -p pb-api` passes (including with
`--features query-workbench` for query tests).

---

### Step 8: Implement Integrity, Latency, Execution Timeline, and Query Workbench frontend routes

**Phase**: 4  
**Depends on**: steps 3, 5  
**Risk**: medium

**Files to change**:
- `web/src/types.ts` — add TypeScript interfaces mirroring the new backend
  response types
- `web/src/client.ts` — add client methods for the new endpoints
- `web/src/demoData.ts` — add seeded fixtures for the new surfaces
- `web/src/IntegrityPage.tsx` — new page component
- `web/src/LatencyPage.tsx` — new page component
- `web/src/ExecutionTimelinePage.tsx` — new page component
- `web/src/QueryWorkbenchPage.tsx` — new page component
- `web/src/App.tsx` — add lazy-loaded routes and nav links for all four new
  pages
- `web/src/App.css` — styles for new page components
- `web/src/App.test.tsx` — add smoke tests for new routes using demo data
- `web/src/ui.tsx` — add shared UI components if needed (e.g., timeline
  visualization, schema browser)

**New TypeScript types** (in `types.ts`):

```typescript
interface IntegritySummaryResponse { ... }
interface LatencySummaryResponse { ... }
interface ExecutionTimelineResponse { ... }
interface ExecutionEventView { ... }
interface LatencyTraceView { ... }
interface QueryResultResponse { ... }
interface QueryColumn { ... }
interface DatasetSchemaResponse { ... }
interface DatasetInfo { ... }
```

**New `WorkstationClient` methods**:

```typescript
getIntegritySummary(assetId, startUs, endUs, options?): Promise<IntegritySummaryResponse>
getLatencySummary(windowStartUs, windowEndUs, options?): Promise<LatencySummaryResponse>
getExecutionTimeline(params, options?): Promise<ExecutionTimelineResponse>
getDatasetSchemas(options?): Promise<DatasetSchemaResponse>
executeQuery(sql, maxRows?, options?): Promise<QueryResultResponse>
```

**Page designs**:

`IntegrityPage`:
- Form: asset ID, time window.
- Results: event counts, reconnect/gap/stale counts, validation match ratio,
  completeness label (Complete vs Best-Effort), continuity event list.
- Uses adaptive polling from `useAdaptivePolling` for auto-refresh.

`LatencyPage`:
- Form: time window.
- Results: percentile summary cards for WS latency, processing, and storage
  flush timing.
- Reserved for future metrics-backed summaries; initial version uses demo data
  or returns a "no data" state when the backend does not yet expose this route.

`ExecutionTimelinePage`:
- Form: order ID (optional), asset ID (optional), time window.
- Results: chronological table of execution events with kind, side, price,
  size, status, and latency trace expansion.

`QueryWorkbenchPage`:
- Schema browser panel listing datasets and their columns.
- SQL editor textarea with canned example buttons.
- Results table with column headers and rows.
- Disabled-by-default banner when the backend returns 503.

**Nav changes in `App.tsx`**:
- Add `NavLink` entries for `/integrity`, `/latency`, `/execution-timeline`,
  and `/query-workbench`.
- Lazy-load all four page components.
- Update the sidebar to move these from "Deferred" to "Current shipped
  surfaces."

**Validation**: `npm run lint && npx tsc -b && npm run test && npm run build`
in `web/`. Manual review in browser with demo data mode.

---

### Step 9: Replace HTTP polling with WebSocket order book streaming in frontend

**Phase**: 4.5  
**Depends on**: step 6  
**Risk**: medium

**Files to change**:
- `web/src/useOrderBookStream.ts` — new hook
- `web/src/LiveFeedPage.tsx` — replace polling with streaming for live
  snapshots
- `web/src/types.ts` — add `BookUpdateMessage` type
- `web/src/client.ts` — add WebSocket URL construction helper
- `web/src/constants.ts` — add WebSocket reconnect config

**Design**:

Create a `useOrderBookStream(assetId)` hook that:
1. Opens a WebSocket to `/api/v1/streams/orderbook?asset_id=<ID>`.
2. Parses incoming JSON messages into `BookUpdateMessage`.
3. Exposes `{ snapshot, status, error }` state.
4. Reconnects with exponential backoff on close or error.
5. Cleans up the socket on unmount or `assetId` change.

In `LiveFeedPage`, when `sourceMode === 'api'`:
- Use `useOrderBookStream` instead of `useAdaptivePolling` for the selected
  asset's book view.
- Keep `useAdaptivePolling` for feed status and active assets (these are
  low-frequency and do not need streaming).

When `sourceMode === 'demo'`:
- Continue using the existing demo data path (no WebSocket).

**Fallback behavior**:
- If the WebSocket fails to connect or the backend returns an upgrade error
  (e.g., route does not exist), fall back to HTTP polling with a console
  warning. This keeps the frontend backward-compatible with older backend
  versions.

**Validation**: manual test with `serve-api --auto-rotate` running, confirm
WebSocket messages flow in the browser DevTools Network tab. Verify fallback
by stopping the backend and confirming the UI switches to polling.

---

### Step 10: Add virtualization and render throttling for deeper books and future streams

**Phase**: 4.5  
**Depends on**: step 9  
**Risk**: medium

**Files to change**:
- `web/package.json` — add `@tanstack/react-virtual` dependency
- `web/src/ui.tsx` — virtualize `OrderBookTable`
- `web/src/LiveFeedPage.tsx` — add render throttle for high-frequency updates
- `web/src/ExecutionTimelinePage.tsx` — virtualize the execution event list
- `web/src/QueryWorkbenchPage.tsx` — virtualize the query results table

**Implementation**:

Virtualization:
- Wrap `OrderBookTable` in a virtualized container using
  `@tanstack/react-virtual`. Only render the visible rows plus a small
  overscan buffer.
- Apply the same virtualization to the execution timeline event list and query
  results table for large result sets.

Render throttling:
- When receiving WebSocket updates, batch incoming messages and only re-render
  the order book at most once per animation frame using `requestAnimationFrame`
  debouncing.
- This prevents layout thrash when the book is updating at high frequency.

**Validation**: `npm run build` still produces a working bundle. Manually test
with a `max_depth=200` snapshot to confirm the virtualized table scrolls
smoothly. Compare bundle size before and after to confirm the virtual library
does not meaningfully inflate the bundle.

---

### Step 11: Add frontend performance budgets and production profiling hooks

**Phase**: 4.5  
**Depends on**: nothing  
**Risk**: low

**Files to change**:
- `web/vite.config.ts` — add `build.rollupOptions.output.manualChunks` for
  fine-grained chunk splitting and add `build` size warnings
- `web/package.json` — add `bundlesize` or use Vite's built-in
  `build.chunkSizeWarningLimit`
- `web/src/main.tsx` — add `reportWebVitals`-style hook for CLS, LCP, FID
  when `import.meta.env.PROD` is true
- `web/src/constants.ts` — add performance budget constants

**Implementation**:

Performance budgets:
- Set `build.chunkSizeWarningLimit` in `vite.config.ts` to 150 KB.
- Add `manualChunks` to split React, react-router-dom, and
  `@tanstack/react-virtual` into separate vendor chunks.
- If any chunk exceeds the budget, the build emits a warning (not a hard
  failure initially).

Profiling hooks:
- In production builds, measure `web-vitals` metrics (LCP, FID, CLS, TTFB)
  and log them to the console. A future iteration could send them to the
  metrics endpoint.
- Add a `performance.mark` / `performance.measure` wrapper for page transitions
  and WebSocket reconnects so they show up in browser DevTools Performance
  traces.

**Validation**: `npm run build` outputs chunk sizes. Confirm vendor chunk
splitting in the `dist/assets/` output. Confirm web vitals logging in the
browser console when running the production build via `npm run preview`.

---

### Step 12: Add local development instructions for running the API and web app together

**Phase**: 5  
**Depends on**: steps 3, 8  
**Risk**: low

**Files to change**:
- `docs/operations.md` — expand the "Workstation Web App" section
- `README.md` — add or update the quick-start section

**Content to add**:

Combined workflow:

```bash
# terminal 1 — Rust API
cargo run -- serve-api --auto-rotate

# terminal 2 — web app dev server
cd web
npm install
npm run dev
```

Open `http://127.0.0.1:4173` in the browser. The Vite dev server proxies
`/api` requests to `127.0.0.1:3000`.

Demo mode (no backend required):

```bash
cd web
npm install
npm run dev
# open http://127.0.0.1:4173/?source=demo
```

Document the env vars: `VITE_DEV_API_PROXY_TARGET`,
`VITE_API_BASE_URL`, and the query param `?source=demo`.

Add a note about port defaults (API: 3000, Metrics: 9090, Web: 4173) and how
to override them.

**Validation**: follow the instructions from a clean state and confirm the app
loads in both API and demo modes.

---

### Step 13: Define later deployment packaging for the Rust API and static web assets

**Phase**: 5  
**Depends on**: steps 8, 9  
**Risk**: low (design-only initially)

**Files to change**:
- `docs/operations.md` — add a "Deployment Packaging" section
- `Dockerfile` — extend or create a multi-stage variant for the combined
  deployment
- `openspec/changes/2026-03-07-quant-workstation-platform/design.md` — update
  the "Rollout" section

**Design to document**:

Option A: Single binary with embedded static assets.
- Use `rust-embed` or `include_dir` to embed the `web/dist/` output into the
  Rust binary at compile time.
- `serve-api` serves the SPA at `/` and the API at `/api/v1/`.
- Simplest deployment: one container, one binary.

Option B: Separate containers.
- Rust API container serves only `/api/v1/`.
- Nginx or Caddy container serves the static SPA and reverse-proxies `/api`
  to the Rust API container.
- Better for independent scaling and CDN caching of static assets.

The recommended initial packaging is Option A for simplicity, with a migration
path to Option B when traffic or team structure warrants it.

**Dockerfile changes**:
- Add a Node build stage that runs `npm ci && npm run build` in `web/`.
- Copy `web/dist/` into the Rust build stage before `cargo build --release`.
- The final image contains only the compiled binary with embedded assets.

**Validation**: `docker build` produces a working image. Run the image and
confirm both the SPA and API are reachable.
