import { useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import type { BookUpdateMessage } from '../../types'
import { queryKeys } from '../api/queries'
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
  const queryClient = useQueryClient()
  const [snapshot, setSnapshot] = useThrottledState<BookUpdateMessage | null>(null)
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
      queueMicrotask(() => {
        if (!unmountedRef.current) setStatus('closed')
      })
      return
    }

    let reconnectTimer: ReturnType<typeof setTimeout> | null = null

    function connect() {
      if (unmountedRef.current) return

      const url = wsUrl(assetId as string)
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
          const raw: unknown = JSON.parse(event.data as string)
          const data = bookUpdateMessageSchema.parse(raw)
          setSnapshot(data)
          // Also update TanStack Query cache for unified data layer
          queryClient.setQueryData(
            queryKeys.orderbook(data.asset_id, data.bids.length),
            (prev: unknown) => {
              if (!prev) return prev
              return {
                ...(prev as Record<string, unknown>),
                sequence: data.sequence,
                last_update_us: data.last_update_us,
                bids: data.bids,
                asks: data.asks,
                mid_price: data.mid_price,
                spread: data.spread,
                best_bid: data.bids[0] ?? null,
                best_ask: data.asks[0] ?? null,
                bid_depth: data.bids.length,
                ask_depth: data.asks.length,
              }
            },
          )
        } catch {
          // Ignore malformed messages
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
        const baseDelay = Math.min(RECONNECT_BASE_MS * 2 ** retriesRef.current, RECONNECT_MAX_MS)
        const jitter = Math.random() * baseDelay * 0.3
        const delay = baseDelay + jitter
        retriesRef.current += 1

        if (retriesRef.current > MAX_RETRIES) {
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
  }, [assetId, close, setSnapshot, queryClient])

  return { snapshot, status, error }
}
