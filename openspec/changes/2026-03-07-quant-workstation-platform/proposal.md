# Quant Workstation Platform

## Why

`poly-book` already has the core subsystems that matter for a credible trading
systems project:

- live market-data ingestion
- split storage for market and execution data
- historical replay with explicit ordering modes
- continuity and validation metadata
- Prometheus metrics and operational hooks

What it does not have is a serving and inspection layer that makes those
capabilities visible to operators, researchers, or reviewers. A frontend added
without clear boundaries would risk looking like a generic dashboard project.

The better direction is a read-only workstation that highlights the qualities
top quant firms care about:

- determinism
- observability
- data quality
- replay correctness
- latency attribution
- separation between market-data truth, execution truth, and control surfaces

## What Changes

- Add a frontend-serving platform based on a Rust API layer plus a separate web
  application.
- Define the initial product as a read-only workstation for live feed health,
  replay, integrity, latency, execution-state inspection, and safe ad hoc
  queries.
- Keep the current repository honest about scope: v1 does not claim live order
  routing, authentication, or risk controls.
- Define future capability specs for live trading, backtesting, and strategy
  control so later work has explicit boundaries before implementation begins.

## Capabilities

### New Capabilities

- `live-market-observability`: live book views, feed health, freshness, and
  continuity signals.
- `replay-integrity-workbench`: replay inspection with continuity metadata,
  checkpoint usage, and validation visibility.
- `execution-observability-console`: read-only inspection of order lifecycle
  state and latency traces.
- `query-workbench`: safe read-only query access to the split datasets.
- `live-trading-control-plane`: future authenticated trading workflows with
  explicit safety boundaries.
- `backtest-workbench`: future replay-backed research workflows with explicit
  data-quality assumptions.
- `strategy-control-plane`: future strategy deployment, state transition, and
  tuning workflows.

### Modified Capabilities

- `metrics-server`: grows from scrape-only observability into a source for
  workstation latency and health views.
- `backtest-credible-replay`: gains a serving surface oriented around operator
  and researcher inspection.
- `live-execution-readiness`: gains a read-only presentation layer for
  execution state and latency breakdowns.

## Impact

- New active OpenSpec change for frontend and serving architecture.
- New Rust API surface for browser-facing REST and WebSocket endpoints.
- New web application and CI checks for frontend validation.
- Later deployment work will need separate packaging for API and static web
  assets.
- The change introduces public-facing interfaces, so the workstation must be
  explicit about what is shipped now versus what remains future scope.
