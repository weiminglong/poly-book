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
const loadIntegrityPage = () => import('./IntegrityPage')
const loadExecutionTimelinePage = () => import('./ExecutionTimelinePage')

const LiveFeedPage = lazy(loadLiveFeedPage)
const ReplayLabPage = lazy(loadReplayLabPage)
const IntegrityPage = lazy(loadIntegrityPage)
const ExecutionTimelinePage = lazy(loadExecutionTimelinePage)

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
          <h1>Quant Workstation</h1>
          <p className="lede">
            Read-only workstation views backed by the pb-api routes: live feed, replay,
            integrity, and execution timeline.
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
        <NavLink
          className={({ isActive }) => (isActive ? 'nav-link is-active' : 'nav-link')}
          onFocus={preloadIntegrityPage}
          onMouseEnter={preloadIntegrityPage}
          to="/integrity"
        >
          Integrity
        </NavLink>
        <NavLink
          className={({ isActive }) => (isActive ? 'nav-link is-active' : 'nav-link')}
          onFocus={preloadExecutionTimelinePage}
          onMouseEnter={preloadExecutionTimelinePage}
          to="/execution-timeline"
        >
          Execution
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
              <Route
                path="/integrity"
                element={<IntegrityPage client={client} sourceMode={sourceMode} />}
              />
              <Route
                path="/execution-timeline"
                element={<ExecutionTimelinePage client={client} sourceMode={sourceMode} />}
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
                <span>Point-in-time replay across recv_time and exchange_time.</span>
              </li>
              <li>
                <strong>Integrity</strong>
                <span>Dataset continuity, gap counts, and validation outcomes.</span>
              </li>
              <li>
                <strong>Execution Timeline</strong>
                <span>Read-only order lifecycle inspection with latency traces.</span>
              </li>
            </ul>
          </SectionCard>

          <SectionCard title="Deferred from this pass">
            <ul className="deferred-list">
              <li>
                <strong>Latency</strong>
                <span>Reserved for later metrics-backed summaries.</span>
              </li>
              <li>
                <strong>Query Workbench</strong>
                <span>Read-only SQL over split datasets; backend route pending.</span>
              </li>
              <li>
                <strong>Streaming transport</strong>
                <span>WebSocket order-book streaming is now available on the backend.</span>
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

function preloadIntegrityPage() {
  void loadIntegrityPage()
}

function preloadExecutionTimelinePage() {
  void loadExecutionTimelinePage()
}

export default WorkstationApp
