import { useCallback, useRef, useState } from 'react'

/**
 * Like useState but coalesces rapid updates so React re-renders at most once
 * per animation frame. Useful for high-frequency WebSocket messages.
 */
export function useThrottledState<T>(initial: T): [T, (value: T) => void] {
  const [state, setState] = useState<T>(initial)
  const pendingRef = useRef<T | null>(null)
  const rafRef = useRef<number | null>(null)

  const set = useCallback((value: T) => {
    pendingRef.current = value
    if (rafRef.current === null) {
      rafRef.current = requestAnimationFrame(() => {
        rafRef.current = null
        if (pendingRef.current !== null) {
          setState(pendingRef.current)
          pendingRef.current = null
        }
      })
    }
  }, [])

  return [state, set]
}
