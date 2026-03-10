import type {
  ActiveAssetSummary,
  DatasetSchemaResponse,
  ExecutionTimelineResponse,
  FeedStatusResponse,
  IntegritySummaryResponse,
  LiveOrderBookSnapshot,
  QueryResultResponse,
  ReplayFormValues,
  ReplayMode,
  ReplayReconstructionResponse,
} from '../../types'

// Lazy-load fixture data only when demo mode is active
let fixtures: typeof import('./demo-fixtures') | null = null

async function loadFixtures() {
  if (!fixtures) {
    fixtures = await import('./demo-fixtures')
  }
  return fixtures
}

export async function getDemoFeedStatus(): Promise<FeedStatusResponse> {
  const f = await loadFixtures()
  return f.demoFeedStatus
}

export async function getDemoActiveAssets(): Promise<ActiveAssetSummary[]> {
  const f = await loadFixtures()
  return f.demoActiveAssets
}

export async function getDemoSnapshot(
  assetId: string,
  depth: number,
): Promise<LiveOrderBookSnapshot> {
  const f = await loadFixtures()
  return f.getDemoSnapshot(assetId, depth)
}

export async function getDemoReplay(
  assetId: string,
  mode: ReplayMode,
  depth: number,
): Promise<ReplayReconstructionResponse> {
  const f = await loadFixtures()
  return f.getDemoReplay(assetId, mode, depth)
}

export async function getDemoReplayDefaults(): Promise<ReplayFormValues> {
  const f = await loadFixtures()
  return f.demoReplayDefaults
}

export async function getDemoIntegrity(): Promise<IntegritySummaryResponse> {
  const f = await loadFixtures()
  return f.demoIntegritySummary
}

export async function getDemoExecution(): Promise<ExecutionTimelineResponse> {
  const f = await loadFixtures()
  return f.demoExecutionTimeline
}

export async function getDemoDatasets(): Promise<DatasetSchemaResponse> {
  const f = await loadFixtures()
  return f.demoDatasets
}

export async function getDemoQueryResult(): Promise<QueryResultResponse> {
  const f = await loadFixtures()
  return f.demoQueryResult
}
