import { lazy, Suspense, useState } from 'react'
import { BrowserRouter, Navigate, NavLink, Route, Routes } from 'react-router-dom'
import { Badge, Button, CardSkeleton } from '../shared/components'
import { CommandPalette } from '../shared/components/command-palette'
import { ShortcutHelp } from '../shared/components/shortcut-help'
import { useFocusOnNavigate } from '../shared/hooks/use-focus-on-navigate'
import { useKeyboardShortcut } from '../shared/hooks/use-keyboard-shortcut'
import { useSourceMode } from '../shared/hooks/use-source-mode'
import type { Density, Theme } from '../shared/hooks/use-theme'
import { useTheme } from '../shared/hooks/use-theme'
import { APP_VERSION_LABEL } from '../shared/lib/constants'
import type { DataSourceMode } from '../types'
import { ErrorBoundary } from './error-boundary'
import { Providers } from './providers'

// Lazy-loaded pages
const loadLiveFeedPage = () => import('../features/live-feed/LiveFeedPage')
const loadOrderbookPage = () => import('../features/orderbook/OrderbookPage')
const loadReplayPage = () => import('../features/replay/ReplayPage')
const loadExecutionPage = () => import('../features/execution/ExecutionPage')
const loadIntegrityPage = () => import('../features/integrity/IntegrityPage')
const loadQueryPage = () => import('../features/query/QueryPage')

const LiveFeedPage = lazy(loadLiveFeedPage)
const OrderbookPage = lazy(loadOrderbookPage)
const ReplayPage = lazy(loadReplayPage)
const ExecutionPage = lazy(loadExecutionPage)
const IntegrityPage = lazy(loadIntegrityPage)
const QueryPage = lazy(loadQueryPage)

const navItems = [
  { to: '/live-feed', label: 'Live Feed', preload: loadLiveFeedPage },
  { to: '/orderbook', label: 'Orderbook', preload: loadOrderbookPage },
  { to: '/replay', label: 'Replay', preload: loadReplayPage },
  { to: '/execution', label: 'Execution', preload: loadExecutionPage },
  { to: '/integrity', label: 'Integrity', preload: loadIntegrityPage },
  { to: '/query', label: 'Query', preload: loadQueryPage },
]

export default function App() {
  return (
    <ErrorBoundary level="root">
      <Providers>
        <BrowserRouter>
          <AppShell />
        </BrowserRouter>
      </Providers>
    </ErrorBoundary>
  )
}

const GLOBAL_SHORTCUTS = [
  { key: '⌘K', description: 'Open command palette' },
  { key: '?', description: 'Show keyboard shortcuts' },
  { key: '1-6', description: 'Depth presets (Orderbook page)' },
]

function AppShell() {
  const { theme, toggleTheme, density, setDensity } = useTheme()
  const { sourceMode, setSourceMode } = useSourceMode()
  const [shortcutHelpOpen, setShortcutHelpOpen] = useState(false)
  useFocusOnNavigate()
  useKeyboardShortcut({
    key: '?',
    handler: () => setShortcutHelpOpen((prev) => !prev),
  })

  return (
    <div className="mx-auto max-w-[1600px] p-8">
      <Header
        theme={theme}
        onToggleTheme={toggleTheme}
        density={density}
        onDensityChange={setDensity}
        sourceMode={sourceMode}
        onSourceModeChange={setSourceMode}
      />

      <nav className="mb-6 flex flex-wrap gap-2" aria-label="primary">
        {navItems.map((item) => (
          <NavLink
            key={item.to}
            to={item.to}
            className={({ isActive }) =>
              `rounded-full border px-4 py-2.5 text-sm transition-colors ${
                isActive
                  ? 'border-ring bg-accent/18 text-foreground'
                  : 'border-card-border text-muted-foreground hover:border-ring/50 hover:text-foreground'
              }`
            }
            onMouseEnter={() => void item.preload()}
            onFocus={() => void item.preload()}
          >
            {item.label}
          </NavLink>
        ))}
      </nav>

      <main id="main-content">
        <ErrorBoundary level="route">
          <Suspense fallback={<RouteLoadingSkeleton />}>
            <Routes>
              <Route path="/" element={<Navigate replace to="/live-feed" />} />
              <Route path="/live-feed" element={<LiveFeedPage />} />
              <Route path="/orderbook" element={<OrderbookPage />} />
              <Route path="/replay" element={<ReplayPage />} />
              <Route path="/execution" element={<ExecutionPage />} />
              <Route path="/integrity" element={<IntegrityPage />} />
              <Route path="/query" element={<QueryPage />} />
            </Routes>
          </Suspense>
        </ErrorBoundary>
      </main>

      <CommandPalette
        onToggleTheme={toggleTheme}
        onSetDensity={setDensity}
        onSetSource={setSourceMode}
      />
      <ShortcutHelp
        open={shortcutHelpOpen}
        onOpenChange={setShortcutHelpOpen}
        shortcuts={GLOBAL_SHORTCUTS}
      />
    </div>
  )
}

