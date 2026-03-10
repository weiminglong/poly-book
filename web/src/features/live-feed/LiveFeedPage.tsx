import { useState } from 'react'
import {
  BACKGROUND_INTERVAL_MS,
  FOREGROUND_INTERVAL_MS,
  useActiveAssets,
  useFeedStatus,
  useOrderBookSnapshot,
} from '../../shared/api/queries'
import { Badge, Card, CardHeader, ErrorBanner, MetricCard } from '../../shared/components'
import {
  formatIntervalMs,
  formatLevel,
  formatNumber,
  formatTimestamp,
  titleCase,
} from '../../shared/lib/formatters'
import type { ContinuityWarning } from '../../types'

export default function LiveFeedPage() {
  const feedQuery = useFeedStatus()
  const assetsQuery = useActiveAssets()
  const [selectedAssetId, setSelectedAssetId] = useState<string>('')

  const feed = feedQuery.data
  const assets = assetsQuery.data ?? []
  const effectiveAssetId = assets.find((a) => a.asset_id === selectedAssetId)
    ? selectedAssetId
    : (assets[0]?.asset_id ?? '')

  return (
    <div className="grid gap-[var(--density-gap)]">
      {/* Hero card */}
      <div className="flex items-start justify-between rounded-xl border border-card-border bg-card p-6">
        <div>
          <p className="mb-2 text-xs font-medium tracking-widest text-accent uppercase">
            Live Feed
          </p>
          <h1 id="page-heading" className="m-0 text-xl font-bold">
            Monitor the current read model without reconstructing in the browser.
          </h1>
        </div>
        <Badge
          variant={
            feed?.session_status === 'connected'
              ? 'success'
              : feed?.session_status === 'reconnecting'
                ? 'warning'
                : 'neutral'
          }
        >
          {titleCase(feed?.session_status ?? 'starting')}
        </Badge>
      </div>

      {/* Error */}
      {feedQuery.error ? (
        <ErrorBanner
          title="Live API request failed"
          message={feedQuery.error instanceof Error ? feedQuery.error.message : 'Unknown error'}
          hint="If serve-api is not running, switch to Demo data to review the SPA."
        />
      ) : null}

      {/* Stats grid */}
      <div className="grid grid-cols-2 gap-[var(--density-gap-sm)] md:grid-cols-4">
        <MetricCard label="Feed mode" value={feed ? titleCase(feed.mode) : 'Loading...'} />
        <MetricCard
          label="Active assets"
          value={String(feed?.active_asset_count ?? assets.length)}
        />
        <MetricCard label="Session ID" value={feed?.current_session_id ?? '---'} />
        <MetricCard label="Last rotation" value={formatTimestamp(feed?.last_rotation_us)} />
        <MetricCard label="Foreground cadence" value={formatIntervalMs(FOREGROUND_INTERVAL_MS)} />
        <MetricCard label="Background cadence" value={formatIntervalMs(BACKGROUND_INTERVAL_MS)} />
        <MetricCard
          label="Data updated"
          value={
            feedQuery.dataUpdatedAt ? new Date(feedQuery.dataUpdatedAt).toLocaleTimeString() : '---'
          }
        />
        <MetricCard label="Fetch status" value={feedQuery.isFetching ? 'Refreshing...' : 'Idle'} />
      </div>

      {/* Global warning */}
      {feed?.latest_global_warning ? (
        <WarningPanel title="Latest continuity warning" warning={feed.latest_global_warning} />
      ) : null}

      {/* Assets grid */}
      <div className="grid gap-[var(--density-gap)] lg:grid-cols-2">
        <Card>
          <CardHeader title="Active Assets" />
          {assetsQuery.isLoading && !assets.length ? (
            <p className="text-muted-foreground">Loading active assets...</p>
          ) : null}
          {assets.length > 0 ? (
            <div className="grid gap-[var(--density-gap-sm)]">
              {assets.map((asset) => (
                <button
                  key={asset.asset_id}
                  type="button"
                  onClick={() => setSelectedAssetId(asset.asset_id)}
                  className={`flex items-center justify-between gap-4 rounded-lg border p-[var(--density-padding-sm)] text-left transition-colors ${
                    effectiveAssetId === asset.asset_id
                      ? 'border-ring shadow-[inset_0_0_0_1px] shadow-ring/45'
                      : 'border-card-border hover:border-ring/50'
                  }`}
                >
                  <div>
                    <strong className="text-foreground">{asset.asset_id}</strong>
                    <p className="mt-1 text-sm text-muted-foreground">
                      recv {formatTimestamp(asset.last_recv_timestamp_us)} · exchange{' '}
                      {formatTimestamp(asset.last_exchange_timestamp_us)}
                    </p>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <Badge variant={asset.has_book ? 'success' : 'warning'}>
                      {asset.has_book ? 'Book ready' : 'No book'}
                    </Badge>
                    <Badge variant={asset.stale ? 'warning' : 'success'}>
                      {asset.stale ? 'Stale' : 'Fresh'}
                    </Badge>
                  </div>
                </button>
              ))}
            </div>
          ) : null}
          {!assetsQuery.isLoading && !assetsQuery.error && assets.length === 0 ? (
            <p className="text-muted-foreground">
              No active assets are currently exposed by the API.
            </p>
          ) : null}
        </Card>

        <Card>
          <CardHeader title={effectiveAssetId ? `Quick View · ${effectiveAssetId}` : 'Quick View'}>
            <span className="text-sm text-muted-foreground">
              Select an asset for full orderbook on the Orderbook page
            </span>
          </CardHeader>
          {!effectiveAssetId ? (
            <p className="text-muted-foreground">Select an asset to preview.</p>
          ) : (
            <AssetQuickView assetId={effectiveAssetId} />
          )}
        </Card>
      </div>
    </div>
  )
}

