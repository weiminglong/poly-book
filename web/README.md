# Workstation SPA

This app is the Phase 4 workstation frontend scaffold for `poly-book`.

Current shipped routes:

- `Live Feed`
- `Replay Lab`
- `Integrity`
- `Execution Timeline`

Current Phase 4.5 hardening in the web client:

- adaptive live polling (`1s` foreground / `5s` background)
- abortable API requests with a client timeout
- lazy-loaded route bundles for current shipped routes
- WebSocket order book streaming with HTTP fallback
- virtualized order book rendering with throttled stream updates

Deferred UI surfaces:

- Latency
- Query Workbench

## Local development

The workstation SPA requires Node `22.12.0` or newer. Use the checked-in
[`web/.nvmrc`](./.nvmrc) as the source of truth for local and CI runs.

```bash
# from the repo root
cargo run -- serve-api --auto-rotate

# in another terminal
cd web
nvm use
npm ci
npm run dev
```

The Vite server runs on `http://127.0.0.1:4173` and proxies `/api` to
`http://127.0.0.1:3000` by default.

Useful overrides:

```bash
VITE_DEV_API_PROXY_TARGET=http://127.0.0.1:3100 npm run dev
VITE_API_BASE_URL=http://127.0.0.1:3000 npm run dev
```

## Demo mode

The SPA includes seeded sample responses so it can be reviewed without live API
or Parquet infrastructure. Open `http://127.0.0.1:4173/?source=demo` or use the
in-app data-source toggle.

## Current transport notes

`Live Feed` uses the backend WebSocket book stream when available and falls back
to adaptive HTTP polling if the stream cannot be established. Feed status and
active asset summaries still use adaptive foreground/background polling.

## Validation

```bash
nvm use
npm run lint
npx tsc -b
npm run test
npm run build
```
