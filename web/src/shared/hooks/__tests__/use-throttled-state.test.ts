import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useThrottledState } from '../use-throttled-state'

describe('useThrottledState', () => {
  beforeEach(() => {
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      cb(0)
      return 0
    })
    vi.spyOn(window, 'cancelAnimationFrame').mockImplementation(() => {})
  })

  it('returns initial value', () => {
    const { result } = renderHook(() => useThrottledState('hello'))
    expect(result.current[0]).toBe('hello')
  })

  it('updates value after animation frame', () => {
    let rafCallback: FrameRequestCallback | null = null
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      rafCallback = cb
      return 1
    })

    const { result } = renderHook(() => useThrottledState(0))

    act(() => result.current[1](42))

    // Value not yet applied (waiting for raf)
    expect(result.current[0]).toBe(0)

    // Flush the raf
    act(() => {
      if (rafCallback) rafCallback(0)
    })

    expect(result.current[0]).toBe(42)
  })

  it('coalesces rapid updates to last value', () => {
    let rafCallback: FrameRequestCallback | null = null
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      rafCallback = cb
      return 1
    })

    const { result } = renderHook(() => useThrottledState(0))

    act(() => {
      result.current[1](1)
      result.current[1](2)
      result.current[1](3)
    })

    act(() => {
      if (rafCallback) rafCallback(0)
    })

    expect(result.current[0]).toBe(3)
  })
})
