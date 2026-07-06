import { useMemo, useState } from 'react'
import { useActiveAssets, useOrderBookSnapshot } from '../../shared/api/queries'
import { Badge, Card, CardHeader, ErrorBanner, MetricCard, Skeleton } from '../../shared/components'
import { useKeyboardShortcuts } from '../../shared/hooks/use-keyboard-shortcut'
import { type StreamStatus, useOrderBookStream } from '../../shared/hooks/use-orderbook-stream'
import { formatLevel, formatNumber, formatTimestamp } from '../../shared/lib/formatters'
import type { LiveOrderBookSnapshot } from '../../types'
import { DepthChart } from './components/depth-chart'
import { PriceLadder } from './components/price-ladder'

const DEPTH_PRESETS = [5, 10, 25, 50, 100, 200] as const

export default function OrderbookPage() {
  const [selectedAssetId, setSelectedAssetId] = useState('')
  const [depth, setDepth] = useState<number>(10)

  // Keyboard shortcuts: 1-6 for depth presets
  const depthShortcuts = useMemo(
    () =>
      DEPTH_PRESETS.map((d, i) => ({
        key: String(i + 1),
        handler: () => setDepth(d),
      })),
    [],
  )
  useKeyboardShortcuts(depthShortcuts)
  const assetsQuery = useActiveAssets()
  const assets = assetsQuery.data ?? []
  const effectiveAssetId = assets.find((a) => a.asset_id === selectedAssetId)
    ? selectedAssetId
    : (assets[0]?.asset_id ?? '')

  const snapshotQuery = useOrderBookSnapshot(effectiveAssetId, depth)
  const { snapshot: wsSnapshot, status: wsStatus } = useOrderBookStream(effectiveAssetId || null)

  // Merge WS data with HTTP snapshot. Memoized so a new object identity is only
  // produced when an input actually changes — otherwise every render (including
  // unrelated state changes) would hand memoized children new props and force
  // them to re-render on the high-frequency live path.
  const httpSnapshot = snapshotQuery.data
  const liveSnapshot: LiveOrderBookSnapshot | null = useMemo(
    () =>
      wsStatus === 'connected' && wsSnapshot && wsSnapshot.asset_id === effectiveAssetId
        ? {
            asset_id: wsSnapshot.asset_id,
            sequence: wsSnapshot.sequence,
            last_update_us: wsSnapshot.last_update_us,
            best_bid: wsSnapshot.bids[0] ?? null,
            best_ask: wsSnapshot.asks[0] ?? null,
            mid_price: wsSnapshot.mid_price,
            spread: wsSnapshot.spread,
            // True totals from the WS message, not the depth-capped array lengths.
            bid_depth: wsSnapshot.bid_depth,
            ask_depth: wsSnapshot.ask_depth,
            bids: wsSnapshot.bids,
            asks: wsSnapshot.asks,
            stale: httpSnapshot?.stale ?? false,
            latest_warning: httpSnapshot?.latest_warning ?? null,
          }
        : (httpSnapshot ?? null),
    [wsStatus, wsSnapshot, httpSnapshot, effectiveAssetId],
  )

  const transportLabel =
    wsStatus === 'connected'
      ? 'WebSocket (live)'
      : wsStatus === 'connecting' || wsStatus === 'reconnecting'
        ? `WebSocket (${wsStatus})`
        : 'HTTP polling'

  return (
    <div className="grid gap-[var(--density-gap)]">
      {/* Hero */}
      <div className="flex items-start justify-between rounded-xl border border-card-border bg-card p-6">
        <div>
          <p className="mb-2 text-xs font-medium tracking-widest text-accent uppercase">
            Orderbook
          </p>
          <h1 id="page-heading" className="m-0 text-xl font-bold">
            Institutional-grade orderbook visualization with price ladder and depth chart.
          </h1>
        </div>
        <TransportBadge status={wsStatus} />
      </div>

      {snapshotQuery.error ? (
        <ErrorBanner
          title="Orderbook fetch failed"
          message={
            snapshotQuery.error instanceof Error ? snapshotQuery.error.message : 'Unknown error'
          }
        />
      ) : null}

      {/* Controls */}
      <div className="flex flex-wrap items-center gap-3">
        {/* Asset selector */}
        <div className="flex flex-wrap gap-2">
          {assets.map((asset) => (
            <button
              key={asset.asset_id}
              type="button"
              onClick={() => setSelectedAssetId(asset.asset_id)}
              className={`rounded-full border px-4 py-2 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background ${
                effectiveAssetId === asset.asset_id
                  ? 'border-ring bg-accent/18 text-foreground font-bold'
                  : 'border-card-border text-muted-foreground hover:border-ring/50'
              }`}
            >
              {asset.asset_id}
            </button>
          ))}
        </div>

        {/* Depth presets */}
        <div className="ml-auto flex gap-1.5">
          {DEPTH_PRESETS.map((d) => (
            <button
              key={d}
              type="button"
              onClick={() => setDepth(d)}
              className={`rounded-lg px-3 py-1.5 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background ${
                depth === d
                  ? 'bg-accent text-accent-foreground font-bold'
                  : 'bg-muted text-muted-foreground hover:text-foreground'
              }`}
            >
              {d}
            </button>
          ))}
        </div>
      </div>

      {/* Metrics */}
      {liveSnapshot ? (
        <div className="grid grid-cols-2 gap-[var(--density-gap-sm)] md:grid-cols-4 lg:grid-cols-8">
          <MetricCard label="Best bid" value={formatLevel(liveSnapshot.best_bid)} />
          <MetricCard label="Best ask" value={formatLevel(liveSnapshot.best_ask)} />
          <MetricCard label="Mid" value={formatNumber(liveSnapshot.mid_price)} />
          <MetricCard label="Spread" value={formatNumber(liveSnapshot.spread)} />
          <MetricCard label="Bid depth" value={String(liveSnapshot.bid_depth)} />
          <MetricCard label="Ask depth" value={String(liveSnapshot.ask_depth)} />
          <MetricCard label="Sequence" value={String(liveSnapshot.sequence)} />
          <MetricCard label="Updated" value={formatTimestamp(liveSnapshot.last_update_us)} />
        </div>
      ) : snapshotQuery.isLoading ? (
        <div className="grid grid-cols-4 gap-[var(--density-gap-sm)]">
          {Array.from({ length: 8 }).map((_, i) => (
            <Skeleton key={i} className="h-16 rounded-lg" />
          ))}
        </div>
      ) : null}

      {/* Orderbook visualization */}
      {liveSnapshot ? (
        <div className="grid gap-[var(--density-gap)] lg:grid-cols-2">
          <Card>
            <CardHeader title="Price Ladder">
              <span className="text-sm text-muted-foreground">{transportLabel}</span>
            </CardHeader>
            <PriceLadder bids={liveSnapshot.bids} asks={liveSnapshot.asks} />
          </Card>

          <Card>
            <CardHeader title="Depth Chart">
              <span className="text-sm text-muted-foreground">Cumulative size</span>
            </CardHeader>
            <DepthChart bids={liveSnapshot.bids} asks={liveSnapshot.asks} />
          </Card>
        </div>
      ) : !effectiveAssetId ? (
        <Card>
          <p className="text-muted-foreground">Select an asset to view the orderbook.</p>
        </Card>
      ) : null}
    </div>
  )
}

function TransportBadge({ status }: { status: StreamStatus }) {
  const variant =
    status === 'connected'
      ? 'success'
      : status === 'connecting' || status === 'reconnecting'
        ? 'warning'
        : 'neutral'
  const label =
    status === 'connected'
      ? 'WebSocket'
      : status === 'fallback'
        ? 'HTTP Fallback'
        : titleCase(status)
  return <Badge variant={variant}>{label}</Badge>
}

function titleCase(s: string): string {
  return s
    .split('_')
    .map((p) => p.charAt(0).toUpperCase() + p.slice(1))
    .join(' ')
}
