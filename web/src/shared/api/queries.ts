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
import { useSourceModeContext } from '../hooks/use-source-mode'
import { buildUrl, fetchAndValidate, getApiBaseUrl, postAndValidate } from './client'
import {
  getDemoActiveAssets,
  getDemoDatasets,
  getDemoExecution,
  getDemoFeedStatus,
  getDemoIntegrity,
  getDemoReplay,
  getDemoSnapshot,
} from './demo'
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
    [
      'execution',
      req.orderId,
      req.assetId,
      req.startUs,
      req.endUs,
      req.limit,
      req.offset,
      req.order,
    ] as const,
  datasets: ['datasets'] as const,
}

// --- Query hooks ---

export function useFeedStatus(opts?: Partial<UseQueryOptions<FeedStatusResponse>>) {
  const sourceMode = useSourceModeContext()
  const base = getApiBaseUrl()
  const isDemo = sourceMode === 'demo'
  return useQuery({
    queryKey: [sourceMode, ...queryKeys.feedStatus],
    queryFn: isDemo
      ? () => getDemoFeedStatus()
      : ({ signal }) =>
          fetchAndValidate(feedStatusResponseSchema, buildUrl(base, '/api/v1/feed/status'), {
            signal,
          }),
    refetchInterval: isDemo ? false : FOREGROUND_INTERVAL_MS,
    staleTime: isDemo ? Number.POSITIVE_INFINITY : FOREGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useActiveAssets(opts?: Partial<UseQueryOptions<ActiveAssetSummary[]>>) {
  const sourceMode = useSourceModeContext()
  const base = getApiBaseUrl()
  const isDemo = sourceMode === 'demo'
  return useQuery({
    queryKey: [sourceMode, ...queryKeys.activeAssets],
    queryFn: isDemo
      ? () => getDemoActiveAssets()
      : ({ signal }) =>
          fetchAndValidate(activeAssetsResponseSchema, buildUrl(base, '/api/v1/assets/active'), {
            signal,
          }),
    refetchInterval: isDemo ? false : FOREGROUND_INTERVAL_MS,
    staleTime: isDemo ? Number.POSITIVE_INFINITY : FOREGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useOrderBookSnapshot(
  assetId: string,
  depth: number,
  opts?: Partial<UseQueryOptions<LiveOrderBookSnapshot>>,
) {
  const sourceMode = useSourceModeContext()
  const base = getApiBaseUrl()
  const isDemo = sourceMode === 'demo'
  return useQuery({
    queryKey: [sourceMode, ...queryKeys.orderbook(assetId, depth)],
    queryFn: isDemo
      ? () => getDemoSnapshot(assetId, depth)
      : ({ signal }) =>
          fetchAndValidate(
            liveOrderBookSnapshotSchema,
            buildUrl(base, `/api/v1/orderbooks/${encodeURIComponent(assetId)}/snapshot`, {
              depth: String(depth),
            }),
            { signal },
          ),
    enabled: Boolean(assetId),
    refetchInterval: isDemo ? false : FOREGROUND_INTERVAL_MS,
    staleTime: isDemo ? Number.POSITIVE_INFINITY : FOREGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useReplayReconstruction(
  request: ReplayRequest | null,
  opts?: Partial<UseQueryOptions<ReplayReconstructionResponse>>,
) {
  const sourceMode = useSourceModeContext()
  const base = getApiBaseUrl()
  const isDemo = sourceMode === 'demo'
  return useQuery({
    queryKey: [sourceMode, ...(request ? queryKeys.replay(request) : ['replay-disabled'])],
    queryFn: isDemo
      ? () => {
          if (!request) throw new Error('No replay request')
          return getDemoReplay(request.assetId, request.mode, request.depth)
        }
      : ({ signal }) => {
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
  const sourceMode = useSourceModeContext()
  const base = getApiBaseUrl()
  const isDemo = sourceMode === 'demo'
  return useQuery({
    queryKey: [sourceMode, ...(request ? queryKeys.integrity(request) : ['integrity-disabled'])],
    queryFn: isDemo
      ? () => getDemoIntegrity()
      : ({ signal }) => {
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
    enabled: isDemo || Boolean(request),
    staleTime: isDemo ? Number.POSITIVE_INFINITY : BACKGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useExecutionTimeline(
  request: ExecutionRequest | null,
  opts?: Partial<UseQueryOptions<ExecutionTimelineResponse>>,
) {
  const sourceMode = useSourceModeContext()
  const base = getApiBaseUrl()
  const isDemo = sourceMode === 'demo'
  return useQuery({
    queryKey: [sourceMode, ...(request ? queryKeys.execution(request) : ['execution-disabled'])],
    queryFn: isDemo
      ? () => getDemoExecution()
      : ({ signal }) => {
          if (!request) throw new Error('No execution request')
          const params: Record<string, string> = {
            start_us: String(request.startUs),
            end_us: String(request.endUs),
          }
          if (request.orderId) params.order_id = request.orderId
          if (request.assetId) params.asset_id = request.assetId
          if (request.limit !== undefined) params.limit = String(request.limit)
          if (request.offset !== undefined) params.offset = String(request.offset)
          if (request.order !== undefined) params.order = request.order
          return fetchAndValidate(
            executionTimelineResponseSchema,
            buildUrl(base, '/api/v1/execution/orders', params),
            { signal },
          )
        },
    enabled: isDemo || Boolean(request),
    staleTime: isDemo ? Number.POSITIVE_INFINITY : BACKGROUND_INTERVAL_MS,
    ...opts,
  })
}

export function useDatasets(opts?: Partial<UseQueryOptions<DatasetSchemaResponse>>) {
  const sourceMode = useSourceModeContext()
  const base = getApiBaseUrl()
  const isDemo = sourceMode === 'demo'
  return useQuery({
    queryKey: [sourceMode, ...queryKeys.datasets],
    queryFn: isDemo
      ? () => getDemoDatasets()
      : ({ signal }) =>
          fetchAndValidate(datasetSchemaResponseSchema, buildUrl(base, '/api/v1/query/datasets'), {
            signal,
          }),
    staleTime: isDemo ? Number.POSITIVE_INFINITY : 60_000,
    ...opts,
  })
}

export function useQuerySql() {
  const base = getApiBaseUrl()
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: async (sql: string) => {
      // The SQL workbench runs analytic queries that can legitimately take
      // longer than the default snappy-read timeout. Allow up to just above the
      // server-side query timeout so valid queries are not aborted client-side
      // (A.73).
      return postAndValidate(
        queryResultResponseSchema,
        buildUrl(base, '/api/v1/query/sql'),
        { sql },
        { timeoutMs: 35_000 },
      )
    },
    onSuccess: (data) => {
      queryClient.setQueryData(['query-result'], data)
    },
  })
}

export { FOREGROUND_INTERVAL_MS, BACKGROUND_INTERVAL_MS }
