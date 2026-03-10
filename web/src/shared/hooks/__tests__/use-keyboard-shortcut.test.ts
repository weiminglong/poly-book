import { renderHook } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { useKeyboardShortcut } from '../use-keyboard-shortcut'

function fireKey(key: string, options: Partial<KeyboardEventInit> = {}) {
  document.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, ...options }))
}

describe('useKeyboardShortcut', () => {
  it('fires callback on matching key press', () => {
    const handler = vi.fn()
    renderHook(() => useKeyboardShortcut({ key: 'k', handler }))

    fireKey('k')
    expect(handler).toHaveBeenCalledOnce()
  })

  it('fires callback with modifier key', () => {
    const handler = vi.fn()
    renderHook(() => useKeyboardShortcut({ key: 'k', modifier: 'meta', handler }))

    fireKey('k') // no modifier
    expect(handler).not.toHaveBeenCalled()

    fireKey('k', { metaKey: true })
    expect(handler).toHaveBeenCalledOnce()
  })

  it('does not fire when input element is focused (no modifier)', () => {
    const handler = vi.fn()
    renderHook(() => useKeyboardShortcut({ key: 'k', handler }))

    const input = document.createElement('input')
    document.body.appendChild(input)
    input.focus()

    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', bubbles: true }))
    expect(handler).not.toHaveBeenCalled()

    document.body.removeChild(input)
  })

  it('does not fire when enabled is false', () => {
    const handler = vi.fn()
    renderHook(() => useKeyboardShortcut({ key: 'k', handler, enabled: false }))

    fireKey('k')
    expect(handler).not.toHaveBeenCalled()
  })

  it('is case-insensitive', () => {
    const handler = vi.fn()
    renderHook(() => useKeyboardShortcut({ key: 'K', handler }))

    fireKey('k')
    expect(handler).toHaveBeenCalledOnce()
  })
})
