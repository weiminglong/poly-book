import { useEffect, useState } from 'react'

interface AdaptivePollingOptions {
  enabled?: boolean
  visibleIntervalMs: number
  hiddenIntervalMs: number
  runTask: (signal: AbortSignal) => Promise<void>
}

interface AdaptivePollingState {
  currentIntervalMs: number
  isVisible: boolean
  lastCompletedAtMs: number | null
  runCount: number
}

export function useAdaptivePolling({
  enabled = true,
  visibleIntervalMs,
  hiddenIntervalMs,
  runTask,
}: AdaptivePollingOptions): AdaptivePollingState {
  const [isVisible, setIsVisible] = useState<boolean>(() => !document.hidden)
  const [lastCompletedAtMs, setLastCompletedAtMs] = useState<number | null>(null)
  const [runCount, setRunCount] = useState(0)

  useEffect(() => {
    const handleVisibilityChange = () => {
      setIsVisible(!document.hidden)
    }

    document.addEventListener('visibilitychange', handleVisibilityChange)

    return () => {
      document.removeEventListener('visibilitychange', handleVisibilityChange)
    }
  }, [])

  useEffect(() => {
    if (!enabled) {
      return
    }

    let cancelled = false
    let timeoutId: number | null = null
    let activeController: AbortController | null = null

    const execute = async () => {
      activeController?.abort()
      activeController = new AbortController()

      try {
        await runTask(activeController.signal)
        if (!cancelled) {
          setLastCompletedAtMs(Date.now())
          setRunCount((previous) => previous + 1)
        }
      } catch {
        // The caller owns request error state; the poller just manages cadence/cancellation.
      } finally {
        if (!cancelled) {
          timeoutId = window.setTimeout(
            execute,
            isVisible ? visibleIntervalMs : hiddenIntervalMs,
          )
        }
      }
    }

    void execute()

    return () => {
      cancelled = true
      activeController?.abort()
      if (timeoutId !== null) {
        window.clearTimeout(timeoutId)
      }
    }
  }, [enabled, hiddenIntervalMs, isVisible, runTask, visibleIntervalMs])

  return {
    currentIntervalMs: isVisible ? visibleIntervalMs : hiddenIntervalMs,
    isVisible,
    lastCompletedAtMs,
    runCount,
  }
}