function AssetQuickView({ assetId }: { assetId: string }) {
  const { data: snapshot, isLoading, error } = useOrderBookSnapshot(assetId, 5)

  if (isLoading && !snapshot) return <p className="text-muted-foreground">Loading snapshot...</p>
  if (error)
    return <p className="text-destructive">{error instanceof Error ? error.message : 'Error'}</p>
  if (!snapshot) return null

  return (
    <div className="grid gap-[var(--density-gap-sm)]">
      <div className="grid grid-cols-3 gap-[var(--density-gap-sm)]">
        <MetricCard label="Best bid" value={formatLevel(snapshot.best_bid)} />
        <MetricCard label="Best ask" value={formatLevel(snapshot.best_ask)} />
        <MetricCard label="Spread" value={formatNumber(snapshot.spread)} />
      </div>
      <div className="grid grid-cols-2 gap-[var(--density-gap-sm)]">
        <MetricCard label="Mid" value={formatNumber(snapshot.mid_price)} />
        <MetricCard label="Sequence" value={String(snapshot.sequence)} />
      </div>
    </div>
  )
}

function WarningPanel({ title, warning }: { title: string; warning: ContinuityWarning }) {
  return (
    <div className="grid gap-2 rounded-lg border border-[rgba(234,179,8,0.25)] bg-[rgba(234,179,8,0.08)] p-[var(--density-padding-sm)]">
      <div className="flex items-center justify-between gap-3">
        <strong className="text-foreground">{title}</strong>
        <Badge variant="warning">{titleCase(warning.kind)}</Badge>
      </div>
      <p className="m-0 text-muted-foreground">{warning.details ?? 'No extra details supplied.'}</p>
      <p className="m-0 text-sm text-muted-foreground">
        recv {formatTimestamp(warning.recv_timestamp_us)} · exchange{' '}
        {formatTimestamp(warning.exchange_timestamp_us)}
      </p>
    </div>
  )
}
