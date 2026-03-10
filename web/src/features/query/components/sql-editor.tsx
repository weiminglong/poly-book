import { type ChangeEvent, forwardRef, type KeyboardEvent, useCallback } from 'react'
import { Button, Card, CardHeader } from '../../../shared/components'

interface SqlEditorProps {
  value: string
  onChange: (value: string) => void
  onRun: () => void
  isRunning: boolean
}

export const SqlEditor = forwardRef<HTMLTextAreaElement, SqlEditorProps>(
  ({ value, onChange, onRun, isRunning }, ref) => {
    const handleChange = useCallback(
      (e: ChangeEvent<HTMLTextAreaElement>) => onChange(e.target.value),
      [onChange],
    )

    const handleKeyDown = useCallback(
      (e: KeyboardEvent<HTMLTextAreaElement>) => {
        if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
          e.preventDefault()
          onRun()
        }
      },
      [onRun],
    )

    const isEmpty = !value.trim()

    return (
      <Card>
        <CardHeader title="SQL Editor">
          <span className="text-xs text-muted-foreground">⌘+Enter to run</span>
        </CardHeader>
        <textarea
          ref={ref}
          value={value}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder="SELECT asset_id, count(*) FROM book_events GROUP BY asset_id"
          rows={6}
          className={`w-full resize-y rounded-md border bg-background p-3 font-mono text-sm text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring ${
            isEmpty && isRunning ? 'border-warning' : 'border-input-border'
          }`}
          spellCheck={false}
        />
        <div className="mt-3 flex items-center gap-3">
          <Button onClick={onRun} disabled={isRunning || isEmpty}>
            {isRunning ? 'Running...' : 'Run Query'}
          </Button>
          {isEmpty && (
            <span className="text-xs text-muted-foreground">Enter a SQL query to run</span>
          )}
        </div>
      </Card>
    )
  },
)

SqlEditor.displayName = 'SqlEditor'
