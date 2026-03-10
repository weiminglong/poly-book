## Context

The poly-book web frontend is a React 19 SPA that visualizes data from `pb-api` routes: live orderbook snapshots, historical replay, integrity summaries, and execution timelines. The current implementation is a prototype-quality scaffold — flat file structure, 417 lines of global CSS, manual `useState`/`fetch` loops per page, no error boundaries, no runtime API validation, and 2 test files.

The backend is mature: Rust workspace with strict types (`FixedPrice`, `FixedSize`), split persisted datasets, WAL coordination, and configurable Parquet/ClickHouse backends. The frontend needs to match this rigor.

The target audience is a quant developer or researcher who needs both:
- **Ops console**: glanceable real-time monitoring of feed health, connection state, and data quality
- **Quant tool**: dense, keyboard-driven views for orderbook analysis, historical replay, execution inspection, and ad-hoc SQL queries

These are separate pages within a single SPA, sharing a common design system and data layer.

## Goals / Non-Goals

**Goals:**
- Establish a project structure and tooling chain that scales to 20+ pages without entropy
- Type-safe data boundary: every API response validated at runtime before entering React state
- Sub-second interaction feedback for all user actions (form submissions, page transitions, keyboard shortcuts)
- Comprehensive test coverage: unit tests for hooks/utilities, component tests for UI, E2E tests for critical flows
- Accessible, keyboard-navigable UI with command palette for power users
- Dense data rendering: orderbook depth charts, latency waterfall visualizations, price ladders
- Offline-capable demo mode that works identically to live mode (already partially exists)

**Non-Goals:**
- Mobile-optimized layouts — this is a desktop workstation tool. Responsive down to ~1024px, but not phone-sized.
- Backend API changes — the web layer adapts to existing `pb-api` routes. No new endpoints.
- Real-time collaboration or multi-user features
- Authentication or authorization — the workstation is read-only and assumes trusted network access
- Custom charting library — use existing libraries (lightweight-charts, recharts, or similar)
- Server-side rendering — SPA is correct for this use case (single-user, real-time, no SEO needs)

## Decisions

### 1. Project structure: feature-based layout

**Decision:** Organize `src/` by feature domain, not by file type.

```
web/src/
├── app/                    # App shell, routing, providers, error boundaries
│   ├── App.tsx
│   ├── routes.tsx
│   ├── providers.tsx       # QueryClient, theme, etc.
│   └── error-boundary.tsx
├── features/
│   ├── live-feed/          # Ops dashboard
│   │   ├── LiveFeedPage.tsx
│   │   ├── components/     # Feature-specific components
│   │   ├── hooks/          # Feature-specific hooks
│   │   └── index.ts        # Public exports
│   ├── orderbook/          # Institutional orderbook viewer
│   ├── replay/             # Replay workbench
│   ├── execution/          # Execution inspector
│   ├── integrity/          # Integrity dashboard
│   └── query/              # SQL workbench
├── shared/
│   ├── api/                # TanStack Query hooks, Zod schemas, fetch client
│   │   ├── client.ts       # Base fetch with timeout, error handling
│   │   ├── schemas.ts      # Zod schemas mirroring pb-api types
│   │   ├── queries.ts      # TanStack Query hook factories
│   │   └── demo.ts         # Demo data provider
│   ├── components/         # Design system components
│   │   ├── card.tsx
│   │   ├── badge.tsx
│   │   ├── table.tsx
│   │   └── ...
│   ├── hooks/              # Shared hooks (useKeyboard, useThrottle, etc.)
│   ├── lib/                # Pure utilities (formatters, constants)
│   └── styles/             # Tailwind config, CSS custom properties, tokens
└── types/                  # Shared TypeScript types (derived from Zod schemas)
```

**Why over alternatives:**
- *Type-based layout* (`components/`, `hooks/`, `pages/`) breaks down at scale — finding "all code related to orderbook" requires searching 5 directories.
- *Feature-based* keeps related code colocated. A feature can be understood, tested, and modified in isolation.
- `shared/` is the escape hatch for cross-cutting concerns. The rule: if it's used by 2+ features, it lives in `shared/`.

