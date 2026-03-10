import { describe, expect, it } from 'vitest'
import {
  demoActiveAssets,
  demoDatasets,
  demoExecutionTimeline,
  demoFeedStatus,
  demoIntegritySummary,
  demoQueryResult,
  getDemoReplay,
  getDemoSnapshot,
} from '../../api/demo-fixtures'
import {
  activeAssetsResponseSchema,
  datasetSchemaResponseSchema,
  executionTimelineResponseSchema,
  feedStatusResponseSchema,
  integritySummaryResponseSchema,
  liveOrderBookSnapshotSchema,
  queryResultResponseSchema,
  replayReconstructionResponseSchema,
} from '../schemas'

describe('Zod schema round-trip validation against demo fixtures', () => {
  it('demoFeedStatus passes feedStatusResponseSchema', () => {
    const result = feedStatusResponseSchema.safeParse(demoFeedStatus)
    expect(result.success).toBe(true)
  })

  it('demoActiveAssets passes activeAssetsResponseSchema', () => {
    const result = activeAssetsResponseSchema.safeParse(demoActiveAssets)
    expect(result.success).toBe(true)
  })

  it('demoOrderbook snapshot passes liveOrderBookSnapshotSchema', () => {
    const snapshot = getDemoSnapshot('btc-5m-yes', 6)
    const result = liveOrderBookSnapshotSchema.safeParse(snapshot)
    expect(result.success).toBe(true)
  })

  it('demoReplayResult passes replayReconstructionResponseSchema', () => {
    const replay = getDemoReplay('btc-5m-yes', 'recv_time', 5)
    const result = replayReconstructionResponseSchema.safeParse(replay)
    expect(result.success).toBe(true)
  })

  it('demoIntegritySummary passes integritySummaryResponseSchema', () => {
    const result = integritySummaryResponseSchema.safeParse(demoIntegritySummary)
    expect(result.success).toBe(true)
  })

  it('demoExecutionTimeline passes executionTimelineResponseSchema', () => {
    const result = executionTimelineResponseSchema.safeParse(demoExecutionTimeline)
    expect(result.success).toBe(true)
  })

  it('demoDatasets passes datasetSchemaResponseSchema', () => {
    const result = datasetSchemaResponseSchema.safeParse(demoDatasets)
    expect(result.success).toBe(true)
  })

  it('demoQueryResult passes queryResultResponseSchema', () => {
    const result = queryResultResponseSchema.safeParse(demoQueryResult)
    expect(result.success).toBe(true)
  })
})
