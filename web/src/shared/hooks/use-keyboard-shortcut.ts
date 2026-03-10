import { useEffect } from 'react'

interface ShortcutOptions {
  key: string
  modifier?: 'meta' | 'ctrl' | 'shift' | 'alt'
  handler: () => void
  enabled?: boolean
}

function isInputElement(target: EventTarget | null): boolean {
  if (!target || !(target instanceof HTMLElement)) return false
  const tag = target.tagName.toLowerCase()
  return tag === 'input' || tag === 'textarea' || tag === 'select' || target.isContentEditable
}

export function useKeyboardShortcut({ key, modifier, handler, enabled = true }: ShortcutOptions) {
  useEffect(() => {
    if (!enabled) return

    function handleKeyDown(e: KeyboardEvent) {
      // Skip if user is typing in an input
      if (!modifier && isInputElement(e.target)) return

      const matchesModifier = modifier
        ? (modifier === 'meta' && e.metaKey) ||
          (modifier === 'ctrl' && e.ctrlKey) ||
          (modifier === 'shift' && e.shiftKey) ||
          (modifier === 'alt' && e.altKey)
        : !e.metaKey && !e.ctrlKey && !e.altKey

      if (matchesModifier && e.key.toLowerCase() === key.toLowerCase()) {
        e.preventDefault()
        handler()
      }
    }

    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [key, modifier, handler, enabled])
}

export function useKeyboardShortcuts(shortcuts: ShortcutOptions[]) {
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      for (const shortcut of shortcuts) {
        if (shortcut.enabled === false) continue

        const hasModifier = shortcut.modifier
        if (!hasModifier && isInputElement(e.target)) continue

        const matchesModifier = hasModifier
          ? (shortcut.modifier === 'meta' && e.metaKey) ||
            (shortcut.modifier === 'ctrl' && e.ctrlKey) ||
            (shortcut.modifier === 'shift' && e.shiftKey) ||
            (shortcut.modifier === 'alt' && e.altKey)
          : !e.metaKey && !e.ctrlKey && !e.altKey

        if (matchesModifier && e.key.toLowerCase() === shortcut.key.toLowerCase()) {
          e.preventDefault()
          shortcut.handler()
          return
        }
      }
    }

    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [shortcuts])
}
