import { useCallback, useMemo, useState } from 'react'
import { liveHiddenPollIntervalMs, liveVisiblePollIntervalMs } from './constants'
import { isAbortError } from './client'
import {
  formatClientTimestamp,
  formatIntervalMs,
  formatLevel,
  formatNumber,
  formatTimestamp,
  titleCase,
} from './formatters'
import type {
  ActiveAssetSummary,
  DataSourceMode,
  FeedStatusResponse,
  LiveOrderBookSnapshot,
  WorkstationClient,
} from './types'
import { useAdaptivePolling } from './useAdaptivePolling'
import { useOrderBookStream } from './useOrderBookStream'
import { ErrorBanner, MetricCard, OrderBookTable, SectionCard, WarningPanel } from './ui'

interface LoadState<T> {
  data: T | null
  loading: boolean
  error: string | null
}

export default function LiveFeedPage({
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

  const refreshFeed = useCallback(
    async (signal: AbortSignal) => {
      setFeedState((previous) => ({ ...previous, loading: true, error: null }))
      setAssetsState((previous) => ({ ...previous, loading: true, error: null }))

      try {
        const [feedStatus, activeAssets] = await Promise.all([
          client.getFeedStatus({ signal }),
          client.getActiveAssets({ signal }),
        ])
        setFeedState({ data: feedStatus, loading: false, error: null })
        setAssetsState({ data: activeAssets, loading: false, error: null })
      } catch (error) {
        if (isAbortError(error)) {
          return
        }

        const message = error instanceof Error ? error.message : 'Unable to load live feed data.'
        setFeedState((previous) => ({ ...previous, loading: false, error: message }))
        setAssetsState((previous) => ({ ...previous, loading: false, error: message }))
      }
    },
    [client],
  )

  const feedPolling = useAdaptivePolling({
    visibleIntervalMs: liveVisiblePollIntervalMs,
    hiddenIntervalMs: liveHiddenPollIntervalMs,
    runTask: refreshFeed,
  })

  const assetIds = assetsState.data?.map((asset) => asset.asset_id) ?? []
  const selectedAssetId = assetIds.includes(userSelectedAssetId) ? userSelectedAssetId : assetIds[0] ?? ''

  const refreshSnapshot = useCallback(
    async (signal: AbortSignal) => {
      if (!selectedAssetId) {
        return
      }

      setSnapshotState((previous) => ({ ...previous, loading: true, error: null }))

      try {
        const snapshot = await client.getOrderBookSnapshot(selectedAssetId, depth, { signal })
        setSnapshotState({ data: snapshot, loading: false, error: null })
      } catch (error) {
        if (isAbortError(error)) {
          return
        }

        const message =
          error instanceof Error ? error.message : 'Unable to load the selected order book.'
        setSnapshotState((previous) => ({ ...previous, loading: false, error: message }))
      }
    },
    [client, depth, selectedAssetId],
  )

  const wsAssetId = sourceMode === 'api' ? selectedAssetId || null : null
  const { snapshot: wsSnapshot, status: wsStatus } = useOrderBookStream(wsAssetId)
  const useWs = sourceMode === 'api' && wsStatus !== 'fallback' && wsStatus !== 'closed'

  const snapshotPolling = useAdaptivePolling({
    enabled: Boolean(selectedAssetId) && !useWs,
    visibleIntervalMs: liveVisiblePollIntervalMs,
    hiddenIntervalMs: liveHiddenPollIntervalMs,
    runTask: refreshSnapshot,
  })

  const wsAsLiveSnapshot: LiveOrderBookSnapshot | null = useMemo(() => {
    if (!wsSnapshot || wsSnapshot.asset_id !== selectedAssetId) return null
    return {
      asset_id: wsSnapshot.asset_id,
      sequence: wsSnapshot.sequence,
      last_update_us: wsSnapshot.last_update_us,
      best_bid: wsSnapshot.bids[0] ?? null,
      best_ask: wsSnapshot.asks[0] ?? null,
      mid_price: wsSnapshot.mid_price,
      spread: wsSnapshot.spread,
      bid_depth: wsSnapshot.bids.length,
      ask_depth: wsSnapshot.asks.length,
      bids: wsSnapshot.bids,
      asks: wsSnapshot.asks,
      stale: false,
      latest_warning: null,
    }
  }, [wsSnapshot, selectedAssetId])

  const liveSnapshot = useWs
    ? wsAsLiveSnapshot
    : snapshotState.data?.asset_id === selectedAssetId
      ? snapshotState.data
      : null

  const transportLabel = useWs
    ? `WebSocket (${wsStatus})`
    : sourceMode === 'api'
      ? 'Adaptive HTTP polling'
      : 'Demo fixtures'

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
          hint={
            sourceMode === 'api'
              ? 'If `serve-api` is not running, switch to Demo data to review the SPA.'
              : undefined
          }
          message={feedState.error}
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
        <MetricCard label="Transport" value={transportLabel} />
        <MetricCard
          label="Cadence"
          value={`${formatIntervalMs(liveVisiblePollIntervalMs)} fg / ${formatIntervalMs(liveHiddenPollIntervalMs)} bg`}
        />
        <MetricCard label="Session ID" value={feedState.data?.current_session_id ?? '—'} />
        <MetricCard label="Last rotation" value={formatTimestamp(feedState.data?.last_rotation_us)} />
        <MetricCard
          label="Last refresh"
          value={formatClientTimestamp(feedPolling.lastCompletedAtMs)}
        />
        <MetricCard
          label="Page state"
          value={feedPolling.isVisible ? 'Foreground' : 'Background'}
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
          toolbar={
            <span className="muted">
              {sourceMode === 'api'
                ? `Adaptive refresh · ${formatIntervalMs(snapshotPolling.currentIntervalMs)}`
                : 'Fixture-backed snapshot'}
            </span>
          }
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
