import { useCallback, useState } from 'react'
import { useIntegritySummary } from '../../shared/api/queries'
import {
  Badge,
  Button,
  Card,
  CardHeader,
  ErrorBanner,
  Input,
  MetricCard,
} from '../../shared/components'
import { ContinuityTimeline } from '../../shared/components/continuity-timeline'
import { formatTimestamp, titleCase } from '../../shared/lib/formatters'
import type { IntegrityRequest } from '../../types'

export default function IntegrityPage() {
  const [assetId, setAssetId] = useState('btc-5m-yes')
  const [windowMinutes, setWindowMinutes] = useState(5)
  const [request, setRequest] = useState<IntegrityRequest | null>(null)

  const integrityQuery = useIntegritySummary(request)
  const result = integrityQuery.data

  const handleSubmit = useCallback(
    (e: React.FormEvent) => {
      e.preventDefault()
      if (!assetId.trim()) return
      const now = Date.now() * 1000
      setRequest({
        assetId: assetId.trim(),
        startUs: now - windowMinutes * 60_000_000,
        endUs: now,
      })
    },
    [assetId, windowMinutes],
  )

  return (
    <div className="grid gap-[var(--density-gap)]">
      {/* Hero */}
      <div className="flex items-start justify-between rounded-xl border border-card-border bg-card p-6">
        <div>
          <p className="mb-2 text-xs font-medium tracking-widest text-accent uppercase">
            Integrity
          </p>
          <h1 id="page-heading" className="m-0 text-xl font-bold">
            Dataset continuity, gap counts, and validation outcomes.
          </h1>
        </div>
        {result ? (
          <Badge variant={result.completeness === 'complete' ? 'success' : 'warning'}>
            {titleCase(result.completeness)}
          </Badge>
        ) : (
          <Badge variant="neutral">Ready</Badge>
        )}
      </div>

      {integrityQuery.error ? (
        <ErrorBanner
          title="Integrity query failed"
          message={
            integrityQuery.error instanceof Error ? integrityQuery.error.message : 'Unknown error'
          }
        />
      ) : null}

      {/* Query form */}
      <Card>
        <CardHeader title="Integrity Query" />
        <form
          className="grid gap-[var(--density-gap-sm)] md:grid-cols-3 md:items-end"
          onSubmit={handleSubmit}
        >
          <Input label="Asset ID" value={assetId} onChange={(e) => setAssetId(e.target.value)} />
          <Input
            label="Window (minutes)"
            type="number"
            min={1}
            max={1440}
            value={windowMinutes}
            onChange={(e) => setWindowMinutes(Number(e.target.value))}
          />
          <Button type="submit" disabled={integrityQuery.isFetching || !assetId.trim()}>
            {integrityQuery.isFetching ? 'Loading...' : 'Query'}
          </Button>
        </form>
      </Card>

      {result ? (
        <>
          {/* Overview metrics */}
          <div className="grid grid-cols-2 gap-[var(--density-gap-sm)] md:grid-cols-3 lg:grid-cols-6">
            <MetricCard label="Book events" value={String(result.total_book_events)} />
            <MetricCard label="Ingest events" value={String(result.total_ingest_events)} />
            <MetricCard label="Reconnects" value={String(result.reconnect_count)} />
            <MetricCard label="Gaps" value={String(result.gap_count)} />
            <MetricCard label="Stale skips" value={String(result.stale_snapshot_skip_count)} />
            <MetricCard label="Completeness" value={titleCase(result.completeness)} />
          </div>

          {/* Validations */}
          <div className="grid grid-cols-3 gap-[var(--density-gap-sm)]">
            <MetricCard label="Total validations" value={String(result.validation_count)} />
            <MetricCard label="Matched" value={String(result.validations_matched)} />
            <MetricCard label="Mismatched" value={String(result.validations_mismatched)} />
          </div>

          {/* Continuity timeline */}
          {result.continuity_events.length > 0 && (
            <Card>
              <CardHeader title="Continuity Events" />
              <ContinuityTimeline events={result.continuity_events} />
            </Card>
          )}

          {/* Window info */}
          <Card dense>
            <CardHeader title="Window" />
            <p className="text-sm text-muted-foreground">
              {formatTimestamp(result.start_us)} → {formatTimestamp(result.end_us)}
            </p>
          </Card>
        </>
      ) : null}
    </div>
  )
}
