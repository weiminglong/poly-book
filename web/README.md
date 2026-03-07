# Workstation SPA

This app is the Phase 4 workstation frontend scaffold for `poly-book`.

Current shipped routes:

- `Live Feed`
- `Replay Lab`

Current Phase 4.5 hardening in the web client:

- adaptive live polling (`1s` foreground / `5s` background)
- abortable API requests with a client timeout
- lazy-loaded route bundles for `Live Feed` and `Replay Lab`

Deferred UI surfaces:

- Integrity
- Latency
- Execution Timeline
- Query Workbench

## Local development

```bash
# from the repo root
cargo run -- serve-api --auto-rotate

# in another terminal
cd web
npm install
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

`Live Feed` still uses HTTP polling because the backend WebSocket book stream is
not implemented yet. The client now polls aggressively only while the page is in
the foreground and backs off when the tab is hidden, which is a stopgap until
true streaming transport lands.

## Validation

```bash
npm run lint
npm run test
npm run build
```
