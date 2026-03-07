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

## Phase 5: CI, Docs, And Packaging

- [ ] Add frontend lint, typecheck, test, and build jobs to CI
- [x] Document the serving topology and the separation between read-only workstation surfaces and future control-plane surfaces
- [x] Add local development instructions for running the API locally
- [ ] Add local development instructions for running the API and web app together
- [ ] Define later deployment packaging for the Rust API and static web assets
