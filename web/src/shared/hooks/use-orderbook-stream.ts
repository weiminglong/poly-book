import { useEffect, useRef, useState } from 'react'
import type { BookUpdateMessage } from '../../types'
import { startDemoOrderBookStream } from '../api/demo-stream'
import { bookUpdateMessageSchema } from '../api/schemas'
import { useSourceModeContext } from './use-source-mode'
import { useThrottledState } from './use-throttled-state'

export type StreamStatus =
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'closed'
  | 'fallback'
  | 'demo'

const RECONNECT_BASE_MS = 500
const RECONNECT_MAX_MS = 10_000
const MAX_RETRIES = 8
/** No message within this window marks the stream data stale. */
const STALE_AFTER_MS = 15_000
const STALE_CHECK_INTERVAL_MS = 3_000
/** After exhausting reconnects, retry from scratch this often so the session
 *  recovers instead of being stuck in 'fallback' forever. */
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
  const sourceMode = useSourceModeContext()
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
    // shared-ref approach reset the flag on every run, defeating it.
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

    // Demo mode never opens a socket: the backend is absent by definition, so
    // a real connection attempt would surface as failure noise (a permanent
    // amber "Reconnecting" badge). Feed the page from the client-side market
    // simulator instead, through the same message shape the WS broadcasts.
    if (sourceMode === 'demo') {
      closeCurrent()
      setStatus('demo')
      setError(null)
      setStale(false)
      const stop = startDemoOrderBookStream(assetId, (msg) => {
        if (!cancelled) setSnapshot(msg)
      })
      return () => {
        cancelled = true
        stop()
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
        } catch (err) {
          // Surface malformed frames instead of silently discarding them. A
          // persistently-malformed stream leaves wsSnapshot null and silently
          // degrades the page to HTTP polling under a green badge; the warning
          // gives that failure a diagnostic trail.
          console.warn('Discarding malformed orderbook stream message:', err)
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
          // stuck in fallback indefinitely.
          retries = 0
          reconnectTimer = setTimeout(connect, FALLBACK_RETRY_MS)
          return
        }

        reconnectTimer = setTimeout(connect, delay)
      }
    }

    connect()

    // Mark data stale if no message arrives within the window, so consumers can
    // stop trusting a frozen book under a green "live" badge.
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
  }, [assetId, sourceMode, setSnapshot])

  return { snapshot, status, error, stale }
}
