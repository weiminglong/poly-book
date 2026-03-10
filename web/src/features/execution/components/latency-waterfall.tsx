import { memo } from 'react'
import { formatDurationUs } from '../../../shared/lib/formatters'
import type { LatencyTraceView } from '../../../types'

interface Stage {
  label: string
  startUs: number | null
  endUs: number | null
  color: string
}

function buildStages(latency: LatencyTraceView): Stage[] {
  return [
    {
      label: 'Market Data → Normalization',
      startUs: latency.market_data_recv_us,
      endUs: latency.normalization_done_us,
      color: '#38bdf8', // sky
    },
    {
      label: 'Normalization → Strategy',
      startUs: latency.normalization_done_us,
      endUs: latency.strategy_decision_us,
      color: '#818cf8', // indigo
    },
    {
      label: 'Strategy → Submit',
      startUs: latency.strategy_decision_us,
      endUs: latency.order_submit_us,
      color: '#a78bfa', // violet
    },
    {
      label: 'Submit → Ack',
      startUs: latency.order_submit_us,
      endUs: latency.exchange_ack_us,
      color: '#fbbf24', // amber
    },
    {
      label: 'Ack → Fill',
      startUs: latency.exchange_ack_us,
      endUs: latency.exchange_fill_us,
      color: '#22c55e', // green
    },
  ]
}

export const LatencyWaterfall = memo(function LatencyWaterfall({
  latency,
}: {
  latency: LatencyTraceView
}) {
  const stages = buildStages(latency)

  // Find global min/max for scaling
  const allTimestamps = [
    latency.market_data_recv_us,
    latency.normalization_done_us,
    latency.strategy_decision_us,
    latency.order_submit_us,
    latency.exchange_ack_us,
    latency.exchange_fill_us,
  ].filter((t): t is number => t !== null)

  if (allTimestamps.length < 2) {
    return (
      <p className="text-sm text-muted-foreground">Insufficient latency data for visualization.</p>
    )
  }

  const minTs = Math.min(...allTimestamps)
  const maxTs = Math.max(...allTimestamps)
  const range = maxTs - minTs || 1

  const BAR_HEIGHT = 24
  const GAP = 6
  const LABEL_WIDTH = 200
  const BAR_AREA_WIDTH = 400
  const TOTAL_WIDTH = LABEL_WIDTH + BAR_AREA_WIDTH + 120
  const TOTAL_HEIGHT = stages.length * (BAR_HEIGHT + GAP)

  return (
    <div className="overflow-x-auto">
      <svg
        width={TOTAL_WIDTH}
        height={TOTAL_HEIGHT}
        className="text-sm"
        style={{ minWidth: TOTAL_WIDTH }}
        role="img"
        aria-label="Latency waterfall chart showing execution timing stages"
      >
        {stages.map((stage, i) => {
          const y = i * (BAR_HEIGHT + GAP)
          const hasData = stage.startUs !== null && stage.endUs !== null

          if (!hasData) {
            return (
              <g key={stage.label}>
                <text
                  x={LABEL_WIDTH - 8}
                  y={y + BAR_HEIGHT / 2 + 4}
                  textAnchor="end"
                  className="fill-muted-foreground"
                  fontSize={12}
                >
                  {stage.label}
                </text>
                <line
                  x1={LABEL_WIDTH}
                  y1={y + BAR_HEIGHT / 2}
                  x2={LABEL_WIDTH + BAR_AREA_WIDTH}
                  y2={y + BAR_HEIGHT / 2}
                  stroke="#334155"
                  strokeWidth={2}
                  strokeDasharray="6 4"
                />
                <text
                  x={LABEL_WIDTH + BAR_AREA_WIDTH + 8}
                  y={y + BAR_HEIGHT / 2 + 4}
                  className="fill-muted-foreground"
                  fontSize={11}
                >
                  N/A
                </text>
              </g>
            )
          }

          const start = stage.startUs as number
          const end = stage.endUs as number
          const x = ((start - minTs) / range) * BAR_AREA_WIDTH + LABEL_WIDTH
          const w = Math.max(((end - start) / range) * BAR_AREA_WIDTH, 2)
          const duration = end - start

          return (
            <g key={stage.label}>
              <text
                x={LABEL_WIDTH - 8}
                y={y + BAR_HEIGHT / 2 + 4}
                textAnchor="end"
                className="fill-foreground"
                fontSize={12}
              >
                {stage.label}
              </text>
              <rect
                x={x}
                y={y + 2}
                width={w}
                height={BAR_HEIGHT - 4}
                rx={4}
                fill={stage.color}
                opacity={0.8}
              />
              <text
                x={x + w + 8}
                y={y + BAR_HEIGHT / 2 + 4}
                className="fill-foreground"
                fontSize={11}
                fontFamily="monospace"
              >
                {formatDurationUs(duration)}
              </text>
            </g>
          )
        })}
      </svg>

      {/* Total latency */}
      <p className="mt-2 text-sm text-muted-foreground">
        Total end-to-end:{' '}
        <strong className="text-foreground">{formatDurationUs(maxTs - minTs)}</strong>
      </p>
    </div>
  )
})
