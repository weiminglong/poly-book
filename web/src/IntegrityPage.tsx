import { useCallback, useState } from 'react'
import type { DataSourceMode, IntegritySummaryResponse, WorkstationClient } from './types'
import { ErrorBanner, MetricCard, SectionCard, WarningPanel } from './ui'
import { isAbortError } from './client'
import { formatTimestamp, titleCase } from './formatters'

interface Props {
  client: WorkstationClient
  sourceMode: DataSourceMode
}

export default function IntegrityPage({ client, sourceMode }: Props) {
  const [assetId, setAssetId] = useState('btc-5m-yes')
  const [windowMinutes, setWindowMinutes] = useState(5)
  const [result, setResult] = useState<IntegritySummaryResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  const handleSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault()
      setError(null)
      setLoading(true)
      try {
        const now = Date.now() * 1000
        const data = await client.getIntegritySummary({
          assetId,
          startUs: now - windowMinutes * 60_000_000,
          endUs: now,
        })
        setResult(data)
      } catch (err) {
        if (!isAbortError(err)) {
          setError(err instanceof Error ? err.message : 'Unknown error')
        }
      } finally {
        setLoading(false)
      }
    },
    [client, assetId, windowMinutes],
  )

  return (
    <>
      <SectionCard title="Integrity Summary">
        <form className="replay-form" onSubmit={handleSubmit}>
          <label>
            Asset ID
            <input type="text" value={assetId} onChange={(e) => setAssetId(e.target.value)} />
          </label>
          <label>
            Window (minutes)
            <input
              type="number"
              min={1}
              max={1440}
              value={windowMinutes}
              onChange={(e) => setWindowMinutes(Number(e.target.value))}
            />
          </label>
          <button type="submit" disabled={loading || !assetId}>
            {loading ? 'Loading…' : 'Query'}
          </button>
          {sourceMode === 'demo' && <p className="muted">Using seeded demo data.</p>}
        </form>
      </SectionCard>

      {error && <ErrorBanner title="Integrity query failed" message={error} />}

      {result && (
        <>
          <SectionCard title="Overview">
            <div className="metric-grid">
              <MetricCard label="Book events" value={String(result.total_book_events)} />
              <MetricCard label="Ingest events" value={String(result.total_ingest_events)} />
              <MetricCard label="Reconnects" value={String(result.reconnect_count)} />
              <MetricCard label="Gaps" value={String(result.gap_count)} />
              <MetricCard label="Stale skips" value={String(result.stale_snapshot_skip_count)} />
              <MetricCard label="Completeness" value={titleCase(result.completeness)} />
            </div>
          </SectionCard>

          <SectionCard title="Validations">
            <div className="metric-grid">
              <MetricCard label="Total" value={String(result.validation_count)} />
              <MetricCard label="Matched" value={String(result.validations_matched)} />
              <MetricCard label="Mismatched" value={String(result.validations_mismatched)} />
            </div>
          </SectionCard>

          {result.continuity_events.length > 0 && (
            <SectionCard title="Continuity Events">
              {result.continuity_events.map((w, i) => (
                <WarningPanel key={`${w.kind}-${w.recv_timestamp_us}-${i}`} warning={w} />
              ))}
            </SectionCard>
          )}

          <SectionCard title="Window" dense>
            <p className="muted">
              {formatTimestamp(result.start_us)} → {formatTimestamp(result.end_us)}
            </p>
          </SectionCard>
        </>
      )}
    </>
  )
}
