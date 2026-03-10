import { Dialog, DialogContent, DialogTitle } from './dialog'

interface ShortcutHelpProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  shortcuts: { key: string; description: string }[]
}

export function ShortcutHelp({ open, onOpenChange, shortcuts }: ShortcutHelpProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogTitle className="mb-4 text-lg font-bold text-foreground">
          Keyboard Shortcuts
        </DialogTitle>
        <div className="grid gap-2">
          {shortcuts.map((s) => (
            <div
              key={s.key}
              className="flex items-center justify-between rounded-lg px-3 py-2 hover:bg-muted"
            >
              <span className="text-sm text-foreground">{s.description}</span>
              <kbd className="rounded border border-card-border bg-muted px-2 py-1 font-mono text-xs text-muted-foreground">
                {s.key}
              </kbd>
            </div>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  )
}
