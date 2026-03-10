import { useCallback, useRef, useState } from 'react'
import { useQuerySql } from '../../shared/api/queries'
import { ErrorBanner } from '../../shared/components'
import { QueryResults } from './components/query-results'
import { SchemaBrowser } from './components/schema-browser'
import { SqlEditor } from './components/sql-editor'

export default function QueryPage() {
  const [sql, setSql] = useState(() => sessionStorage.getItem('pb-last-query') ?? '')
  const mutation = useQuerySql()
  const editorRef = useRef<HTMLTextAreaElement>(null)

  const handleRun = useCallback(() => {
    const trimmed = sql.trim()
    if (!trimmed) return
    sessionStorage.setItem('pb-last-query', trimmed)
    mutation.mutate(trimmed)
  }, [sql, mutation])

  const handleColumnInsert = useCallback(
    (columnName: string) => {
      const editor = editorRef.current
      if (!editor) return
      const start = editor.selectionStart
      const end = editor.selectionEnd
      const before = sql.slice(0, start)
      const after = sql.slice(end)
      const newSql = `${before}${columnName}${after}`
      setSql(newSql)
      // Restore cursor position after the inserted text
      requestAnimationFrame(() => {
        editor.focus()
        editor.selectionStart = start + columnName.length
        editor.selectionEnd = start + columnName.length
      })
    },
    [sql],
  )

  return (
    <div className="grid gap-[var(--density-gap)]">
      {/* Hero */}
      <div className="flex items-start justify-between rounded-xl border border-card-border bg-card p-6">
        <div>
          <p className="mb-2 text-xs font-medium tracking-widest text-accent uppercase">
            Query Workbench
          </p>
          <h1 id="page-heading" className="m-0 text-xl font-bold">
            Read-only SQL over split datasets.
          </h1>
        </div>
      </div>

      <div className="grid gap-[var(--density-gap)] lg:grid-cols-[300px_1fr]">
        {/* Schema browser sidebar */}
        <SchemaBrowser onColumnClick={handleColumnInsert} />

        {/* Editor + Results */}
        <div className="grid gap-[var(--density-gap)]">
          <SqlEditor
            ref={editorRef}
            value={sql}
            onChange={setSql}
            onRun={handleRun}
            isRunning={mutation.isPending}
          />

          {mutation.error ? (
            <ErrorBanner
              title="Query failed"
              message={mutation.error instanceof Error ? mutation.error.message : 'Unknown error'}
            />
          ) : null}

          {mutation.data ? <QueryResults result={mutation.data} /> : null}
        </div>
      </div>
    </div>
  )
}
