import { memo, useMemo } from 'react'
import { Badge, Card, CardHeader, MetricCard } from '../../../shared/components'
import { formatPrice, formatSize, formatTimestamp } from '../../../shared/lib/formatters'
import type { PriceLevelView, ReplayReconstructionResponse } from '../../../types'

interface ReplayComparisonProps {
  recvTimeResult?: ReplayReconstructionResponse | null
  exchangeTimeResult?: ReplayReconstructionResponse | null
}

interface LevelDiff {
  price: string
  recvSize: string | null
  exchSize: string | null
  status: 'match' | 'size_diff' | 'recv_only' | 'exch_only'
}

function buildDiffMap(recvLevels: PriceLevelView[], exchLevels: PriceLevelView[]): LevelDiff[] {
  const recvMap = new Map(recvLevels.map((l) => [l.price, l.size]))
  const exchMap = new Map(exchLevels.map((l) => [l.price, l.size]))
  const allPrices = new Set([...recvMap.keys(), ...exchMap.keys()])

  const diffs: LevelDiff[] = []
  for (const price of allPrices) {
    const recvSize = recvMap.get(price) ?? null
    const exchSize = exchMap.get(price) ?? null
    let status: LevelDiff['status'] = 'match'
    if (recvSize && !exchSize) status = 'recv_only'
    else if (!recvSize && exchSize) status = 'exch_only'
    else if (recvSize !== exchSize) status = 'size_diff'
    diffs.push({ price, recvSize, exchSize, status })
  }

  // Sort descending by price for consistent display
  diffs.sort((a, b) => Number.parseFloat(b.price) - Number.parseFloat(a.price))
  return diffs
}

const statusRowClass: Record<LevelDiff['status'], string> = {
  match: '',
  size_diff: 'bg-warning/8',
  recv_only: 'bg-success/8',
  exch_only: 'bg-destructive/8',
}

const DiffTable = memo(function DiffTable({
  diffs,
  side,
}: {
  diffs: LevelDiff[]
  side: 'bid' | 'ask'
}) {
  if (diffs.length === 0) {
    return <p className="text-sm text-muted-foreground">No {side} levels.</p>
  }

  return (
    <div>
      <h4 className="mb-2 text-sm font-bold text-muted-foreground">
        {side === 'bid' ? 'Bids' : 'Asks'}
      </h4>
      <div className="grid gap-0">
        {/* Header */}
        <div className="grid grid-cols-3 gap-2 border-b border-card-border px-2 py-1 text-xs font-medium text-muted-foreground">
          <span>Price</span>
          <span className="text-right">recv_time</span>
          <span className="text-right">exchange_time</span>
        </div>
        {diffs.map((d) => (
          <div
            key={`${side}-${d.price}`}
            className={`grid grid-cols-3 gap-2 rounded px-2 py-1 text-sm ${statusRowClass[d.status]}`}
          >
            <span className={`font-mono ${side === 'bid' ? 'text-success' : 'text-destructive'}`}>
              {formatPrice(d.price)}
            </span>
            <span className="text-right font-mono text-muted-foreground">
              {d.recvSize ? formatSize(d.recvSize) : '---'}
            </span>
            <span className="text-right font-mono text-muted-foreground">
              {d.exchSize ? formatSize(d.exchSize) : '---'}
            </span>
          </div>
        ))}
      </div>
    </div>
  )
})

export const ReplayComparison = memo(function ReplayComparison({
  recvTimeResult,
  exchangeTimeResult,
}: ReplayComparisonProps) {
  const bidDiffs = useMemo(() => {
    const recvBids = recvTimeResult?.bids ?? []
    const exchBids = exchangeTimeResult?.bids ?? []
    return buildDiffMap(recvBids, exchBids)
  }, [recvTimeResult?.bids, exchangeTimeResult?.bids])

  const askDiffs = useMemo(() => {
    const recvAsks = recvTimeResult?.asks ?? []
    const exchAsks = exchangeTimeResult?.asks ?? []
    return buildDiffMap(recvAsks, exchAsks)
  }, [recvTimeResult?.asks, exchangeTimeResult?.asks])

  const summary = useMemo(() => {
    const allDiffs = [...bidDiffs, ...askDiffs]
    const differingLevels = allDiffs.filter((d) => d.status !== 'match').length
    const totalLevels = allDiffs.length

    let maxPriceDeviation = 0
    const recvMid = recvTimeResult?.mid_price
    const exchMid = exchangeTimeResult?.mid_price
    if (recvMid != null && exchMid != null) {
      maxPriceDeviation = Math.abs(recvMid - exchMid)
    }

    return { differingLevels, totalLevels, maxPriceDeviation }
  }, [bidDiffs, askDiffs, recvTimeResult?.mid_price, exchangeTimeResult?.mid_price])

  const hasRecv = recvTimeResult != null
  const hasExch = exchangeTimeResult != null

  if (!hasRecv && !hasExch) {
    return (
      <Card>
        <CardHeader title="Comparison View" />
        <p className="text-muted-foreground">
          Run two replay queries (recv_time and exchange_time) to compare.
        </p>
      </Card>
    )
  }

  return (
    <div className="grid gap-[var(--density-gap)]">
      {/* Timestamp header */}
      <Card dense>
        <div className="grid grid-cols-2 gap-[var(--density-gap)]">
          <div>
            <span className="text-xs font-medium text-muted-foreground">recv_time</span>
            <p className="mt-1 text-sm font-mono text-foreground">
              {hasRecv ? formatTimestamp(recvTimeResult.last_update_us) : 'Not loaded'}
            </p>
            {hasRecv && (
              <Badge variant={recvTimeResult.used_checkpoint ? 'success' : 'neutral'}>
                {recvTimeResult.used_checkpoint ? 'checkpoint' : 'snapshot only'}
              </Badge>
            )}
          </div>
          <div>
            <span className="text-xs font-medium text-muted-foreground">exchange_time</span>
            <p className="mt-1 text-sm font-mono text-foreground">
              {hasExch ? formatTimestamp(exchangeTimeResult.last_update_us) : 'Not loaded'}
            </p>
            {hasExch && (
              <Badge variant={exchangeTimeResult.used_checkpoint ? 'success' : 'neutral'}>
                {exchangeTimeResult.used_checkpoint ? 'checkpoint' : 'snapshot only'}
              </Badge>
            )}
          </div>
        </div>
      </Card>

      {/* Side-by-side ladders with diff highlighting */}
      <Card>
        <CardHeader title="Level Comparison">
          <Badge variant={summary.differingLevels > 0 ? 'warning' : 'success'}>
            {summary.differingLevels} / {summary.totalLevels} differ
          </Badge>
        </CardHeader>
        <div className="grid grid-cols-2 gap-[var(--density-gap)]">
          <DiffTable diffs={bidDiffs} side="bid" />
          <DiffTable diffs={askDiffs} side="ask" />
        </div>
      </Card>

      {/* Summary metrics */}
      <div className="grid grid-cols-3 gap-[var(--density-gap-sm)]">
        <MetricCard
          label="Differing Levels"
          value={`${summary.differingLevels} / ${summary.totalLevels}`}
        />
        <MetricCard
          label="Mid Price Deviation"
          value={
            summary.maxPriceDeviation > 0
              ? summary.maxPriceDeviation.toFixed(6)
              : hasRecv && hasExch
                ? '0'
                : '---'
          }
        />
        <MetricCard
          label="Sequence Delta"
          value={
            hasRecv && hasExch
              ? String(Math.abs(recvTimeResult.sequence - exchangeTimeResult.sequence))
              : '---'
          }
        />
      </div>
    </div>
  )
})
