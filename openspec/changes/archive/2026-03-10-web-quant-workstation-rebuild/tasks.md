## 1. Platform Foundation — Tooling & Structure

- [x] 1.1 Restructure `web/src/` into `app/`, `features/`, `shared/`, `types/` directories with barrel `index.ts` files
- [x] 1.2 Replace ESLint with Biome: remove ESLint deps, add `@biomejs/biome`, create `biome.json` config, update `package.json` lint script
- [x] 1.3 Install and configure Tailwind CSS v4: add `@tailwindcss/vite`, configure `@theme` tokens (colors, spacing, radii, shadows), remove `App.css` and `index.css`
- [x] 1.4 Install TanStack Query v5: add `@tanstack/react-query` and `@tanstack/react-query-devtools`, create `QueryClient` provider in `app/providers.tsx`
- [x] 1.5 Install Zod: add `zod`, create `shared/api/schemas.ts` with Zod schemas for all API response types (FeedStatus, ActiveAsset, OrderBookSnapshot, Replay, Integrity, Execution, Query)
- [x] 1.6 Derive TypeScript types from Zod schemas using `z.infer<>`, replace manual `types.ts` with inferred types
- [x] 1.7 Rewrite `shared/api/client.ts`: `fetchJson` validates responses through Zod schemas before returning
- [x] 1.8 Create TanStack Query hook factories in `shared/api/queries.ts` for all API endpoints (feed status, active assets, orderbook snapshot, replay, integrity, execution, query datasets, query SQL)
- [x] 1.9 Implement demo data provider in `shared/api/demo.ts` with lazy-loaded fixtures via dynamic `import()`
- [x] 1.10 Create root-level and route-level React error boundaries in `app/error-boundary.tsx`
- [x] 1.11 Set up app shell in `app/App.tsx`: providers (QueryClient, theme, density), error boundaries, nav bar, lazy-loaded routes
- [x] 1.12 Install Playwright: add `@playwright/test`, create `playwright.config.ts`, add `test:e2e` script to `package.json`
- [x] 1.13 Write Zod schema round-trip tests: verify all demo fixtures pass schema validation
- [x] 1.14 Verify build: `npm run lint && npx tsc -b && npm test && npm run build` passes with <200KB initial JS gzipped

## 2. Design System — Tokens, Themes, Components

- [x] 2.1 Define CSS custom property tokens via Tailwind `@theme`: color palette (background, foreground, muted, accent, destructive, warning, success), spacing scale, font sizes, border radii, shadows
- [x] 2.2 Implement dark and light theme CSS using custom properties, with `localStorage` persistence and `<html>` class toggling
- [x] 2.3 Implement density modes (compact, comfortable, spacious) via CSS custom properties on a root class, with `localStorage` persistence
- [x] 2.4 Install Radix UI primitives: `@radix-ui/react-dialog`, `@radix-ui/react-tooltip`, `@radix-ui/react-select`, `@radix-ui/react-popover`
- [x] 2.5 Build `shared/components/card.tsx` — Card with optional header, toolbar, dense variant
- [x] 2.6 Build `shared/components/metric-card.tsx` — MetricCard with label/value, inheriting density tokens
- [x] 2.7 Build `shared/components/badge.tsx` — Badge with variants (success, warning, error, neutral)
- [x] 2.8 Build `shared/components/button.tsx` — Button with variants (primary, secondary, ghost) and size props
- [x] 2.9 Build `shared/components/input.tsx` and `shared/components/select.tsx` — Form controls with consistent styling
- [x] 2.10 Build `shared/components/data-table.tsx` — Sortable, paginated table using `@tanstack/react-table`
- [x] 2.11 Build `shared/components/error-banner.tsx` — Error display with title, message, hint, role="alert"
- [x] 2.12 Build `shared/components/skeleton.tsx` — Loading skeleton with pulse animation
- [x] 2.13 Build `shared/components/tooltip.tsx` — Tooltip using Radix Tooltip primitive
- [x] 2.14 Build `shared/components/dialog.tsx` — Dialog using Radix Dialog primitive with focus management
- [x] 2.15 Add `prefers-reduced-motion` media query to disable transitions globally
- [x] 2.16 Write component tests for Card, Badge, Button, DataTable, ErrorBanner using Testing Library

## 3. Market Data Views — Ops Dashboard & Orderbook

