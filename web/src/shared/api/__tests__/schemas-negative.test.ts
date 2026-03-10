import { describe, expect, it } from 'vitest'
import {
  activeAssetsResponseSchema,
  feedStatusResponseSchema,
  liveOrderBookSnapshotSchema,
  queryResultResponseSchema,
  replayReconstructionResponseSchema,
} from '../schemas'

describe('Zod schema rejection of malformed input', () => {
  it('feedStatusResponseSchema rejects missing mode', () => {
    const result = feedStatusResponseSchema.safeParse({
      session_status: 'connected',
      current_session_id: null,
      active_asset_count: 0,
      active_assets: [],
      last_rotation_us: null,
      latest_global_warning: null,
    })
    expect(result.success).toBe(false)
  })

  it('feedStatusResponseSchema rejects invalid mode value', () => {
    const result = feedStatusResponseSchema.safeParse({
      mode: 'invalid_mode',
      session_status: 'connected',
      current_session_id: null,
      active_asset_count: 0,
      active_assets: [],
      last_rotation_us: null,
      latest_global_warning: null,
    })
    expect(result.success).toBe(false)
  })

  it('activeAssetsResponseSchema rejects non-array input', () => {
    const result = activeAssetsResponseSchema.safeParse({ assets: [] })
    expect(result.success).toBe(false)
  })

  it('activeAssetsResponseSchema rejects array with missing fields', () => {
    const result = activeAssetsResponseSchema.safeParse([{ asset_id: 'test' }])
    expect(result.success).toBe(false)
  })

  it('liveOrderBookSnapshotSchema rejects missing bids array', () => {
    const result = liveOrderBookSnapshotSchema.safeParse({
      asset_id: 'test',
      sequence: 1,
      last_update_us: 1000,
      best_bid: null,
      best_ask: null,
      mid_price: null,
      spread: null,
      bid_depth: 0,
      ask_depth: 0,
      asks: [],
      stale: false,
      latest_warning: null,
    })
    expect(result.success).toBe(false)
  })

  it('liveOrderBookSnapshotSchema rejects price level with numeric price (expects string)', () => {
    const result = liveOrderBookSnapshotSchema.safeParse({
      asset_id: 'test',
      sequence: 1,
      last_update_us: 1000,
      best_bid: { price: 0.53, size: '100' },
      best_ask: null,
      mid_price: null,
      spread: null,
      bid_depth: 0,
      ask_depth: 0,
      bids: [],
      asks: [],
      stale: false,
      latest_warning: null,
    })
    expect(result.success).toBe(false)
  })

  it('replayReconstructionResponseSchema rejects invalid mode enum', () => {
    const result = replayReconstructionResponseSchema.safeParse({
      asset_id: 'test',
      mode: 'wall_clock',
      used_checkpoint: false,
      sequence: 1,
      last_update_us: 1000,
      best_bid: null,
      best_ask: null,
      mid_price: null,
      spread: null,
      bid_depth: 0,
      ask_depth: 0,
      bids: [],
      asks: [],
      continuity_events: [],
    })
    expect(result.success).toBe(false)
  })

  it('queryResultResponseSchema rejects missing row_count', () => {
    const result = queryResultResponseSchema.safeParse({
      columns: [],
      rows: [],
      truncated: false,
      execution_time_ms: 10,
    })
    expect(result.success).toBe(false)
  })

  it('feedStatusResponseSchema rejects null input', () => {
    const result = feedStatusResponseSchema.safeParse(null)
    expect(result.success).toBe(false)
  })

  it('feedStatusResponseSchema rejects string input', () => {
    const result = feedStatusResponseSchema.safeParse('not an object')
    expect(result.success).toBe(false)
  })
})
