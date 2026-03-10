import { type UseQueryOptions, useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import type {
  ActiveAssetSummary,
  DatasetSchemaResponse,
  ExecutionRequest,
  ExecutionTimelineResponse,
  FeedStatusResponse,
  IntegrityRequest,
  IntegritySummaryResponse,
  LiveOrderBookSnapshot,
  ReplayReconstructionResponse,
  ReplayRequest,
} from '../../types'
import { buildUrl, fetchAndValidate, getApiBaseUrl, postAndValidate } from './client'
import {
  activeAssetsResponseSchema,
  datasetSchemaResponseSchema,
  executionTimelineResponseSchema,
  feedStatusResponseSchema,
  integritySummaryResponseSchema,
  liveOrderBookSnapshotSchema,
  queryResultResponseSchema,
  replayReconstructionResponseSchema,
} from './schemas'

// --- Constants ---

const FOREGROUND_INTERVAL_MS = 1_000
const BACKGROUND_INTERVAL_MS = 5_000

// --- Query keys ---

export const queryKeys = {
  feedStatus: ['feed-status'] as const,
  activeAssets: ['active-assets'] as const,
  orderbook: (assetId: string, depth: number) => ['orderbook', assetId, depth] as const,
  replay: (req: ReplayRequest) => ['replay', req.assetId, req.atUs, req.mode, req.depth] as const,
  integrity: (req: IntegrityRequest) => ['integrity', req.assetId, req.startUs, req.endUs] as const,
  execution: (req: ExecutionRequest) =>
    ['execution', req.orderId, req.assetId, req.startUs, req.endUs, req.limit] as const,
  datasets: ['datasets'] as const,
}

// --- Query hooks ---

export function useFeedStatus(opts?: Partial<UseQueryOptions<FeedStatusResponse>>) {
  const base = getApiBaseUrl()
  return useQuery({
    queryKey: queryKeys.feedStatus,
    queryFn: ({ signal }) =>
      fetchAndValidate(feedStatusResponseSchema, buildUrl(base, '/api/v1/feed/status'), { signal }),
    refetchInterval: FOREGROUND_INTERVAL_MS,
    staleTime: FOREGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useActiveAssets(opts?: Partial<UseQueryOptions<ActiveAssetSummary[]>>) {
  const base = getApiBaseUrl()
  return useQuery({
    queryKey: queryKeys.activeAssets,
    queryFn: ({ signal }) =>
      fetchAndValidate(activeAssetsResponseSchema, buildUrl(base, '/api/v1/assets/active'), {
        signal,
      }),
    refetchInterval: FOREGROUND_INTERVAL_MS,
    staleTime: FOREGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useOrderBookSnapshot(
  assetId: string,
  depth: number,
  opts?: Partial<UseQueryOptions<LiveOrderBookSnapshot>>,
) {
  const base = getApiBaseUrl()
  return useQuery({
    queryKey: queryKeys.orderbook(assetId, depth),
    queryFn: ({ signal }) =>
      fetchAndValidate(
        liveOrderBookSnapshotSchema,
        buildUrl(base, `/api/v1/orderbooks/${encodeURIComponent(assetId)}/snapshot`, {
          depth: String(depth),
        }),
        { signal },
      ),
    enabled: Boolean(assetId),
    refetchInterval: FOREGROUND_INTERVAL_MS,
    staleTime: FOREGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useReplayReconstruction(
  request: ReplayRequest | null,
  opts?: Partial<UseQueryOptions<ReplayReconstructionResponse>>,
) {
  const base = getApiBaseUrl()
  return useQuery({
    queryKey: request ? queryKeys.replay(request) : ['replay-disabled'],
    queryFn: ({ signal }) => {
      if (!request) throw new Error('No replay request')
      return fetchAndValidate(
        replayReconstructionResponseSchema,
        buildUrl(base, '/api/v1/replay/reconstruct', {
          asset_id: request.assetId,
          at_us: String(request.atUs),
          mode: request.mode,
          source: 'parquet',
          depth: String(request.depth),
        }),
        { signal },
      )
    },
    enabled: Boolean(request),
    staleTime: Number.POSITIVE_INFINITY,
    ...opts,
  })
}

export function useIntegritySummary(
  request: IntegrityRequest | null,
  opts?: Partial<UseQueryOptions<IntegritySummaryResponse>>,
) {
  const base = getApiBaseUrl()
  return useQuery({
    queryKey: request ? queryKeys.integrity(request) : ['integrity-disabled'],
    queryFn: ({ signal }) => {
      if (!request) throw new Error('No integrity request')
      return fetchAndValidate(
        integritySummaryResponseSchema,
        buildUrl(base, '/api/v1/integrity/summary', {
          asset_id: request.assetId,
          start_us: String(request.startUs),
          end_us: String(request.endUs),
        }),
        { signal },
      )
    },
    enabled: Boolean(request),
    staleTime: BACKGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useExecutionTimeline(
  request: ExecutionRequest | null,
  opts?: Partial<UseQueryOptions<ExecutionTimelineResponse>>,
) {
  const base = getApiBaseUrl()
  return useQuery({
    queryKey: request ? queryKeys.execution(request) : ['execution-disabled'],
    queryFn: ({ signal }) => {
      if (!request) throw new Error('No execution request')
      const params: Record<string, string> = {
        start_us: String(request.startUs),
        end_us: String(request.endUs),
      }
      if (request.orderId) params.order_id = request.orderId
      if (request.assetId) params.asset_id = request.assetId
      if (request.limit !== undefined) params.limit = String(request.limit)
      return fetchAndValidate(
        executionTimelineResponseSchema,
        buildUrl(base, '/api/v1/execution/orders', params),
        { signal },
      )
    },
    enabled: Boolean(request),
    staleTime: BACKGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useDatasets(opts?: Partial<UseQueryOptions<DatasetSchemaResponse>>) {
  const base = getApiBaseUrl()
  return useQuery({
    queryKey: queryKeys.datasets,
    queryFn: ({ signal }) =>
      fetchAndValidate(datasetSchemaResponseSchema, buildUrl(base, '/api/v1/query/datasets'), {
        signal,
      }),
    staleTime: 60_000,
    ...opts,
  })
}

export function useQuerySql() {
  const base = getApiBaseUrl()
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (sql: string) => {
      return postAndValidate(queryResultResponseSchema, buildUrl(base, '/api/v1/query/sql'), {
        sql,
      })
    },
    onSuccess: (data) => {
      queryClient.setQueryData(['query-result'], data)
    },
  })
}

export { FOREGROUND_INTERVAL_MS, BACKGROUND_INTERVAL_MS }
