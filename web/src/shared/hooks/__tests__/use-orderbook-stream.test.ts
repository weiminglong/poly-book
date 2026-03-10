import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderHook } from '@testing-library/react'
import { createElement, type ReactNode } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { useOrderBookStream } from '../use-orderbook-stream'

// Mock WebSocket
class MockWebSocket {
  static instances: MockWebSocket[] = []
  onopen: (() => void) | null = null
  onmessage: ((e: { data: string }) => void) | null = null
  onerror: (() => void) | null = null
  onclose: (() => void) | null = null

  constructor(_url: string) {
    MockWebSocket.instances.push(this)
  }

  close() {}
}

vi.stubGlobal('WebSocket', MockWebSocket)

function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return createElement(QueryClientProvider, { client }, children)
}

describe('useOrderBookStream', () => {
  it('returns closed status when assetId is null', () => {
    const { result } = renderHook(() => useOrderBookStream(null), { wrapper })
    expect(result.current.status).toBe('closed')
    expect(result.current.snapshot).toBeNull()
  })

  it('returns connecting status on mount with assetId', () => {
    MockWebSocket.instances = []
    const { result } = renderHook(() => useOrderBookStream('tok1'), { wrapper })
    expect(result.current.status).toBe('connecting')
    expect(MockWebSocket.instances.length).toBe(1)
  })
})
