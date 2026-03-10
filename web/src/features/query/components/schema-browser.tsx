import { useState } from 'react'
import { useDatasets } from '../../../shared/api/queries'
import { Card, CardHeader, Skeleton } from '../../../shared/components'

interface SchemaBrowserProps {
  onColumnClick: (columnName: string) => void
}

export function SchemaBrowser({ onColumnClick }: SchemaBrowserProps) {
  const { data, isLoading, error, refetch } = useDatasets()
  const [expanded, setExpanded] = useState<Set<string>>(new Set())

  const toggle = (name: string) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(name)) {
        next.delete(name)
      } else {
        next.add(name)
      }
      return next
    })
  }

  return (
    <Card>
      <CardHeader title="Datasets" />

      {isLoading ? (
        <div className="grid gap-2">
          <Skeleton className="h-8" />
          <Skeleton className="h-8" />
        </div>
      ) : error ? (
        <div className="grid gap-2">
          <p className="text-sm text-destructive">
            {error instanceof Error ? error.message : 'Failed to load datasets'}
          </p>
          <button type="button" onClick={() => refetch()} className="text-sm text-accent underline">
            Retry
          </button>
        </div>
      ) : data?.datasets.length ? (
        <div className="grid gap-1">
          {data.datasets.map((dataset) => (
            <div key={dataset.name}>
              <button
                type="button"
                onClick={() => toggle(dataset.name)}
                className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-sm transition-colors hover:bg-muted"
              >
                <span
                  className={`inline-block transition-transform ${expanded.has(dataset.name) ? 'rotate-90' : ''}`}
                >
                  ▶
                </span>
                <strong className="text-foreground">{dataset.name}</strong>
              </button>
              {expanded.has(dataset.name) && (
                <div className="ml-6 grid gap-0.5">
                  <p className="text-xs text-muted-foreground">{dataset.description}</p>
                  {dataset.columns.map((col) => (
                    <button
                      key={col.name}
                      type="button"
                      onClick={() => onColumnClick(col.name)}
                      className="flex items-center justify-between rounded px-2 py-1 text-xs transition-colors hover:bg-muted"
                    >
                      <span className="font-mono text-foreground">{col.name}</span>
                      <span className="text-muted-foreground">{col.data_type}</span>
                    </button>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">No datasets available.</p>
      )}
    </Card>
  )
}
