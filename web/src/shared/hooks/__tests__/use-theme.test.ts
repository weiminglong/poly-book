import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'
import { useTheme } from '../use-theme'

describe('useTheme', () => {
  beforeEach(() => {
    localStorage.clear()
    document.documentElement.classList.remove('theme-light', 'density-compact', 'density-spacious')
  })

  it('defaults to dark theme and comfortable density', () => {
    const { result } = renderHook(() => useTheme())
    expect(result.current.theme).toBe('dark')
    expect(result.current.density).toBe('comfortable')
  })

  it('toggles between dark and light', () => {
    const { result } = renderHook(() => useTheme())
    act(() => result.current.toggleTheme())
    expect(result.current.theme).toBe('light')
    expect(document.documentElement.classList.contains('theme-light')).toBe(true)

    act(() => result.current.toggleTheme())
    expect(result.current.theme).toBe('dark')
    expect(document.documentElement.classList.contains('theme-light')).toBe(false)
  })

  it('persists theme to localStorage', () => {
    const { result } = renderHook(() => useTheme())
    act(() => result.current.setTheme('light'))
    expect(localStorage.getItem('pb-theme')).toBe('light')
  })

  it('reads stored theme on mount', () => {
    localStorage.setItem('pb-theme', 'light')
    const { result } = renderHook(() => useTheme())
    expect(result.current.theme).toBe('light')
  })

  it('sets density and persists', () => {
    const { result } = renderHook(() => useTheme())
    act(() => result.current.setDensity('compact'))
    expect(result.current.density).toBe('compact')
    expect(localStorage.getItem('pb-density')).toBe('compact')
    expect(document.documentElement.classList.contains('density-compact')).toBe(true)
  })
})
