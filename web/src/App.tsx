import './App.css'
import { lazy, Suspense, useEffect, useMemo, useState } from 'react'
import { BrowserRouter, MemoryRouter, NavLink, Navigate, Route, Routes } from 'react-router-dom'
import { appVersionLabel } from './constants'
import { createWorkstationClient, getInitialSourceMode, persistSourceMode } from './client'
import type { DataSourceMode } from './types'
import { SectionCard } from './ui'

interface AppContentProps {
  sourceMode: DataSourceMode
  onSourceModeChange: (mode: DataSourceMode) => void
  routerType?: 'browser' | 'memory'
  initialEntries?: string[]
}

const loadLiveFeedPage = () => import('./LiveFeedPage')
const loadReplayLabPage = () => import('./ReplayLabPage')

const LiveFeedPage = lazy(loadLiveFeedPage)
const ReplayLabPage = lazy(loadReplayLabPage)

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
      initialEntries={initialEntries}
      onSourceModeChange={() => undefined}
      routerType="memory"
      sourceMode={sourceMode}
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
            Read-only workstation views backed by the current `pb-api` routes, now with adaptive
            polling and lazy route loading as the first Phase 4.5 hardening slice.
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
          onFocus={preloadLiveFeedPage}
          onMouseEnter={preloadLiveFeedPage}
          to="/live-feed"
        >
          Live Feed
        </NavLink>
        <NavLink
          className={({ isActive }) => (isActive ? 'nav-link is-active' : 'nav-link')}
          onFocus={preloadReplayLabPage}
          onMouseEnter={preloadReplayLabPage}
          to="/replay-lab"
        >
          Replay Lab
        </NavLink>
      </nav>

      <main className="content-grid">
        <section className="workspace-column">
          <Suspense fallback={<RouteSkeleton />}>
            <Routes>
              <Route path="/" element={<Navigate replace to="/live-feed" />} />
              <Route path="/live-feed" element={<LiveFeedPage client={client} sourceMode={sourceMode} />} />
              <Route
                path="/replay-lab"
                element={<ReplayLabPage client={client} sourceMode={sourceMode} />}
              />
            </Routes>
          </Suspense>
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

          <SectionCard title="Phase 4.5 hardening now">
            <ul className="deferred-list">
              <li>
                <strong>Adaptive transport</strong>
                <span>1s foreground polling, 5s background polling, and stale request cancellation.</span>
              </li>
              <li>
                <strong>Lean route loading</strong>
                <span>Live Feed and Replay Lab now split into lazily loaded route bundles.</span>
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
                <strong>Streaming transport</strong>
                <span>True trader-grade updates still depend on backend WebSocket order-book streams.</span>
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

function RouteSkeleton() {
  return (
    <section className="card">
      <div className="card-header">
        <h3>Loading route</h3>
      </div>
      <p className="muted">Preparing the workstation view.</p>
    </section>
  )
}

function preloadLiveFeedPage() {
  void loadLiveFeedPage()
}

function preloadReplayLabPage() {
  void loadReplayLabPage()
}

export default WorkstationApp
