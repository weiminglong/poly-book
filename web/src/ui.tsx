import { memo } from 'react'
import type { ReactNode } from 'react'
import { formatPrice, formatSize, formatTimestamp, titleCase } from './formatters'
import type { ContinuityWarning, LiveOrderBookSnapshot, ReplayReconstructionResponse } from './types'

export function SectionCard({
  title,
  children,
  dense = false,
  toolbar,
}: {
  title: string
  children: ReactNode
  dense?: boolean
  toolbar?: ReactNode
}) {
  return (
    <section className={dense ? 'card card--dense' : 'card'}>
      <div className="card-header">
        <h3>{title}</h3>
        {toolbar}
      </div>
      {children}
    </section>
  )
}

export function MetricCard({ label, value }: { label: string; value: string }) {
  return (
    <article className="metric-card">
      <span>{label}</span>
      <strong>{value}</strong>
    </article>
  )
}

export function WarningPanel({
  title = titleCase('continuity_warning'),
  warning,
}: {
  title?: string
  warning: ContinuityWarning
}) {
  return (
    <div className="warning-panel">
      <div className="warning-panel__header">
        <strong>{title}</strong>
        <span className="pill pill--warning">{titleCase(warning.kind)}</span>
      </div>
      <p>{warning.details ?? 'No extra details were supplied by the backend.'}</p>
      <p className="muted">
        recv {formatTimestamp(warning.recv_timestamp_us)} · exchange{' '}
        {formatTimestamp(warning.exchange_timestamp_us)}
      </p>
    </div>
  )
}

export function ErrorBanner({
  title,
  message,
  hint,
}: {
  title: string
  message: string
  hint?: string
}) {
  return (
    <div className="error-banner" role="alert">
      <strong>{title}</strong>
      <p>{message}</p>
      {hint ? <p className="muted">{hint}</p> : null}
    </div>
  )
}

type BookLevels =
  | LiveOrderBookSnapshot['bids']
  | LiveOrderBookSnapshot['asks']
  | ReplayReconstructionResponse['bids']
  | ReplayReconstructionResponse['asks']

export const OrderBookTable = memo(function OrderBookTable({
  bids,
  asks,
}: {
  bids: BookLevels
  asks: BookLevels
}) {
  return (
    <div className="book-grid">
      <OrderBookSide levels={bids} side="Bids" />
      <OrderBookSide levels={asks} side="Asks" />
    </div>
  )
})

function OrderBookSide({ levels, side }: { levels: BookLevels; side: 'Bids' | 'Asks' }) {
  return (
    <div>
      <h4>{side}</h4>
      <table>
        <thead>
          <tr>
            <th>Price</th>
            <th>Size</th>
          </tr>
        </thead>
        <tbody>
          {levels.map((level) => (
            <tr key={`${side}-${level.price}-${level.size}`}>
              <td>{formatPrice(level.price)}</td>
              <td>{formatSize(level.size)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
