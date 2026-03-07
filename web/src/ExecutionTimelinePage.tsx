import { useCallback, useState } from 'react'
import type { DataSourceMode, ExecutionTimelineResponse, WorkstationClient } from './types'
import { ErrorBanner, MetricCard, SectionCard } from './ui'
import { isAbortError } from './client'
import { formatPrice, formatSize, formatTimestamp, titleCase } from './formatters'

interface Props {
  client: WorkstationClient
  sourceMode: DataSourceMode
}

export default function ExecutionTimelinePage({ client, sourceMode }: Props) {
  const [orderId, setOrderId] = useState('')
  const [assetId, setAssetId] = useState('')
  const [windowMinutes, setWindowMinutes] = useState(5)
  const [result, setResult] = useState<ExecutionTimelineResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  const handleSubmit = useCallback(
    async (e: React.FormEvent) => {
      e.preventDefault()
      setError(null)
      setLoading(true)
      try {
        const now = Date.now() * 1000
        const data = await client.getExecutionTimeline({
          orderId: orderId || undefined,
          assetId: assetId || undefined,
          startUs: now - windowMinutes * 60_000_000,
          endUs: now,
          limit: 100,
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
    [client, orderId, assetId, windowMinutes],
  )

  return (
    <>
      <SectionCard title="Execution Timeline">
        <form className="replay-form" onSubmit={handleSubmit}>
          <label>
            Order ID <span className="muted">(optional)</span>
            <input type="text" value={orderId} onChange={(e) => setOrderId(e.target.value)} />
          </label>
          <label>
            Asset ID <span className="muted">(optional)</span>
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
          <button type="submit" disabled={loading}>
            {loading ? 'Loading…' : 'Query'}
          </button>
          {sourceMode === 'demo' && <p className="muted">Using seeded demo data.</p>}
        </form>
      </SectionCard>

      {error && <ErrorBanner title="Execution query failed" message={error} />}

      {result && (
        <>
          <SectionCard title="Results">
            <div className="metric-grid">
              <MetricCard label="Total events" value={String(result.total_count)} />
              <MetricCard label="Shown" value={String(result.events.length)} />
            </div>
          </SectionCard>

          <SectionCard title="Events">
            <div className="table-scroll">
              <table>
                <thead>
                  <tr>
                    <th>Time</th>
                    <th>Order</th>
                    <th>Kind</th>
                    <th>Side</th>
                    <th>Price</th>
                    <th>Size</th>
                    <th>Status</th>
                  </tr>
                </thead>
                <tbody>
                  {result.events.map((ev, i) => (
                    <tr key={`${ev.order_id}-${ev.event_timestamp_us}-${i}`}>
                      <td>{formatTimestamp(ev.event_timestamp_us)}</td>
                      <td className="mono">{ev.order_id}</td>
                      <td>
                        <span className="pill">{titleCase(ev.kind)}</span>
                      </td>
                      <td>{ev.side ?? '—'}</td>
                      <td>{ev.price ? formatPrice(ev.price) : '—'}</td>
                      <td>{ev.size ? formatSize(ev.size) : '—'}</td>
                      <td>{ev.status ?? '—'}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </SectionCard>
        </>
      )}
    </>
  )
}
