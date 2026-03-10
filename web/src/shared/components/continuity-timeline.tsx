import type { ContinuityWarning } from '../../types'
import { formatTimestamp, titleCase } from '../lib/formatters'
import { Tooltip } from './tooltip'

const kindColors: Record<string, string> = {
  reconnect_start: 'bg-warning',
  reconnect_success: 'bg-success',
  reconnect_boundary: 'bg-warning',
  sequence_gap: 'bg-destructive',
  sequence_gap_detected: 'bg-destructive',
  checkpoint_loaded: 'bg-accent',
}

export function ContinuityTimeline({ events }: { events: ContinuityWarning[] }) {
  if (events.length === 0) {
    return <p className="text-sm text-muted-foreground">No continuity events in this window.</p>
  }

  const timestamps = events.map((e) => e.recv_timestamp_us)
  const minTs = Math.min(...timestamps)
  const maxTs = Math.max(...timestamps)
  const range = maxTs - minTs || 1

  return (
    <div className="relative h-12 w-full rounded-lg bg-muted">
      {/* Axis labels */}
      <span className="absolute bottom-0 left-1 text-[10px] text-muted-foreground">
        {formatTimestamp(minTs)}
      </span>
      <span className="absolute bottom-0 right-1 text-[10px] text-muted-foreground">
        {formatTimestamp(maxTs)}
      </span>

      {/* Event markers */}
      {events.map((event, i) => {
        const pct = ((event.recv_timestamp_us - minTs) / range) * 100
        const colorClass = kindColors[event.kind] ?? 'bg-muted-foreground'
        return (
          <Tooltip
            key={`${event.kind}-${event.recv_timestamp_us}-${i}`}
            content={
              <div className="grid gap-1 text-xs">
                <strong>{titleCase(event.kind)}</strong>
                <p className="m-0">{event.details ?? 'No details'}</p>
                <p className="m-0 text-muted-foreground">
                  {formatTimestamp(event.recv_timestamp_us)}
                </p>
              </div>
            }
          >
            <button
              type="button"
              className={`absolute top-2 h-4 w-4 -translate-x-1/2 rounded-full border-2 border-background ${colorClass} cursor-pointer transition-transform hover:scale-125`}
              style={{ left: `${Math.max(4, Math.min(96, pct))}%` }}
              aria-label={`${titleCase(event.kind)} at ${formatTimestamp(event.recv_timestamp_us)}`}
            />
          </Tooltip>
        )
      })}
    </div>
  )
}