function Header({
  theme,
  onToggleTheme,
  density,
  onDensityChange,
  sourceMode,
  onSourceModeChange,
}: {
  theme: Theme
  onToggleTheme: () => void
  density: Density
  onDensityChange: (d: Density) => void
  sourceMode: DataSourceMode
  onSourceModeChange: (m: DataSourceMode) => void
}) {
  return (
    <header className="mb-6 flex flex-wrap items-end justify-between gap-6">
      <div>
        <p className="mb-2 text-xs font-medium tracking-widest text-accent uppercase">
          Poly-book Quant Workstation
        </p>
        <h1 className="m-0 text-2xl font-bold text-foreground">Quant Workstation</h1>
        <p className="mt-2 max-w-[72ch] text-muted-foreground">
          Institutional-grade read-only workstation: live orderbooks, replay, execution inspection,
          integrity analysis, and SQL workbench.
        </p>
      </div>

      <div className="flex flex-wrap items-center gap-3">
        {/* Source mode toggle */}
        {/* biome-ignore lint/a11y/useSemanticElements: toggle group, not a form fieldset */}
        <div
          className="flex rounded-full border border-card-border bg-card p-1"
          role="group"
          aria-label="data source"
        >
          <button
            type="button"
            onClick={() => onSourceModeChange('api')}
            className={`rounded-full px-4 py-2 text-sm transition-colors ${
              sourceMode === 'api'
                ? 'bg-gradient-to-br from-teal-700 to-cyan-600 font-bold text-white'
                : 'text-muted-foreground'
            }`}
          >
            Live API
          </button>
          <button
            type="button"
            onClick={() => onSourceModeChange('demo')}
            className={`rounded-full px-4 py-2 text-sm transition-colors ${
              sourceMode === 'demo'
                ? 'bg-gradient-to-br from-teal-700 to-cyan-600 font-bold text-white'
                : 'text-muted-foreground'
            }`}
          >
            Demo
          </button>
        </div>

        {/* Theme toggle */}
        <Button variant="ghost" size="sm" onClick={onToggleTheme}>
          {theme === 'dark' ? 'Light' : 'Dark'}
        </Button>

        {/* Density selector */}
        <select
          value={density}
          onChange={(e) => onDensityChange(e.target.value as Density)}
          className="rounded-lg border border-card-border bg-card px-3 py-2 text-sm text-foreground"
          aria-label="density mode"
        >
          <option value="compact">Compact</option>
          <option value="comfortable">Comfortable</option>
          <option value="spacious">Spacious</option>
        </select>

        <Badge variant="neutral">{APP_VERSION_LABEL}</Badge>
      </div>
    </header>
  )
}

function RouteLoadingSkeleton() {
  return (
    <div className="grid gap-4">
      <CardSkeleton />
      <CardSkeleton />
    </div>
  )
}
