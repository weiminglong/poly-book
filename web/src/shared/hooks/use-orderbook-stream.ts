import { useEffect, useRef, useState } from 'react'
import type { BookUpdateMessage } from '../../types'
import { bookUpdateMessageSchema } from '../api/schemas'
import { useThrottledState } from './use-throttled-state'

export type StreamStatus = 'connecting' | 'connected' | 'reconnecting' | 'closed' | 'fallback'

const RECONNECT_BASE_MS = 500
const RECONNECT_MAX_MS = 10_000
const MAX_RETRIES = 8

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
  const wsRef = useRef<WebSocket | null>(null)

  useEffect(() => {
    // Per-run cancellation flag: each effect run owns its own `cancelled`, so a
    // socket created by a previous run (e.g. before an asset switch, or the
    // first run under React StrictMode) cannot, via its async onclose/onmessage,
    // clobber the current connection or spawn a ghost reconnect loop. The old
    // shared-ref approach reset the flag on every run, defeating it (A.10).
    let cancelled = false
    let retries = 0
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null

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
          return
        }

        reconnectTimer = setTimeout(connect, delay)
      }
    }

    connect()

    return () => {
      cancelled = true
      if (reconnectTimer) clearTimeout(reconnectTimer)
      closeCurrent()
    }
  }, [assetId, setSnapshot])

  return { snapshot, status, error }
}
