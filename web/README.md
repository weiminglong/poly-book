# Quant Workstation SPA

Read-only quant workstation frontend for `poly-book`. Built with React 19, Vite 7,
TypeScript 5.9 (strict), Tailwind CSS v4, and TanStack Query v5.

## Routes

| Route | Page | Purpose |
|-------|------|---------|
| `/live-feed` | Live Feed | Ops dashboard — feed health, asset grid, connection status |
| `/orderbook` | Orderbook | Price ladder + depth chart with WebSocket streaming |
| `/replay` | Replay | Reconstruct orderbooks at any historical timestamp |
| `/execution` | Execution | Inspect execution orders with latency waterfall |
| `/integrity` | Integrity | Data completeness and continuity analysis |
| `/query` | Query | SQL workbench over split datasets |

## Project structure

```
src/
  app/           App shell, providers, error boundaries, routing
  features/      Feature modules (one directory per page)
    live-feed/
    orderbook/
    replay/
    execution/
    integrity/
    query/
  shared/
    api/         Zod schemas, fetch client, TanStack Query hooks, demo fixtures
    components/  Reusable UI components (Card, Badge, Button, DataTable, etc.)
    hooks/       Shared hooks (theme, source mode, keyboard shortcuts, WS stream)
    lib/         Formatters, constants
    styles/      Tailwind v4 theme tokens and base styles
  types/         TypeScript types derived from Zod schemas via z.infer
```

## Tech stack

- **React 19** + **Vite 7** — lazy-loaded route bundles, <200KB initial JS gzipped
- **TypeScript 5.9** — strict mode with `noUnusedLocals`, `noUnusedParameters`
- **Tailwind CSS v4** — CSS-first `@theme` configuration, dark/light themes, density modes
- **TanStack Query v5** — all server state, polling, caching, mutations
- **TanStack Table v8** — sortable, paginated data tables
- **Zod** — runtime API response validation at fetch boundary
- **Radix UI** — accessible dialog, tooltip, select, popover primitives
- **cmdk** — Cmd+K command palette
- **Biome** — linting and formatting (replaces ESLint + Prettier)
- **Vitest** — unit and component tests
- **Playwright** — end-to-end tests

## Local development

Requires Node 24.13.1+ (see `.nvmrc`).

```bash
# Start the backend
cargo run -- serve-api --auto-rotate

# Start the frontend
cd web
nvm use
npm ci
npm run dev
```

Vite dev server binds `http://127.0.0.1:4173` by default and proxies `/api` to
the backend. Set `VITE_DEV_HOST=0.0.0.0` only for an intentional LAN-exposed dev
session.

```bash
# Override the API proxy target
VITE_DEV_API_PROXY_TARGET=http://127.0.0.1:3100 npm run dev

# Override the dev server host
VITE_DEV_HOST=0.0.0.0 npm run dev

# Override the API base URL directly
VITE_API_BASE_URL=http://127.0.0.1:3000 npm run dev
```

## Demo mode

The SPA includes seeded sample responses for review without live infrastructure.
Open `http://localhost:4173/?source=demo` or use the in-app data source toggle.

Demo data is lazy-loaded via dynamic `import()` to keep the initial bundle small.

## Scripts

```bash
npm run dev        # Start Vite dev server
npm run build      # Type-check + production build
npm run preview    # Preview production build locally
npm run lint       # Biome check (lint + format)
npm run lint:fix   # Biome auto-fix
npm run test       # Run Vitest unit/component tests
npm run test:e2e   # Run Playwright E2E tests
```

## Transport

The Orderbook page uses WebSocket streaming when available and falls back to HTTP
polling. A transport badge in the header shows the active mode. Feed status and
asset summaries use foreground (1s) / background (5s) adaptive polling via
TanStack Query `refetchInterval`.

## Keyboard shortcuts

- **Cmd+K** — Command palette (page navigation, theme, density, source)
- **?** — Shortcut help overlay
- **1-6** — Depth presets on Orderbook page
