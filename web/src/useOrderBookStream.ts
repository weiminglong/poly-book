import { useCallback, useEffect, useRef, useState } from 'react'

export interface BookUpdateMessage {
  asset_id: string
  sequence: number
  last_update_us: number
  bids: { price: string; size: string }[]
  asks: { price: string; size: string }[]
  mid_price: number | null
  spread: number | null
}

export type StreamStatus = 'connecting' | 'connected' | 'reconnecting' | 'closed' | 'fallback'

const RECONNECT_BASE_MS = 500
const RECONNECT_MAX_MS = 10_000

function wsUrl(assetId: string): string {
  const loc = window.location
  const proto = loc.protocol === 'https:' ? 'wss:' : 'ws:'
  const apiBase = import.meta.env.VITE_API_BASE_URL
  if (apiBase) {
    const url = new URL(apiBase)
    const wsProto = url.protocol === 'https:' ? 'wss:' : 'ws:'
    return `${wsProto}//${url.host}/api/v1/streams/orderbook?asset_id=${encodeURIComponent(assetId)}`
  }
  return `${proto}//${loc.host}/api/v1/streams/orderbook?asset_id=${encodeURIComponent(assetId)}`
}

export function useOrderBookStream(assetId: string | null) {
  const [snapshot, setSnapshot] = useState<BookUpdateMessage | null>(null)
  const [status, setStatus] = useState<StreamStatus>('closed')
  const [error, setError] = useState<string | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const retriesRef = useRef(0)
  const unmountedRef = useRef(false)

  const close = useCallback(() => {
    if (wsRef.current) {
      wsRef.current.close()
      wsRef.current = null
    }
  }, [])

  useEffect(() => {
    unmountedRef.current = false

    if (!assetId) {
      close()
      // Defer state update to avoid synchronous setState in effect body.
      queueMicrotask(() => {
        if (!unmountedRef.current) setStatus('closed')
      })
      return
    }

    let reconnectTimer: ReturnType<typeof setTimeout> | null = null

    function connect() {
      if (unmountedRef.current) return

      const url = wsUrl(assetId!)
      setStatus('connecting')
      setError(null)

      const ws = new WebSocket(url)
      wsRef.current = ws

      ws.onopen = () => {
        if (unmountedRef.current) return
        retriesRef.current = 0
        setStatus('connected')
      }

      ws.onmessage = (event) => {
        if (unmountedRef.current) return
        try {
          const data = JSON.parse(event.data as string) as BookUpdateMessage
          setSnapshot(data)
        } catch {
          // Ignore malformed messages.
        }
      }

      ws.onerror = () => {
        if (unmountedRef.current) return
        setError('WebSocket error')
      }

      ws.onclose = () => {
        if (unmountedRef.current) return
        wsRef.current = null
        setStatus('reconnecting')
        const delay = Math.min(
          RECONNECT_BASE_MS * Math.pow(2, retriesRef.current),
          RECONNECT_MAX_MS,
        )
        retriesRef.current += 1

        if (retriesRef.current > 8) {
          setStatus('fallback')
          setError('WebSocket unavailable, falling back to HTTP polling')
          return
        }

        reconnectTimer = setTimeout(connect, delay)
      }
    }

    connect()

    return () => {
      unmountedRef.current = true
      if (reconnectTimer) clearTimeout(reconnectTimer)
      close()
    }
  }, [assetId, close])

  return { snapshot, status, error }
}