### 2. Styling: Tailwind CSS v4

**Decision:** Replace global CSS with Tailwind CSS v4.

**Why:**
- Tailwind v4 uses CSS-first configuration (`@theme` in CSS, no `tailwind.config.js`), design tokens are CSS custom properties natively.
- Utility-first prevents the cascade collision problem that global CSS has today.
- The dark/light theming maps directly to CSS custom properties + `prefers-color-scheme`.
- Density modes (compact/comfortable/spacious) are achieved via CSS custom properties on a root class, adjusting spacing/font-size tokens.

**Why not CSS Modules:** CSS Modules solve scoping but don't provide a design token system, spacing scale, or consistent typography. You end up building a mini-framework. Tailwind gives this out of the box.

**Why not styled-components/Emotion:** Runtime CSS-in-JS adds bundle weight and slows render performance — the opposite of what a data-dense workstation needs.

### 3. Data fetching: TanStack Query v5

**Decision:** Replace manual `fetch` + `useState` + `useAdaptivePolling` loops with TanStack Query.

**Why:**
- Automatic cache, dedup, background refetch, stale-while-revalidate, retry, and devtools.
- `refetchInterval` replaces `useAdaptivePolling` for HTTP-polled data. Visibility-aware refetching is built in.
- The WebSocket orderbook stream stays as a custom hook but writes into the TanStack Query cache via `queryClient.setQueryData`, unifying the data layer.
- `placeholderData` replaces the manual "show stale data while loading" pattern used across all pages.

**What stays custom:**
- `useOrderBookStream` — WebSocket lifecycle is inherently imperative. It feeds into the query cache rather than being replaced by it.
- `useThrottledState` — RAF coalescing for high-frequency WS updates still needed between WS messages and query cache writes.

### 4. API validation: Zod schemas

**Decision:** Define Zod schemas that mirror every `pb-api` response type. Validate at the fetch boundary.

```
API response (unknown) → Zod parse → typed data → TanStack Query cache → components
```

**Why:** The current `fetchJson<T>` performs `response.json() as T` — a lie at runtime. A backend schema change (e.g., field renamed from `mid_price` to `midPrice`) would silently produce `undefined` values that propagate through the UI until something crashes. Zod catches this at the boundary with a clear error message.

**Performance:** Zod parsing is ~1-5ms per response at the sizes we handle. Negligible compared to network latency.

### 5. Component primitives: Radix UI (shadcn/ui pattern)

**Decision:** Use Radix UI headless primitives for interactive components (Dialog, Tooltip, Popover, Select, Command). Copy the shadcn/ui composition pattern: components are owned source files, not imported from a library.

**Why:**
- Radix handles focus management, keyboard navigation, screen reader announcements, and portal rendering correctly. Hand-rolling these in a trading UI is a liability.
- shadcn/ui pattern means we own the code — no version-lock to a component library. Components live in `shared/components/` and are styled with Tailwind.

**Why not MUI/Ant Design:** Too opinionated, too heavy, and their design language doesn't fit a trading workstation.

### 6. Charting: lightweight-charts for orderbook depth

**Decision:** Use TradingView's `lightweight-charts` for orderbook depth visualization and price history. Use custom SVG/Canvas for the latency waterfall (execution inspector) since it's a domain-specific visualization.

**Why:**
- `lightweight-charts` is purpose-built for financial data, handles large datasets efficiently, and is used by trading platforms in production.
- The latency waterfall is a simple horizontal bar chart with 6 stages — no library needed, a custom `<canvas>` or SVG element is simpler and more controllable.

**Why not D3:** D3 is powerful but low-level. For the orderbook depth chart, `lightweight-charts` gives us the right abstractions out of the box. D3 would mean writing 200+ lines of scale/axis/rendering code for the same result.

### 7. Tooling: Biome replaces ESLint

**Decision:** Replace ESLint + typescript-eslint + eslint-plugin-react-hooks + eslint-plugin-react-refresh with Biome.

