import { MetricCard } from '../../../shared/components'
import { formatLevel, formatNumber, formatTimestamp } from '../../../shared/lib/formatters'
import type { LiveOrderBookSnapshot } from '../../../types'

export function OrderbookMetricGrid({ snapshot }: { snapshot: LiveOrderBookSnapshot }) {
  return (
    <div className="grid grid-cols-2 gap-[var(--density-gap-sm)] md:grid-cols-4 lg:grid-cols-8">
      <MetricCard label="Best bid" value={formatLevel(snapshot.best_bid)} />
      <MetricCard label="Best ask" value={formatLevel(snapshot.best_ask)} />
      <MetricCard label="Mid" value={formatNumber(snapshot.mid_price)} />
      <MetricCard label="Spread" value={formatNumber(snapshot.spread)} />
      <MetricCard label="Bid depth" value={String(snapshot.bid_depth)} />
      <MetricCard label="Ask depth" value={String(snapshot.ask_depth)} />
      <MetricCard label="Sequence" value={String(snapshot.sequence)} />
      <MetricCard label="Updated" value={formatTimestamp(snapshot.last_update_us)} />
    </div>
  )
}
