import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { BookUpdateMessage } from '../../../types'
import { startDemoOrderBookStream } from '../demo-stream'
import { bookUpdateMessageSchema } from '../schemas'

// The simulator seeds asynchronously (lazy fixture import); flush that with
// real microtasks, then drive the tick interval with fake timers.
async function collect(assetId: string, ticks: number): Promise<BookUpdateMessage[]> {
  const messages: BookUpdateMessage[] = []
  const stop = startDemoOrderBookStream(assetId, (m) => messages.push(m))
  // Let the dynamic import + seed message resolve.
  await vi.waitFor(() => {
    if (messages.length === 0) throw new Error('not seeded yet')
  })
  await vi.advanceTimersByTimeAsync(ticks * 140)
  stop()
  return messages
}

describe('startDemoOrderBookStream', () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('emits frames matching the real WS broadcast schema', async () => {
    const messages = await collect('btc-5m-yes', 10)
    expect(messages.length).toBeGreaterThan(5)
    for (const msg of messages) {
      expect(() => bookUpdateMessageSchema.parse(msg)).not.toThrow()
    }
  })

  it('never crosses the book and keeps sides sorted best-first', async () => {
    const messages = await collect('btc-5m-yes', 50)
    for (const msg of messages) {
      const bestBid = Number(msg.bids[0]?.price)
      const bestAsk = Number(msg.asks[0]?.price)
      expect(bestBid).toBeLessThan(bestAsk)
      for (let i = 1; i < msg.bids.length; i++) {
        expect(Number(msg.bids[i]?.price)).toBeLessThan(Number(msg.bids[i - 1]?.price))
      }
      for (let i = 1; i < msg.asks.length; i++) {
        expect(Number(msg.asks[i]?.price)).toBeGreaterThan(Number(msg.asks[i - 1]?.price))
      }
    }
  })

  it('increments sequence monotonically and stops emitting after stop()', async () => {
    const messages = await collect('btc-5m-yes', 10)
    for (let i = 1; i < messages.length; i++) {
      const prev = messages[i - 1]
      const cur = messages[i]
      if (!prev || !cur) continue
      expect(cur.sequence).toBeGreaterThan(prev.sequence)
    }
    const count = messages.length
    await vi.advanceTimersByTimeAsync(1_000)
    expect(messages.length).toBe(count)
  })

  it('synthesizes a book for unknown asset ids instead of dead-ending', async () => {
    const messages = await collect('not-a-real-asset', 5)
    expect(messages.length).toBeGreaterThan(0)
    const first = messages[0]
    expect(first?.bids.length).toBeGreaterThan(0)
    expect(first?.asks.length).toBeGreaterThan(0)
  })
})
