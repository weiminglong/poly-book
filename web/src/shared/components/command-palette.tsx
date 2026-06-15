import { Command } from 'cmdk'
import { useCallback, useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'

interface CommandPaletteProps {
  onToggleTheme?: () => void
  onSetDensity?: (d: 'compact' | 'comfortable' | 'spacious') => void
  onSetSource?: (m: 'api' | 'demo') => void
}

export function CommandPalette({ onToggleTheme, onSetDensity, onSetSource }: CommandPaletteProps) {
  const [open, setOpen] = useState(false)
  const navigate = useNavigate()

  // Toggle with Cmd+K
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        e.preventDefault()
        setOpen((prev) => !prev)
      }
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [])

  const runAndClose = useCallback((fn: () => void) => {
    fn()
    setOpen(false)
  }, [])

  if (!open) return null

  return (
    <div className="fixed inset-0 z-50">
      {/* biome-ignore lint/a11y/noStaticElementInteractions: backdrop overlay with role="presentation" is intentional */}
      <div
        className="absolute inset-0 bg-black/60 backdrop-blur-sm"
        onClick={() => setOpen(false)}
        role="presentation"
      />

      {/* Command dialog */}
      <div
        className="absolute left-1/2 top-[20%] w-full max-w-lg -translate-x-1/2"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
      >
        <Command
          className="rounded-xl border border-card-border bg-card shadow-lg"
          onKeyDown={(e) => {
            if (e.key === 'Escape') setOpen(false)
          }}
        >
          <Command.Input
            placeholder="Type a command or search..."
            className="w-full border-b border-card-border bg-transparent px-4 py-3 text-foreground placeholder:text-muted-foreground focus:outline-none"
            autoFocus
          />
          <Command.List className="max-h-[300px] overflow-y-auto p-2">
            <Command.Empty className="px-4 py-6 text-center text-sm text-muted-foreground">
              No results found.
            </Command.Empty>

            <Command.Group
              heading="Navigation"
              className="px-2 py-1.5 text-xs font-bold text-muted-foreground"
            >
              <CommandItem onSelect={() => runAndClose(() => navigate('/live-feed'))}>
                Live Feed
              </CommandItem>
              <CommandItem onSelect={() => runAndClose(() => navigate('/orderbook'))}>
                Orderbook
              </CommandItem>
              <CommandItem onSelect={() => runAndClose(() => navigate('/replay'))}>
                Replay Workbench
              </CommandItem>
              <CommandItem onSelect={() => runAndClose(() => navigate('/execution'))}>
                Execution Inspector
              </CommandItem>
              <CommandItem onSelect={() => runAndClose(() => navigate('/integrity'))}>
                Integrity Dashboard
              </CommandItem>
              <CommandItem onSelect={() => runAndClose(() => navigate('/query'))}>
                Query Workbench
              </CommandItem>
            </Command.Group>

            <Command.Separator className="my-1 h-px bg-card-border" />

            <Command.Group
              heading="Settings"
              className="px-2 py-1.5 text-xs font-bold text-muted-foreground"
            >
              {onToggleTheme && (
                <CommandItem onSelect={() => runAndClose(onToggleTheme)}>
                  Toggle theme (dark/light)
                </CommandItem>
              )}
              {onSetDensity && (
                <>
                  <CommandItem onSelect={() => runAndClose(() => onSetDensity('compact'))}>
                    Density: Compact
                  </CommandItem>
                  <CommandItem onSelect={() => runAndClose(() => onSetDensity('comfortable'))}>
                    Density: Comfortable
                  </CommandItem>
                  <CommandItem onSelect={() => runAndClose(() => onSetDensity('spacious'))}>
                    Density: Spacious
                  </CommandItem>
                </>
              )}
              {onSetSource && (
                <>
                  <CommandItem onSelect={() => runAndClose(() => onSetSource('api'))}>
                    Source: Live API
                  </CommandItem>
                  <CommandItem onSelect={() => runAndClose(() => onSetSource('demo'))}>
                    Source: Demo Data
                  </CommandItem>
                </>
              )}
            </Command.Group>
          </Command.List>

          <div className="border-t border-card-border px-4 py-2 text-xs text-muted-foreground">
            ↑↓ navigate · ↵ select · esc close · ⌘K toggle
          </div>
        </Command>
      </div>
    </div>
  )
}

function CommandItem({ children, onSelect }: { children: React.ReactNode; onSelect: () => void }) {
  return (
    <Command.Item
      onSelect={onSelect}
      className="cursor-pointer rounded-lg px-3 py-2 text-sm text-foreground aria-selected:bg-muted"
    >
      {children}
    </Command.Item>
  )
}
