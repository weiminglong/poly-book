import { useCallback, useState } from 'react'
import { useActiveAssets, useReplayReconstruction } from '../../shared/api/queries'
import {
  Badge,
  Button,
  Card,
  CardHeader,
  ErrorBanner,
  Input,
  MetricCard,
  Select,
} from '../../shared/components'
import { ContinuityTimeline } from '../../shared/components/continuity-timeline'
import { formatNumber, formatTimestamp } from '../../shared/lib/formatters'
import type { ReplayReconstructionResponse, ReplayRequest } from '../../types'
import { PriceLadder } from '../orderbook/components/price-ladder'
import { ReplayComparison } from './components/replay-comparison'

type ViewMode = 'single' | 'comparison'

export default function ReplayPage() {
  const [assetId, setAssetId] = useState('btc-5m-yes')
  const [atUs, setAtUs] = useState('')
  const [mode, setMode] = useState<'recv_time' | 'exchange_time'>('recv_time')
  const [depth, setDepth] = useState(5)
  const [request, setRequest] = useState<ReplayRequest | null>(null)
  const [viewMode, setViewMode] = useState<ViewMode>('single')

  // Comparison mode: fire both recv_time and exchange_time queries
  const [compRequest, setCompRequest] = useState<{
    assetId: string
    atUs: number
    depth: number
  } | null>(null)
  const recvQuery = useReplayReconstruction(
    compRequest ? { ...compRequest, mode: 'recv_time' as const } : null,
  )
  const exchQuery = useReplayReconstruction(
    compRequest ? { ...compRequest, mode: 'exchange_time' as const } : null,
  )

  const assetsQuery = useActiveAssets()
  const assets = assetsQuery.data ?? []
  const replayQuery = useReplayReconstruction(viewMode === 'single' ? request : null)
  const result = replayQuery.data

  const handleSubmit = useCallback(
    (e: React.FormEvent) => {
      e.preventDefault()
      const ts = Number(atUs)
      if (!assetId.trim() || !Number.isFinite(ts) || ts <= 0 || depth <= 0) return

      if (viewMode === 'single') {
        setRequest({ assetId: assetId.trim(), atUs: ts, mode, depth })
        setCompRequest(null)
      } else {
        setCompRequest({ assetId: assetId.trim(), atUs: ts, depth })
        setRequest(null)
      }
    },
    [assetId, atUs, mode, depth, viewMode],
  )

  const isFetching =
    viewMode === 'single' ? replayQuery.isFetching : recvQuery.isFetching || exchQuery.isFetching

  const queryError =
    viewMode === 'single' ? replayQuery.error : (recvQuery.error ?? exchQuery.error)

  return (
    <div className="grid gap-[var(--density-gap)]">
      {/* Hero */}
      <div className="flex items-start justify-between rounded-xl border border-card-border bg-card p-6">
        <div>
          <p className="mb-2 text-xs font-medium tracking-widest text-accent uppercase">
            Replay Workbench
          </p>
          <h1 id="page-heading" className="m-0 text-xl font-bold">
            Inspect Parquet-backed reconstruction with explicit time semantics.
          </h1>
        </div>
        <Badge variant="neutral">Parquet only</Badge>
      </div>

      {queryError ? (
        <ErrorBanner
          title="Replay request failed"
          message={queryError instanceof Error ? queryError.message : 'Unknown error'}
          hint="Replay uses Parquet-backed data. Ensure local history exists under ./data."
        />
      ) : null}

      {/* View mode toggle */}
      <div className="flex gap-1 rounded-lg border border-card-border bg-card p-1 self-start">
        <button
          type="button"
          onClick={() => setViewMode('single')}
          className={`rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
            viewMode === 'single'
              ? 'bg-accent/20 text-accent'
              : 'text-muted-foreground hover:text-foreground'
          }`}
        >
          Single View
        </button>
        <button
          type="button"
          onClick={() => setViewMode('comparison')}
          className={`rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
            viewMode === 'comparison'
              ? 'bg-accent/20 text-accent'
              : 'text-muted-foreground hover:text-foreground'
          }`}
        >
          Comparison View
        </button>
      </div>

      <div
        className={`grid gap-[var(--density-gap)] ${viewMode === 'single' ? 'lg:grid-cols-2' : ''}`}
      >
        {/* Query form */}
        <Card>
          <CardHeader title="Replay Query">
            <span className="text-sm text-muted-foreground">
              {viewMode === 'single'
                ? 'Point-in-time reconstruction'
                : 'recv_time vs exchange_time'}
            </span>
          </CardHeader>
          <form className="grid gap-[var(--density-gap-sm)]" onSubmit={handleSubmit}>
            <Input
              label="Asset ID"
              value={assetId}
              onChange={(e) => setAssetId(e.target.value)}
              placeholder="btc-5m-yes"
            />
            <Input
              label="Timestamp (us)"
              value={atUs}
              onChange={(e) => setAtUs(e.target.value)}
              placeholder="1700000000000000"
            />
            {viewMode === 'single' && (
              <Select
                label="Mode"
                value={mode}
                onChange={(e) => setMode(e.target.value as 'recv_time' | 'exchange_time')}
                options={[
                  { value: 'recv_time', label: 'recv_time' },
                  { value: 'exchange_time', label: 'exchange_time' },
                ]}
              />
            )}
            <Input
              label="Depth"
              type="number"
              min={1}
              max={200}
              value={depth}
              onChange={(e) => setDepth(Number(e.target.value) || 1)}
            />
            <Button type="submit" disabled={isFetching}>
              {isFetching
                ? 'Running...'
                : viewMode === 'single'
                  ? 'Run Reconstruction'
                  : 'Run Comparison'}
            </Button>
          </form>

          {/* Asset chips */}
          {assets.length > 0 && (
            <div className="mt-4 flex flex-wrap gap-2">
              {assets.map((a) => (
                <button
                  key={a.asset_id}
                  type="button"
                  onClick={() => setAssetId(a.asset_id)}
                  className="rounded-full border border-ring/40 bg-accent/12 px-3 py-1.5 text-sm text-accent transition-transform hover:-translate-y-0.5"
                >
                  {a.asset_id}
                </button>
              ))}
            </div>
          )}
        </Card>

        {/* Single view result */}
        {viewMode === 'single' && (
          <Card>
            <CardHeader title="Replay Result" />
            {replayQuery.isFetching && !result ? (
              <p className="text-muted-foreground">Loading reconstruction...</p>
            ) : null}
            {!request && !result ? (
              <p className="text-muted-foreground">
                Submit a replay query to inspect the reconstructed top of book.
              </p>
            ) : null}
            {result ? <SingleResult result={result} /> : null}
          </Card>
        )}
      </div>

      {/* Comparison view result */}
      {viewMode === 'comparison' && (
        <ReplayComparison recvTimeResult={recvQuery.data} exchangeTimeResult={exchQuery.data} />
      )}
    </div>
  )
}

function SingleResult({ result }: { result: ReplayReconstructionResponse }) {
  return (
    <div className="grid gap-[var(--density-gap-sm)]">
      <div className="grid grid-cols-3 gap-[var(--density-gap-sm)]">
        <MetricCard label="Asset" value={result.asset_id} />
        <MetricCard label="Mode" value={result.mode} />
        <MetricCard
          label="Checkpoint"
          value={result.used_checkpoint ? 'Used checkpoint' : 'Snapshot only'}
        />
      </div>
      <div className="grid grid-cols-3 gap-[var(--density-gap-sm)]">
        <MetricCard label="Sequence" value={String(result.sequence)} />
        <MetricCard label="Updated" value={formatTimestamp(result.last_update_us)} />
        <MetricCard label="Mid" value={formatNumber(result.mid_price)} />
      </div>

      <PriceLadder bids={result.bids} asks={result.asks} />

      <Card dense>
        <CardHeader title="Continuity Events" />
        <ContinuityTimeline events={result.continuity_events} />
      </Card>
    </div>
  )
}
