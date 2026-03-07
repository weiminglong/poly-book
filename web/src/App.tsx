import './App.css'
import { useCallback, useEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { BrowserRouter, MemoryRouter, NavLink, Navigate, Route, Routes } from 'react-router-dom'
import {
  createWorkstationClient,
  getDemoReplayDefaults,
  getInitialSourceMode,
  persistSourceMode,
} from './client'
import { formatLevel, formatNumber, formatPrice, formatSize, formatTimestamp, titleCase } from './formatters'
import type {
  ActiveAssetSummary,
  ContinuityWarning,
  DataSourceMode,
  FeedStatusResponse,
  LiveOrderBookSnapshot,
  ReplayFormValues,
  ReplayReconstructionResponse,
  WorkstationClient,
} from './types'

interface LoadState<T> {
  data: T | null
  loading: boolean
  error: string | null
}

interface AppContentProps {
  sourceMode: DataSourceMode
  onSourceModeChange: (mode: DataSourceMode) => void
  routerType?: 'browser' | 'memory'
  initialEntries?: string[]
}

const refreshIntervalMs = 5_000
const appVersionLabel = 'Phase 4 scaffold'

function WorkstationApp() {
  const [sourceMode, setSourceMode] = useState<DataSourceMode>(() => getInitialSourceMode())

  useEffect(() => {
    persistSourceMode(sourceMode)
  }, [sourceMode])

  return <AppContent sourceMode={sourceMode} onSourceModeChange={setSourceMode} />
}

export function TestableApp({
  sourceMode = 'demo',
  initialEntries = ['/live-feed'],
}: {
  sourceMode?: DataSourceMode
  initialEntries?: string[]
}) {
  return (
    <AppContent
      sourceMode={sourceMode}
      onSourceModeChange={() => undefined}
      routerType="memory"
      initialEntries={initialEntries}
    />
  )
}

function AppContent({
  sourceMode,
  onSourceModeChange,
  routerType = 'browser',
  initialEntries = ['/live-feed'],
}: AppContentProps) {
  const client = useMemo(() => createWorkstationClient(sourceMode), [sourceMode])
  const shell = (
    <div className="app-shell">
      <header className="topbar">
        <div>
          <p className="eyebrow">Poly-book quant workstation</p>
          <h1>Live Feed + Replay Lab</h1>
          <p className="lede">
            Read-only workstation views backed by the current `pb-api` routes, with seeded demo
            data for offline review.
          </p>
        </div>
        <div className="topbar-actions">
          <div className="source-toggle" role="group" aria-label="data source">
            <button
              className={sourceMode === 'api' ? 'source-toggle__button is-active' : 'source-toggle__button'}
              onClick={() => onSourceModeChange('api')}
              type="button"
            >
              Live API
            </button>
            <button
              className={sourceMode === 'demo' ? 'source-toggle__button is-active' : 'source-toggle__button'}
              onClick={() => onSourceModeChange('demo')}
              type="button"
            >
              Demo data
            </button>
          </div>
          <p className="source-hint">
            {sourceMode === 'api'
              ? 'Fetches /api/v1 routes from the configured pb-api base URL.'
              : 'Uses seeded fixtures so the UI is reviewable without live infrastructure.'}
          </p>
        </div>
      </header>

      <nav className="primary-nav" aria-label="primary">
        <NavLink
          className={({ isActive }) => (isActive ? 'nav-link is-active' : 'nav-link')}
          to="/live-feed"
        >
          Live Feed
        </NavLink>
        <NavLink
          className={({ isActive }) => (isActive ? 'nav-link is-active' : 'nav-link')}
          to="/replay-lab"
        >
          Replay Lab
        </NavLink>
      </nav>

      <main className="content-grid">
        <section className="workspace-column">
          <Routes>
            <Route path="/" element={<Navigate replace to="/live-feed" />} />
            <Route path="/live-feed" element={<LiveFeedPage client={client} sourceMode={sourceMode} />} />
            <Route
              path="/replay-lab"
              element={<ReplayLabPage client={client} sourceMode={sourceMode} />}
            />
          </Routes>
        </section>

        <aside className="sidebar-column">
          <SectionCard title="Current shipped surfaces">
            <ul className="deferred-list">
              <li>
                <strong>Live Feed</strong>
                <span>Session health, active assets, and live snapshots.</span>
              </li>
              <li>
                <strong>Replay Lab</strong>
                <span>Point-in-time replay across `recv_time` and `exchange_time`.</span>
              </li>
            </ul>
          </SectionCard>

          <SectionCard title="Deferred from this pass">
            <ul className="deferred-list">
              <li>
                <strong>Integrity</strong>
                <span>Not yet wired because the backend integrity routes remain deferred.</span>
              </li>
              <li>
                <strong>Latency</strong>
                <span>Reserved for later metrics-backed summaries.</span>
              </li>
              <li>
                <strong>Execution Timeline</strong>
                <span>Read-only execution inspection stays future scope.</span>
              </li>
              <li>
                <strong>Query Workbench</strong>
                <span>SQL routes are still planned but intentionally unshipped.</span>
              </li>
            </ul>
          </SectionCard>

          <SectionCard title="Notes">
            <ul className="notes-list">
              <li>{appVersionLabel}</li>
              <li>Replay uses `source=parquet` only, matching the current backend contract.</li>
              <li>Live snapshots remain server-derived; the browser never reconstructs feed state.</li>
            </ul>
          </SectionCard>
        </aside>
      </main>
    </div>
  )

  return routerType === 'memory' ? (
    <MemoryRouter initialEntries={initialEntries}>{shell}</MemoryRouter>
  ) : (
    <BrowserRouter>{shell}</BrowserRouter>
  )
}

function LiveFeedPage({
  client,
  sourceMode,
}: {
  client: WorkstationClient
  sourceMode: DataSourceMode
}) {
  const [feedState, setFeedState] = useState<LoadState<FeedStatusResponse>>({
    data: null,
    loading: true,
    error: null,
  })
  const [assetsState, setAssetsState] = useState<LoadState<ActiveAssetSummary[]>>({
    data: null,
    loading: true,
    error: null,
  })
  const [snapshotState, setSnapshotState] = useState<LoadState<LiveOrderBookSnapshot>>({
    data: null,
    loading: false,
    error: null,
  })
  const [userSelectedAssetId, setUserSelectedAssetId] = useState<string>('')
  const [depth, setDepth] = useState<number>(5)

  useEffect(() => {
    let isMounted = true

    const refresh = async () => {
      setFeedState((previous) => ({ ...previous, loading: true, error: null }))
      setAssetsState((previous) => ({ ...previous, loading: true, error: null }))

      try {
        const [feedStatus, activeAssets] = await Promise.all([
          client.getFeedStatus(),
          client.getActiveAssets(),
        ])

        if (!isMounted) {
          return
        }

        setFeedState({ data: feedStatus, loading: false, error: null })
        setAssetsState({ data: activeAssets, loading: false, error: null })
      } catch (error) {
        if (!isMounted) {
          return
        }

        const message = error instanceof Error ? error.message : 'Unable to load live feed data.'
        setFeedState((previous) => ({ ...previous, loading: false, error: message }))
        setAssetsState((previous) => ({ ...previous, loading: false, error: message }))
      }
    }

    void refresh()
    const intervalId = window.setInterval(() => {
      void refresh()
    }, refreshIntervalMs)

    return () => {
      isMounted = false
      window.clearInterval(intervalId)
    }
  }, [client])

  const assetIds = assetsState.data?.map((asset) => asset.asset_id) ?? []
  const selectedAssetId = assetIds.includes(userSelectedAssetId) ? userSelectedAssetId : assetIds[0] ?? ''

  useEffect(() => {
    if (!selectedAssetId) {
      return
    }

    let isMounted = true

    const refresh = async () => {
      setSnapshotState((previous) => ({ ...previous, loading: true, error: null }))

      try {
        const snapshot = await client.getOrderBookSnapshot(selectedAssetId, depth)
        if (!isMounted) {
          return
        }

        setSnapshotState({ data: snapshot, loading: false, error: null })
      } catch (error) {
        if (!isMounted) {
          return
        }

        const message =
          error instanceof Error ? error.message : 'Unable to load the selected order book.'
        setSnapshotState((previous) => ({ ...previous, loading: false, error: message }))
      }
    }

    void refresh()
    const intervalId = window.setInterval(() => {
      void refresh()
    }, refreshIntervalMs)

    return () => {
      isMounted = false
      window.clearInterval(intervalId)
    }
  }, [client, depth, selectedAssetId])

  const liveSnapshot =
    snapshotState.data?.asset_id === selectedAssetId ? snapshotState.data : null

  return (
    <div className="page-stack">
      <section className="hero-card">
        <div>
          <p className="section-title">Live Feed</p>
          <h2>Monitor the current read model without reconstructing in the browser.</h2>
        </div>
        <span className={`pill pill--${feedState.data?.session_status ?? 'starting'}`}>
          {titleCase(feedState.data?.session_status ?? 'starting')}
        </span>
      </section>

      {feedState.error ? (
        <ErrorBanner
          message={feedState.error}
          sourceMode={sourceMode}
          title="Live API request failed"
        />
      ) : null}

      <div className="stats-grid">
        <MetricCard
          label="Feed mode"
          value={feedState.data ? titleCase(feedState.data.mode) : 'Loading…'}
        />
        <MetricCard
          label="Active assets"
          value={String(feedState.data?.active_asset_count ?? assetsState.data?.length ?? 0)}
        />
        <MetricCard
          label="Session ID"
          value={feedState.data?.current_session_id ?? '—'}
        />
        <MetricCard
          label="Last rotation"
          value={formatTimestamp(feedState.data?.last_rotation_us)}
        />
      </div>

      {feedState.data?.latest_global_warning ? (
        <WarningPanel title="Latest continuity warning" warning={feedState.data.latest_global_warning} />
      ) : null}

      <div className="two-column-grid">
        <SectionCard
          title="Active assets"
          toolbar={
            <label className="inline-field">
              <span>Depth</span>
              <input
                max={200}
                min={1}
                onChange={(event) => setDepth(Number(event.target.value))}
                type="number"
                value={depth}
              />
            </label>
          }
        >
          {assetsState.loading && !assetsState.data ? <p className="muted">Loading active assets…</p> : null}
          {assetsState.data && assetsState.data.length > 0 ? (
            <div className="asset-list">
              {assetsState.data.map((asset) => (
                <button
                  className={selectedAssetId === asset.asset_id ? 'asset-row is-selected' : 'asset-row'}
                  key={asset.asset_id}
                  onClick={() => setUserSelectedAssetId(asset.asset_id)}
                  type="button"
                >
                  <div>
                    <strong>{asset.asset_id}</strong>
                    <p>
                      recv {formatTimestamp(asset.last_recv_timestamp_us)} · exchange{' '}
                      {formatTimestamp(asset.last_exchange_timestamp_us)}
                    </p>
                  </div>
                  <div className="asset-flags">
                    <span className={asset.has_book ? 'pill pill--healthy' : 'pill pill--warning'}>
                      {asset.has_book ? 'Book ready' : 'No book'}
                    </span>
                    <span className={asset.stale ? 'pill pill--warning' : 'pill pill--healthy'}>
                      {asset.stale ? 'Stale' : 'Fresh'}
                    </span>
                  </div>
                </button>
              ))}
            </div>
          ) : null}
          {!assetsState.loading && !assetsState.error && (assetsState.data?.length ?? 0) === 0 ? (
            <p className="muted">No active assets are currently exposed by the API.</p>
          ) : null}
        </SectionCard>

        <SectionCard
          title={selectedAssetId ? `Order book · ${selectedAssetId}` : 'Order book'}
          toolbar={<span className="muted">Polling every {refreshIntervalMs / 1000}s</span>}
        >
          {!selectedAssetId ? <p className="muted">Select an asset to load the current live snapshot.</p> : null}
          {snapshotState.loading && !liveSnapshot ? <p className="muted">Loading order book snapshot…</p> : null}
          {snapshotState.error ? <p className="error-text">{snapshotState.error}</p> : null}
          {liveSnapshot ? (
            <div className="snapshot-stack">
              <div className="stats-grid compact">
                <MetricCard label="Sequence" value={String(liveSnapshot.sequence)} />
                <MetricCard label="Updated" value={formatTimestamp(liveSnapshot.last_update_us)} />
                <MetricCard label="Best bid" value={formatLevel(liveSnapshot.best_bid)} />
                <MetricCard label="Best ask" value={formatLevel(liveSnapshot.best_ask)} />
                <MetricCard label="Mid" value={formatNumber(liveSnapshot.mid_price)} />
                <MetricCard label="Spread" value={formatNumber(liveSnapshot.spread)} />
              </div>

              {liveSnapshot.latest_warning ? (
                <WarningPanel title="Asset warning" warning={liveSnapshot.latest_warning} />
              ) : null}

              <OrderBookTable asks={liveSnapshot.asks} bids={liveSnapshot.bids} />
            </div>
          ) : null}
        </SectionCard>
      </div>
    </div>
  )
}

function ReplayLabPage({
  client,
  sourceMode,
}: {
  client: WorkstationClient
  sourceMode: DataSourceMode
}) {
  const [form, setForm] = useState<ReplayFormValues>(() =>
    sourceMode === 'demo'
      ? getDemoReplayDefaults()
      : {
          assetId: '',
          atUs: '',
          mode: 'recv_time',
          depth: 5,
        },
  )
  const [resultState, setResultState] = useState<LoadState<ReplayReconstructionResponse>>({
    data: null,
    loading: false,
    error: null,
  })
  const [assetOptions, setAssetOptions] = useState<string[]>([])

  useEffect(() => {
    let isMounted = true

    const loadAssetOptions = async () => {
      try {
        const assets = await client.getActiveAssets()
        if (!isMounted) {
          return
        }

        setAssetOptions(assets.map((asset) => asset.asset_id))
      } catch {
        if (isMounted) {
          setAssetOptions([])
        }
      }
    }

    void loadAssetOptions()

    return () => {
      isMounted = false
    }
  }, [client])

  const runReplay = useCallback(
    async (values: ReplayFormValues) => {
      const atUs = Number(values.atUs)
      if (!values.assetId.trim()) {
        setResultState({ data: null, loading: false, error: 'Asset ID is required.' })
        return
      }
      if (!Number.isFinite(atUs) || atUs <= 0) {
        setResultState({ data: null, loading: false, error: 'Timestamp must be a positive microsecond value.' })
        return
      }
      if (values.depth <= 0) {
        setResultState({ data: null, loading: false, error: 'Depth must be greater than zero.' })
        return
      }

      setResultState((previous) => ({ ...previous, loading: true, error: null }))

      try {
        const replay = await client.getReplayReconstruction({
          assetId: values.assetId.trim(),
          atUs,
          mode: values.mode,
          depth: values.depth,
        })
        setResultState({ data: replay, loading: false, error: null })
      } catch (error) {
        const message = error instanceof Error ? error.message : 'Replay reconstruction failed.'
        setResultState({ data: null, loading: false, error: message })
      }
    },
    [client],
  )

  return (
    <div className="page-stack">
      <section className="hero-card">
        <div>
          <p className="section-title">Replay Lab</p>
          <h2>Inspect Parquet-backed reconstruction with explicit time semantics.</h2>
        </div>
        <span className="pill pill--neutral">Parquet only</span>
      </section>

      {resultState.error ? (
        <ErrorBanner
          message={resultState.error}
          sourceMode={sourceMode}
          title="Replay request failed"
        />
      ) : null}

      <div className="two-column-grid">
        <SectionCard title="Replay query">
          <form
            className="form-grid"
            onSubmit={(event) => {
              event.preventDefault()
              void runReplay(form)
            }}
          >
            <label>
              <span>Asset ID</span>
              <input
                onChange={(event) => setForm((previous) => ({ ...previous, assetId: event.target.value }))}
                placeholder="tok1"
                type="text"
                value={form.assetId}
              />
            </label>

            <label>
              <span>`at_us`</span>
              <input
                onChange={(event) => setForm((previous) => ({ ...previous, atUs: event.target.value }))}
                placeholder="1700000000000000"
                type="text"
                value={form.atUs}
              />
            </label>

            <label>
              <span>Mode</span>
              <select
                onChange={(event) =>
                  setForm((previous) => ({
                    ...previous,
                    mode: event.target.value as ReplayFormValues['mode'],
                  }))
                }
                value={form.mode}
              >
                <option value="recv_time">recv_time</option>
                <option value="exchange_time">exchange_time</option>
              </select>
            </label>

            <label>
              <span>Depth</span>
              <input
                max={200}
                min={1}
                onChange={(event) =>
                  setForm((previous) => ({ ...previous, depth: Number(event.target.value) || 1 }))
                }
                type="number"
                value={form.depth}
              />
            </label>

            <button className="primary-button" type="submit">
              {resultState.loading ? 'Running…' : 'Run reconstruction'}
            </button>
          </form>

          <div className="chip-list">
            {assetOptions.map((assetId) => (
              <button
                className="chip"
                key={assetId}
                onClick={() => setForm((previous) => ({ ...previous, assetId }))}
                type="button"
              >
                {assetId}
              </button>
            ))}
          </div>

          {sourceMode === 'demo' ? (
            <button
              className="secondary-button"
              onClick={() => setForm(getDemoReplayDefaults())}
              type="button"
            >
              Load demo query
            </button>
          ) : null}

          <p className="muted">
            API mode requires local Parquet history under `./data`. Demo mode provides a seeded
            query plus fixture-backed replay responses.
          </p>
        </SectionCard>

        <SectionCard title="Replay result">
          {resultState.loading && !resultState.data ? <p className="muted">Loading replay reconstruction…</p> : null}
          {!resultState.loading && !resultState.error && !resultState.data ? (
            <p className="muted">Submit a replay query to inspect the reconstructed top of book.</p>
          ) : null}
          {resultState.data ? (
            <div className="snapshot-stack">
              <div className="stats-grid compact">
                <MetricCard label="Asset" value={resultState.data.asset_id} />
                <MetricCard label="Mode" value={resultState.data.mode} />
                <MetricCard
                  label="Checkpoint"
                  value={resultState.data.used_checkpoint ? 'Used checkpoint' : 'Snapshot only'}
                />
                <MetricCard label="Sequence" value={String(resultState.data.sequence)} />
                <MetricCard label="Updated" value={formatTimestamp(resultState.data.last_update_us)} />
                <MetricCard label="Mid" value={formatNumber(resultState.data.mid_price)} />
              </div>

              <OrderBookTable asks={resultState.data.asks} bids={resultState.data.bids} />

              <SectionCard title="Continuity events" dense>
                {resultState.data.continuity_events.length === 0 ? (
                  <p className="muted">No continuity events were returned for this reconstruction.</p>
                ) : (
                  <div className="warning-stack">
                    {resultState.data.continuity_events.map((warning) => (
                      <WarningPanel key={`${warning.kind}-${warning.recv_timestamp_us}`} warning={warning} />
                    ))}
                  </div>
                )}
              </SectionCard>
            </div>
          ) : null}
        </SectionCard>
      </div>
    </div>
  )
}

function SectionCard({
  title,
  children,
  dense = false,
  toolbar,
}: {
  title: string
  children: ReactNode
  dense?: boolean
  toolbar?: ReactNode
}) {
  return (
    <section className={dense ? 'card card--dense' : 'card'}>
      <div className="card-header">
        <h3>{title}</h3>
        {toolbar}
      </div>
      {children}
    </section>
  )
}

function MetricCard({ label, value }: { label: string; value: string }) {
  return (
    <article className="metric-card">
      <span>{label}</span>
      <strong>{value}</strong>
    </article>
  )
}

function WarningPanel({
  title = titleCase('continuity_warning'),
  warning,
}: {
  title?: string
  warning: ContinuityWarning
}) {
  return (
    <div className="warning-panel">
      <div className="warning-panel__header">
        <strong>{title}</strong>
        <span className="pill pill--warning">{titleCase(warning.kind)}</span>
      </div>
      <p>{warning.details ?? 'No extra details were supplied by the backend.'}</p>
      <p className="muted">
        recv {formatTimestamp(warning.recv_timestamp_us)} · exchange{' '}
        {formatTimestamp(warning.exchange_timestamp_us)}
      </p>
    </div>
  )
}

function ErrorBanner({
  title,
  message,
  sourceMode,
}: {
  title: string
  message: string
  sourceMode: DataSourceMode
}) {
  return (
    <div className="error-banner" role="alert">
      <strong>{title}</strong>
      <p>{message}</p>
      {sourceMode === 'api' ? (
        <p className="muted">If `serve-api` is not running, switch to Demo data to review the SPA.</p>
      ) : null}
    </div>
  )
}

function OrderBookTable({
  bids,
  asks,
}: {
  bids: LiveOrderBookSnapshot['bids'] | ReplayReconstructionResponse['bids']
  asks: LiveOrderBookSnapshot['asks'] | ReplayReconstructionResponse['asks']
}) {
  return (
    <div className="book-grid">
      <div>
        <h4>Bids</h4>
        <table>
          <thead>
            <tr>
              <th>Price</th>
              <th>Size</th>
            </tr>
          </thead>
          <tbody>
            {bids.map((level) => (
              <tr key={`bid-${level.price}-${level.size}`}>
                <td>{formatPrice(level.price)}</td>
                <td>{formatSize(level.size)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div>
        <h4>Asks</h4>
        <table>
          <thead>
            <tr>
              <th>Price</th>
              <th>Size</th>
            </tr>
          </thead>
          <tbody>
            {asks.map((level) => (
              <tr key={`ask-${level.price}-${level.size}`}>
                <td>{formatPrice(level.price)}</td>
                <td>{formatSize(level.size)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}

export default WorkstationApp
