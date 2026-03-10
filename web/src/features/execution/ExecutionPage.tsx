import { useCallback, useState } from 'react'
import { useExecutionTimeline } from '../../shared/api/queries'
import {
  Badge,
  Button,
  Card,
  CardHeader,
  ErrorBanner,
  Input,
  MetricCard,
} from '../../shared/components'
import { formatPrice, formatSize, formatTimestamp, titleCase } from '../../shared/lib/formatters'
import type { ExecutionEventView, ExecutionRequest } from '../../types'
import { LatencyWaterfall } from './components/latency-waterfall'

export default function ExecutionPage() {
  const [orderId, setOrderId] = useState('')
  const [assetId, setAssetId] = useState('')
  const [windowMinutes, setWindowMinutes] = useState(5)
  const [request, setRequest] = useState<ExecutionRequest | null>(null)
  const [expandedRow, setExpandedRow] = useState<number | null>(null)
  const [page, setPage] = useState(0)
  const PAGE_SIZE = 50

  const execQuery = useExecutionTimeline(request)
  const result = execQuery.data

  const handleSubmit = useCallback(
    (e: React.FormEvent) => {
      e.preventDefault()
      const now = Date.now() * 1000
      setRequest({
        orderId: orderId || undefined,
        assetId: assetId || undefined,
        startUs: now - windowMinutes * 60_000_000,
        endUs: now,
        limit: 200,
      })
      setPage(0)
      setExpandedRow(null)
    },
    [orderId, assetId, windowMinutes],
  )

  const events = result?.events ?? []
  const pageStart = page * PAGE_SIZE
  const pageEnd = pageStart + PAGE_SIZE
  const pagedEvents = events.slice(pageStart, pageEnd)
  const totalPages = Math.ceil(events.length / PAGE_SIZE)

  return (
    <div className="grid gap-[var(--density-gap)]">
      {/* Hero */}
      <div className="flex items-start justify-between rounded-xl border border-card-border bg-card p-6">
        <div>
          <p className="mb-2 text-xs font-medium tracking-widest text-accent uppercase">
            Execution Inspector
          </p>
          <h1 id="page-heading" className="m-0 text-xl font-bold">
            Read-only order lifecycle inspection with latency traces.
          </h1>
        </div>
        <Badge variant="neutral">Read-only</Badge>
      </div>

      {execQuery.error ? (
        <ErrorBanner
          title="Execution query failed"
          message={execQuery.error instanceof Error ? execQuery.error.message : 'Unknown error'}
        />
      ) : null}

      {/* Query form */}
      <Card>
        <CardHeader title="Execution Query" />
        <form
          className="grid gap-[var(--density-gap-sm)] md:grid-cols-4 md:items-end"
          onSubmit={handleSubmit}
        >
          <Input
            label="Order ID (optional)"
            value={orderId}
            onChange={(e) => setOrderId(e.target.value)}
          />
          <Input
            label="Asset ID (optional)"
            value={assetId}
            onChange={(e) => setAssetId(e.target.value)}
          />
          <Input
            label="Window (minutes)"
            type="number"
            min={1}
            max={1440}
            value={windowMinutes}
            onChange={(e) => setWindowMinutes(Number(e.target.value))}
          />
          <Button type="submit" disabled={execQuery.isFetching}>
            {execQuery.isFetching ? 'Loading...' : 'Query'}
          </Button>
        </form>
      </Card>

      {result ? (
        <>
          <div className="grid grid-cols-2 gap-[var(--density-gap-sm)]">
            <MetricCard label="Total events" value={String(result.total_count)} />
            <MetricCard
              label="Showing"
              value={`${pageStart + 1}–${Math.min(pageEnd, events.length)} of ${events.length}`}
            />
          </div>

          <Card>
            <CardHeader title="Events" />
            <div className="overflow-x-auto">
              <table className="w-full border-collapse whitespace-nowrap">
                <thead>
                  <tr>
                    <th className="border-b border-card-border px-3 py-2.5 text-left text-[var(--density-font-size)] font-bold text-muted-foreground" />
                    <th className="border-b border-card-border px-3 py-2.5 text-left text-[var(--density-font-size)] font-bold text-muted-foreground">
                      Time
                    </th>
                    <th className="border-b border-card-border px-3 py-2.5 text-left text-[var(--density-font-size)] font-bold text-muted-foreground">
                      Order
                    </th>
                    <th className="border-b border-card-border px-3 py-2.5 text-left text-[var(--density-font-size)] font-bold text-muted-foreground">
                      Kind
                    </th>
                    <th className="border-b border-card-border px-3 py-2.5 text-left text-[var(--density-font-size)] font-bold text-muted-foreground">
                      Side
                    </th>
                    <th className="border-b border-card-border px-3 py-2.5 text-left text-[var(--density-font-size)] font-bold text-muted-foreground">
                      Price
                    </th>
                    <th className="border-b border-card-border px-3 py-2.5 text-left text-[var(--density-font-size)] font-bold text-muted-foreground">
                      Size
                    </th>
                    <th className="border-b border-card-border px-3 py-2.5 text-left text-[var(--density-font-size)] font-bold text-muted-foreground">
                      Status
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {pagedEvents.map((ev, i) => {
                    const globalIndex = pageStart + i
                    const isExpanded = expandedRow === globalIndex
                    return (
                      <EventRow
                        key={`${ev.order_id}-${ev.event_timestamp_us}-${globalIndex}`}
                        event={ev}
                        isExpanded={isExpanded}
                        onToggle={() => setExpandedRow(isExpanded ? null : globalIndex)}
                      />
                    )
                  })}
                </tbody>
              </table>
            </div>

            {/* Pagination */}
            {totalPages > 1 && (
              <div className="mt-3 flex items-center justify-between text-sm text-muted-foreground">
                <span>
                  Page {page + 1} of {totalPages}
                </span>
                <div className="flex gap-2">
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setPage((p) => p - 1)}
                    disabled={page === 0}
                  >
                    Previous
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setPage((p) => p + 1)}
                    disabled={page >= totalPages - 1}
                  >
                    Next
                  </Button>
                </div>
              </div>
            )}
          </Card>
        </>
      ) : null}
    </div>
  )
}

