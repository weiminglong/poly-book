import { type UseQueryOptions, useQuery } from '@tanstack/react-query'
import type {
  ActiveAssetSummary,
  FeedStatusResponse,
  IntegrityRequest,
  IntegritySummaryResponse,
  LiveOrderBookSnapshot,
  ReplayReconstructionResponse,
  ReplayRequest,
} from '../../types'
import { useSourceModeContext } from '../hooks/use-source-mode'
import { buildUrl, fetchAndValidate, getApiBaseUrl } from './client'
import {
  getDemoActiveAssets,
  getDemoFeedStatus,
  getDemoIntegrity,
  getDemoReplay,
  getDemoSnapshot,
} from './demo'
import {
  activeAssetsResponseSchema,
  feedStatusResponseSchema,
  integritySummaryResponseSchema,
  liveOrderBookSnapshotSchema,
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
              depth: String(request.depth),
            }),
            // Historical reconstruction can scan a wide window; the default 4s
            // timeout aborts legitimate queries.
            { signal, timeoutMs: 30_000 },
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
            // Integrity over a wide window can exceed the default 4s.
            { signal, timeoutMs: 30_000 },
          )
        },
    enabled: isDemo || Boolean(request),
    staleTime: isDemo ? Number.POSITIVE_INFINITY : BACKGROUND_INTERVAL_MS,
    ...opts,
  })
}

export { FOREGROUND_INTERVAL_MS, BACKGROUND_INTERVAL_MS }
