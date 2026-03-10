## Why

The current web frontend is a functional prototype with 4 pages, 417 lines of global CSS, manual fetch/setState loops, no error boundaries, and no runtime API validation. It works for demo screenshots but falls short of what an institutional quant workstation demands: dense data rendering, sub-second feedback loops, keyboard-driven workflows, and the kind of reliability where a malformed API response doesn't crash the entire app. Rebuilding it as a clean-slate, modern platform establishes the patterns used at firms like Jane Street and Citadel — type-safe data boundaries, component isolation, deterministic styling, and aggressive testing — while the domain surface (orderbook, replay, execution, integrity) is still small enough to rebuild without migration headaches.

## What Changes

### Platform foundation
- **BREAKING**: Replace the flat `src/` structure with a feature-based layout (`src/features/`, `src/shared/`, `src/app/`)
- **BREAKING**: Replace global CSS (`App.css`, `index.css`) with Tailwind CSS v4 and a design token system
- **BREAKING**: Replace manual `fetchJson` + `useState` loops with TanStack Query for all server state
- Add Zod schemas at the fetch boundary for runtime validation of every API response
- Add React error boundaries per route and per widget
- Replace ESLint + manual config with Biome for linting and formatting
- Add comprehensive test infrastructure: Vitest unit tests, component tests, and Playwright E2E

### Ops console mode
- Redesign the Live Feed page as a real-time ops dashboard: feed health panel, asset grid with sparklines, connection status, and alerting indicators
- Add a system vitals sidebar showing polling cadence, WebSocket state, and last-seen timestamps

### Quant tool mode
- Build an institutional-grade orderbook viewer: price ladder with size bars, depth chart (cumulative size), bid/ask heatmap, and configurable depth levels
- Enhance the Replay Lab into a replay workbench with timeline scrubbing, side-by-side recv_time vs exchange_time comparison, and checkpoint markers
- Upgrade the Execution Timeline into an execution inspector with latency waterfall visualization (market_data_recv → normalization → strategy_decision → order_submit → exchange_ack → fill)
- Build the deferred Query Workbench: schema browser, SQL editor with syntax highlighting, paginated results table

### Navigation and interaction
- Add a command palette (Cmd+K) for fast navigation and actions
- Add keyboard shortcuts for common operations (next asset, toggle depth, switch pages)
- Support density modes (compact / comfortable / spacious) for different screen sizes and preferences
- Implement a theming system with dark (default) and light modes using CSS custom properties

### Design system
- Build a shared component library: Card, MetricCard, Badge, Table, Form controls, ErrorBanner, Skeleton, Tooltip
- All components built on Radix UI primitives (via shadcn/ui pattern) for accessibility
- Consistent motion system using CSS transitions and `prefers-reduced-motion`

## Capabilities

### New Capabilities
- `platform-foundation`: Project structure, tooling (Biome, Tailwind v4, TanStack Query, Zod), error boundaries, test infrastructure, and data-fetching architecture
- `design-system`: Shared component library with Radix primitives, theming (dark/light), density modes, and design tokens
- `market-data-views`: Real-time ops dashboard and institutional orderbook viewer with depth chart, price ladder, and heatmap
- `historical-analysis`: Replay workbench with timeline controls and comparison views, execution inspector with latency waterfall, and integrity dashboard with gap visualization
- `query-workbench`: SQL workbench over split datasets with schema browser and results table
- `keyboard-navigation`: Command palette (Cmd+K), page-level keyboard shortcuts, and focus management

### Modified Capabilities
(none — this is a clean-slate rebuild of the web layer; no existing specs are affected)

## Impact

- **web/**: Complete rewrite of all files. Every existing `.tsx`, `.ts`, and `.css` file will be replaced.
- **web/package.json**: New dependencies added (TanStack Query, Zod, Radix UI primitives, Tailwind CSS v4, @tanstack/react-table, a charting library, cmdk). Biome replaces ESLint. Node engine requirement stays >=24.
- **Backend API contract**: No backend changes. The web layer consumes existing `pb-api` routes as-is. Zod schemas will be derived from `types.ts` which already mirrors the Rust types.
- **Build output**: Chunk splitting strategy will change. Bundle analysis target: <200KB initial JS (gzipped), with route-level code splitting preserved.
- **CI**: Web validation step (`npm run lint && npx tsc -b && npm test && npm run build`) stays the same, but `lint` invokes Biome instead of ESLint. Playwright E2E tests added as a separate CI step.
- **docs/**: No documentation changes required — `web/README.md` will be updated as part of the rebuild.
