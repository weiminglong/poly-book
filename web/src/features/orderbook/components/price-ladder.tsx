import { memo } from 'react'
import { formatPrice, formatSize } from '../../../shared/lib/formatters'
import type { PriceLevelView } from '../../../types'

interface PriceLadderProps {
  bids: PriceLevelView[]
  asks: PriceLevelView[]
}

export const PriceLadder = memo(function PriceLadder({ bids, asks }: PriceLadderProps) {
  const allSizes = [...bids, ...asks].map((l) => Number.parseFloat(l.size))
  const maxSize = Math.max(...allSizes, 1)

  return (
    <div className="grid grid-cols-2 gap-[var(--density-gap)]">
      {/* Bids */}
      <div>
        <h4 className="mb-2 text-sm font-bold text-muted-foreground">Bids</h4>
        <div className="grid gap-0.5">
          {bids.map((level, i) => {
            const pct = (Number.parseFloat(level.size) / maxSize) * 100
            return (
              <div
                key={`bid-${i}`}
                className="relative flex items-center justify-between rounded px-2 py-1 text-sm"
              >
                <div
                  className="absolute inset-y-0 left-0 rounded bg-bid-bg"
                  style={{ width: `${pct}%` }}
                />
                <span className="relative z-10 font-mono text-success">
                  {formatPrice(level.price)}
                </span>
                <span className="relative z-10 font-mono text-muted-foreground">
                  {formatSize(level.size)}
                </span>
              </div>
            )
          })}
        </div>
      </div>

      {/* Asks */}
      <div>
        <h4 className="mb-2 text-sm font-bold text-muted-foreground">Asks</h4>
        <div className="grid gap-0.5">
          {asks.map((level, i) => {
            const pct = (Number.parseFloat(level.size) / maxSize) * 100
            return (
              <div
                key={`ask-${i}`}
                className="relative flex items-center justify-between rounded px-2 py-1 text-sm"
              >
                <div
                  className="absolute inset-y-0 right-0 rounded bg-ask-bg"
                  style={{ width: `${pct}%` }}
                />
                <span className="relative z-10 font-mono text-destructive">
                  {formatPrice(level.price)}
                </span>
                <span className="relative z-10 font-mono text-muted-foreground">
                  {formatSize(level.size)}
                </span>
              </div>
            )
          })}
        </div>
      </div>
    </div>
  )
})