function EventRow({
  event,
  isExpanded,
  onToggle,
}: {
  event: ExecutionEventView
  isExpanded: boolean
  onToggle: () => void
}) {
  return (
    <>
      {/* biome-ignore lint/a11y/useSemanticElements: expandable table row requires role="button" */}
      <tr
        className="h-[var(--density-row-height)] cursor-pointer transition-colors hover:bg-muted/50"
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            onToggle()
          }
        }}
        tabIndex={0}
        role="button"
        aria-expanded={isExpanded}
      >
        <td className="border-b border-card-border/60 px-3 py-2 text-[var(--density-font-size)]">
          <span
            className={`inline-block transition-transform ${isExpanded ? 'rotate-90' : ''}`}
            aria-hidden="true"
          >
            ▶
          </span>
        </td>
        <td className="border-b border-card-border/60 px-3 py-2 text-[var(--density-font-size)]">
          {formatTimestamp(event.event_timestamp_us)}
        </td>
        <td className="border-b border-card-border/60 px-3 py-2 font-mono text-[var(--density-font-size)]">
          {event.order_id}
        </td>
        <td className="border-b border-card-border/60 px-3 py-2 text-[var(--density-font-size)]">
          <Badge variant="accent">{titleCase(event.kind)}</Badge>
        </td>
        <td className="border-b border-card-border/60 px-3 py-2 text-[var(--density-font-size)]">
          {event.side ?? '---'}
        </td>
        <td className="border-b border-card-border/60 px-3 py-2 text-[var(--density-font-size)]">
          {event.price ? formatPrice(event.price) : '---'}
        </td>
        <td className="border-b border-card-border/60 px-3 py-2 text-[var(--density-font-size)]">
          {event.size ? formatSize(event.size) : '---'}
        </td>
        <td className="border-b border-card-border/60 px-3 py-2 text-[var(--density-font-size)]">
          {event.status ?? '---'}
        </td>
      </tr>
      {isExpanded && (
        <tr>
          <td colSpan={8} className="border-b border-card-border/60 bg-muted/30 px-6 py-4">
            <LatencyWaterfall latency={event.latency} />
          </td>
        </tr>
      )}
    </>
  )
}
