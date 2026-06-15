import { useEffect, useRef, useState } from 'react'
import type { BookUpdateMessage } from '../../types'
import { bookUpdateMessageSchema } from '../api/schemas'
import { useThrottledState } from './use-throttled-state'

export type StreamStatus = 'connecting' | 'connected' | 'reconnecting' | 'closed' | 'fallback'

const RECONNECT_BASE_MS = 500
const RECONNECT_MAX_MS = 10_000
const MAX_RETRIES = 8
/** No message within this window marks the stream data stale (A.67). */
const STALE_AFTER_MS = 15_000
const STALE_CHECK_INTERVAL_MS = 3_000
/** After exhausting reconnects, retry from scratch this often so the session
 *  recovers instead of being stuck in 'fallback' forever (A.67). */
const FALLBACK_RETRY_MS = 30_000

function wsUrl(assetId: string): string {
  const loc = window.location
  const proto = loc.protocol === 'https:' ? 'wss:' : 'ws:'
  const apiBase = import.meta.env.VITE_API_BASE_URL
  if (apiBase) {
    const url = new URL(apiBase as string)
    const wsProto = url.protocol === 'https:' ? 'wss:' : 'ws:'
    return `${wsProto}//${url.host}/api/v1/streams/orderbook?asset_id=${encodeURIComponent(assetId)}`
  }
  return `${proto}//${loc.host}/api/v1/streams/orderbook?asset_id=${encodeURIComponent(assetId)}`
}

export function useOrderBookStream(assetId: string | null) {
  const [snapshot, setSnapshot] = useThrottledState<BookUpdateMessage | null>(null)
  const [status, setStatus] = useState<StreamStatus>('closed')
  const [error, setError] = useState<string | null>(null)
  const [stale, setStale] = useState(false)
  const wsRef = useRef<WebSocket | null>(null)
  const lastMessageAtRef = useRef<number>(Date.now())

  useEffect(() => {
    // Per-run cancellation flag: each effect run owns its own `cancelled`, so a
    // socket created by a previous run (e.g. before an asset switch, or the
    // first run under React StrictMode) cannot, via its async onclose/onmessage,
    // clobber the current connection or spawn a ghost reconnect loop. The old
    // shared-ref approach reset the flag on every run, defeating it (A.10).
    let cancelled = false
    let retries = 0
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null

    // Fresh connection: reset the staleness clock so it isn't immediately stale.
    lastMessageAtRef.current = Date.now()
    setStale(false)

    const closeCurrent = () => {
      if (wsRef.current) {
        wsRef.current.close()
        wsRef.current = null
      }
    }

    if (!assetId) {
      closeCurrent()
      setStatus('closed')
      return () => {
        cancelled = true
      }
    }

    function connect() {
      if (cancelled) return

      const url = wsUrl(assetId as string)
      setStatus('connecting')
      setError(null)

      const ws = new WebSocket(url)
      wsRef.current = ws

      // `wsRef.current !== ws` rejects callbacks from a socket that is no longer
      // the active one, in addition to the per-run `cancelled` guard.
      const isStale = () => cancelled || wsRef.current !== ws

      ws.onopen = () => {
        if (isStale()) return
        retries = 0
        setStatus('connected')
      }

      ws.onmessage = (event) => {
        if (isStale()) return
        try {
          const raw: unknown = JSON.parse(event.data as string)
          const data = bookUpdateMessageSchema.parse(raw)
          lastMessageAtRef.current = Date.now()
          setStale(false)
          setSnapshot(data)
        } catch {
          // Ignore malformed messages.
        }
      }

      ws.onerror = () => {
        if (isStale()) return
        setError('WebSocket error')
      }

      ws.onclose = () => {
        if (isStale()) return
        wsRef.current = null
        setStatus('reconnecting')
        const baseDelay = Math.min(RECONNECT_BASE_MS * 2 ** retries, RECONNECT_MAX_MS)
        const jitter = Math.random() * baseDelay * 0.3
        const delay = baseDelay + jitter
        retries += 1

        if (retries > MAX_RETRIES) {
          setStatus('fallback')
          setError('WebSocket unavailable, falling back to HTTP polling')
          // Keep trying from scratch so the stream recovers rather than being
          // stuck in fallback indefinitely (A.67).
          retries = 0
          reconnectTimer = setTimeout(connect, FALLBACK_RETRY_MS)
          return
        }

        reconnectTimer = setTimeout(connect, delay)
      }
    }

    connect()

    // Mark data stale if no message arrives within the window, so consumers can
    // stop trusting a frozen book under a green "live" badge (A.67).
    const staleTimer = setInterval(() => {
      if (cancelled) return
      if (Date.now() - lastMessageAtRef.current > STALE_AFTER_MS) {
        setStale(true)
      }
    }, STALE_CHECK_INTERVAL_MS)

    return () => {
      cancelled = true
      if (reconnectTimer) clearTimeout(reconnectTimer)
      clearInterval(staleTimer)
      closeCurrent()
    }
  }, [assetId, setSnapshot])

  return { snapshot, status, error, stale }
}
