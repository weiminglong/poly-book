import type { ColumnDef } from '@tanstack/react-table'
import { useMemo } from 'react'
import { Badge, Card, CardHeader, DataTable } from '../../../shared/components'
import type { QueryResultResponse } from '../../../types'

interface QueryResultsProps {
  result: QueryResultResponse
}

export function QueryResults({ result }: QueryResultsProps) {
  // Convert rows to objects for DataTable
  const columns: ColumnDef<Record<string, unknown>, unknown>[] = useMemo(
    () =>
      result.columns.map((col) => ({
        id: col.name,
        accessorFn: (row: Record<string, unknown>) => row[col.name],
        header: () => (
          <span>
            {col.name}
            <span className="ml-1 text-xs text-muted-foreground">({col.data_type})</span>
          </span>
        ),
        cell: ({ getValue }) => {
          const val = getValue()
          if (val === null || val === undefined)
            return <span className="text-muted-foreground">NULL</span>
          return <span className="font-mono">{String(val)}</span>
        },
      })),
    [result.columns],
  )

  const data = useMemo(
    () =>
      result.rows.map((row) => {
        const obj: Record<string, unknown> = {}
        result.columns.forEach((col, i) => {
          obj[col.name] = row[i]
        })
        return obj
      }),
    [result.rows, result.columns],
  )

  return (
    <Card>
      <CardHeader title="Results">
        <div className="flex items-center gap-3">
          <span className="text-sm text-muted-foreground">
            {result.row_count} row{result.row_count !== 1 ? 's' : ''} in {result.execution_time_ms}
            ms
          </span>
          {result.truncated && <Badge variant="warning">Truncated</Badge>}
        </div>
      </CardHeader>

      {result.truncated && (
        <div className="mb-3 rounded-lg border border-warning/25 bg-warning/8 px-3 py-2 text-sm text-warning">
          Results truncated. The query returned more rows than the limit.
        </div>
      )}

      <DataTable columns={columns} data={data} pageSize={50} />
    </Card>
  )
}
