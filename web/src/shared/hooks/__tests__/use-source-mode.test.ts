import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import { useSourceMode } from '../use-source-mode'

describe('useSourceMode', () => {
  beforeEach(() => {
    localStorage.clear()
    // Reset URL search params
    window.history.replaceState({}, '', window.location.pathname)
  })

  it('defaults to api mode', () => {
    const { result } = renderHook(() => useSourceMode())
    expect(result.current.sourceMode).toBe('api')
  })

  it('toggles to demo and persists', () => {
    const { result } = renderHook(() => useSourceMode())
    act(() => result.current.setSourceMode('demo'))
    expect(result.current.sourceMode).toBe('demo')
    expect(localStorage.getItem('pb-workstation-source-mode')).toBe('demo')
  })

  it('reads stored mode on mount', () => {
    localStorage.setItem('pb-workstation-source-mode', 'demo')
    const { result } = renderHook(() => useSourceMode())
    expect(result.current.sourceMode).toBe('demo')
  })

  it('reads mode from URL search param', () => {
    window.history.replaceState({}, '', '?source=demo')
    const { result } = renderHook(() => useSourceMode())
    expect(result.current.sourceMode).toBe('demo')
  })

  it('URL param takes precedence over localStorage', () => {
    localStorage.setItem('pb-workstation-source-mode', 'api')
    window.history.replaceState({}, '', '?source=demo')
    const { result } = renderHook(() => useSourceMode())
    expect(result.current.sourceMode).toBe('demo')
  })
})