- [x] 3.1 Build `features/live-feed/LiveFeedPage.tsx` — ops dashboard using TanStack Query hooks: connection hero card, feed health panel, asset grid, transport indicator
- [x] 3.2 Port `useOrderBookStream` to `shared/hooks/use-orderbook-stream.ts`, update to write into TanStack Query cache via `queryClient.setQueryData`
- [x] 3.3 Port `useThrottledState` to `shared/hooks/use-throttled-state.ts`
- [x] 3.4 Build `features/orderbook/OrderbookPage.tsx` — dedicated orderbook viewer page with asset selector and depth controls
- [x] 3.5 Build `features/orderbook/components/price-ladder.tsx` — price ladder with horizontal size bars proportional to max level size, green bids, red asks
- [x] 3.6 Build `features/orderbook/components/depth-chart.tsx` — cumulative bid/ask area chart using lightweight-charts (or canvas), mid-price marker
- [x] 3.7 Build `features/orderbook/components/metric-grid.tsx` — compact 8-metric grid (best bid, best ask, mid, spread, bid depth, ask depth, sequence, last update)
- [x] 3.8 Add route `/orderbook` to app routing with lazy loading and nav link with hover preloading
- [x] 3.9 Write component tests for LiveFeedPage and OrderbookPage in demo mode
- [x] 3.10 Write E2E test: navigate to Live Feed, verify demo data renders, select an asset

## 4. Historical Analysis — Replay, Execution, Integrity

- [x] 4.1 Build `features/replay/ReplayPage.tsx` — replay workbench with asset selector (populated from active assets), datetime picker, microsecond input, mode selector, depth control, "Run" button
- [x] 4.2 Build `features/replay/components/replay-comparison.tsx` — side-by-side recv_time vs exchange_time comparison view
- [x] 4.3 Build `shared/components/continuity-timeline.tsx` — horizontal timeline with colored markers for continuity events, tooltip on hover
- [x] 4.4 Build `features/execution/ExecutionPage.tsx` — execution inspector with order ID search, asset filter, time window selector, paginated results table (50 per page)
- [x] 4.5 Build `features/execution/components/latency-waterfall.tsx` — horizontal waterfall chart (canvas or SVG) showing 6 latency stages with duration labels, dashed segments for null stages
- [x] 4.6 Build `features/integrity/IntegrityPage.tsx` — integrity dashboard with completeness badge, metrics grid, continuity timeline, time window display
- [x] 4.7 Add routes `/replay`, `/execution`, `/integrity` with lazy loading
- [x] 4.8 Write component tests for ReplayPage, ExecutionPage, IntegrityPage in demo mode
- [x] 4.9 Write E2E test: navigate to Replay, load demo query, run reconstruction, verify result renders

## 5. Query Workbench

- [x] 5.1 Build `features/query/QueryPage.tsx` — page layout with schema browser sidebar and editor/results main area
- [x] 5.2 Build `features/query/components/schema-browser.tsx` — collapsible dataset list fetched from `/api/v1/query/datasets`, click-to-insert column names
- [x] 5.3 Build `features/query/components/sql-editor.tsx` — multi-line textarea with basic SQL keyword highlighting, Cmd+Enter submission, sessionStorage persistence
- [x] 5.4 Build `features/query/components/query-results.tsx` — results table using DataTable component, truncation warning banner, execution time display
- [x] 5.5 Add TanStack Query mutation for `POST /api/v1/query/sql` in `shared/api/queries.ts`
- [x] 5.6 Add route `/query` with lazy loading
- [x] 5.7 Write component tests for QueryPage in demo mode (mock dataset schema, mock query result)

## 6. Keyboard Navigation & Command Palette

- [x] 6.1 Install `cmdk` (command palette library built on Radix)
- [x] 6.2 Build `shared/components/command-palette.tsx` — command palette with Cmd+K activation, fuzzy search, page navigation commands, theme toggle, density toggle, source mode toggle
- [x] 6.3 Build `shared/hooks/use-keyboard-shortcut.ts` — hook that registers keyboard shortcuts, ignores input/textarea/select focus, supports modifier keys
- [x] 6.4 Add page-level shortcuts: `r` for refresh, `?` for help overlay, `1-6` for depth presets on orderbook page
- [x] 6.5 Build `shared/components/shortcut-help.tsx` — modal overlay listing all shortcuts for the current page
- [x] 6.6 Implement focus management: move focus to page heading on navigation, return focus on modal close
- [x] 6.7 Write tests for command palette: open with Cmd+K, search, navigate, dismiss with Escape

## 7. Integration, Polish & Documentation

- [x] 7.1 Run full E2E test suite across all pages in demo mode
- [x] 7.2 Audit bundle size: verify <200KB initial JS gzipped, review chunk splitting, verify demo data loads lazily
- [x] 7.3 Verify accessibility: run axe-core checks on all pages, fix any WCAG 2.1 AA violations
- [x] 7.4 Update `web/README.md` with new project structure, dependencies, development workflow, testing instructions
- [x] 7.5 Update `web/package.json` — verify all scripts work: `dev`, `build`, `lint`, `test`, `test:e2e`, `preview`
- [x] 7.6 Remove all legacy files: old `App.css`, `index.css`, flat-structure source files, ESLint config