**Why:** Single binary, ~100x faster than ESLint, handles both linting and formatting, stricter defaults, and first-class TypeScript support. The current ESLint config is 23 lines of boilerplate that Biome replaces with zero config.

### 8. Testing strategy

**Decision:** Three-tier testing:

| Layer | Tool | What it covers |
|-------|------|---------------|
| Unit | Vitest | Hooks, formatters, Zod schemas, utilities |
| Component | Vitest + Testing Library | Individual components and page-level rendering |
| E2E | Playwright | Critical user flows (navigate pages, submit forms, demo mode) |

**Coverage targets:** >80% line coverage for `shared/`, >60% for feature pages. All Zod schemas must have round-trip tests against demo fixtures.

### 9. Routing: keep react-router-dom v7

**Decision:** Stay with react-router-dom v7. It's already in use, supports lazy routes, and the routing needs are simple (6-8 top-level routes, no nested layouts beyond the shell).

**Why not TanStack Router:** TanStack Router offers type-safe routes and built-in search param validation, which is appealing. But it's a migration that adds complexity without proportional benefit for a flat route structure. If route params become more complex (e.g., deep-linking replay queries), revisit.

### 10. State management: TanStack Query is the state manager

**Decision:** No additional state management library (no Redux, no Zustand). TanStack Query owns server state. Local UI state (form values, sidebar open/closed, density mode) uses React `useState` or `useContext` for app-level preferences.

**Why:** The current app has no client-side-only domain state. Everything comes from the API or WebSocket. TanStack Query is the right tool for server state. Adding Zustand or Redux would be over-engineering.

## Risks / Trade-offs

**[Risk: Tailwind v4 is relatively new]** → Tailwind v4 was released in early 2025 and is stable. The CSS-first config is a departure from v3 but aligns better with standard CSS tooling. Fallback: v3 is a safe alternative with the same utility-first approach.

**[Risk: lightweight-charts may not support all orderbook visualizations]** → Depth chart (area/histogram) is well-supported. Price ladder with size bars is a custom component, not a chart library concern. If lightweight-charts proves limiting, it can be replaced per-feature without affecting the rest of the app.

**[Risk: Bundle size growth from new dependencies]** → Mitigation: aggressive code splitting (already in place), tree-shaking, and route-level lazy loading. Radix primitives are individually importable. TanStack Query is ~13KB gzipped. lightweight-charts is ~45KB gzipped but only loaded on the orderbook page. Budget: <200KB initial JS gzipped.

**[Risk: Scope creep — 6 capabilities is a lot]** → Mitigation: Implement in phases. Platform foundation and design system first (these are prerequisites). Then market-data-views (highest value). Then historical-analysis, query-workbench, keyboard-navigation in order of value. Each capability is independently shippable.

**[Trade-off: Biome doesn't support all ESLint rules]** → Biome covers the important rules (react-hooks, no-unused-vars, consistent-returns, etc.) but lacks some niche ESLint plugins. This is acceptable — the niche rules weren't configured in the current ESLint setup anyway.

**[Trade-off: Zod adds runtime cost]** → ~1-5ms per API response parse. Acceptable for the data sizes involved (orderbooks with <500 levels, execution events <1000 rows). The alternative — trusting `as T` casts — has bitten production systems at scale.

## Open Questions

1. **Charting library final pick**: Should we evaluate `uPlot` as an alternative to `lightweight-charts`? uPlot is smaller (~30KB) and faster for time-series, but less polished for financial-specific features. Decision can be deferred to the `market-data-views` implementation task.

2. **WebSocket reconnection strategy**: The current hook has exponential backoff with 8-retry fallback to HTTP. Should this be configurable per-deployment, or is the current hardcoded strategy sufficient?

3. **Demo data isolation**: Currently `demoData.ts` (334 lines) ships in the main bundle even in `api` mode. Should demo data be split into a separate chunk that's only loaded when `sourceMode === 'demo'`? (Likely yes — lazy import.)
