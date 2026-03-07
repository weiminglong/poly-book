import {
  demoActiveAssets,
  demoFeedStatus,
  demoReplayDefaults,
  getDemoReplay,
  getDemoSnapshot,
} from './demoData'
import { requestTimeoutMs } from './constants'
import type {
  ActiveAssetSummary,
  ApiErrorResponse,
  DataSourceMode,
  FeedStatusResponse,
  LiveOrderBookSnapshot,
  RequestOptions,
  ReplayReconstructionResponse,
  WorkstationClient,
} from './types'

const defaultApiBaseUrl = import.meta.env.VITE_API_BASE_URL ?? ''
export const sourceStorageKey = 'pb-workstation-source-mode'

export function createWorkstationClient(
  sourceMode: DataSourceMode,
  apiBaseUrl = defaultApiBaseUrl,
): WorkstationClient {
  if (sourceMode === 'demo') {
    return {
      getFeedStatus: async () => demoFeedStatus,
      getActiveAssets: async () => demoActiveAssets,
      getOrderBookSnapshot: async (assetId, depth) => getDemoSnapshot(assetId, depth),
      getReplayReconstruction: async ({ assetId, mode, depth }) =>
        getDemoReplay(assetId, mode, depth),
    }
  }

  return {
    getFeedStatus: (options) =>
      fetchJson<FeedStatusResponse>(buildUrl(apiBaseUrl, '/api/v1/feed/status'), options),
    getActiveAssets: (options) =>
      fetchJson<ActiveAssetSummary[]>(buildUrl(apiBaseUrl, '/api/v1/assets/active'), options),
    getOrderBookSnapshot: (assetId, depth, options) =>
      fetchJson<LiveOrderBookSnapshot>(
        buildUrl(apiBaseUrl, `/api/v1/orderbooks/${encodeURIComponent(assetId)}/snapshot`, {
          depth: String(depth),
        }),
        options,
      ),
    getReplayReconstruction: ({ assetId, atUs, mode, depth }, options) =>
      fetchJson<ReplayReconstructionResponse>(
        buildUrl(apiBaseUrl, '/api/v1/replay/reconstruct', {
          asset_id: assetId,
          at_us: String(atUs),
          mode,
          source: 'parquet',
          depth: String(depth),
        }),
        options,
      ),
  }
}

export function getInitialSourceMode(): DataSourceMode {
  const urlMode = new URLSearchParams(window.location.search).get('source')
  if (urlMode === 'demo' || urlMode === 'api') {
    return urlMode
  }

  const storedMode = window.localStorage.getItem(sourceStorageKey)
  if (storedMode === 'demo' || storedMode === 'api') {
    return storedMode
  }

  return 'api'
}

export function persistSourceMode(sourceMode: DataSourceMode): void {
  window.localStorage.setItem(sourceStorageKey, sourceMode)
}

export function getDemoReplayDefaults() {
  return demoReplayDefaults
}

function buildUrl(pathPrefix: string, path: string, params?: Record<string, string>): string {
  const base = pathPrefix.endsWith('/') ? pathPrefix.slice(0, -1) : pathPrefix
  const url = `${base}${path}`
  if (!params) {
    return url
  }

  const query = new URLSearchParams(params)
  return `${url}?${query.toString()}`
}

async function fetchJson<T>(url: string, options?: RequestOptions): Promise<T> {
  const timeoutController = new AbortController()
  let timedOut = false
  const timeoutId = window.setTimeout(() => {
    timedOut = true
    timeoutController.abort()
  }, requestTimeoutMs)

  const abortHandler = () => timeoutController.abort()
  options?.signal?.addEventListener('abort', abortHandler, { once: true })

  try {
    const response = await fetch(url, {
      signal: timeoutController.signal,
    })
    if (!response.ok) {
      let message = `Request failed with status ${response.status}`
      try {
        const body = (await response.json()) as ApiErrorResponse
        if (typeof body.error === 'string') {
          message = body.error
        }
      } catch {
        // Ignore JSON parsing failures and fall back to the HTTP status.
      }
      throw new Error(message)
    }

    return (await response.json()) as T
  } catch (error) {
    if (timedOut) {
      throw new Error(`Request timed out after ${requestTimeoutMs}ms`)
    }
    throw error
  } finally {
    window.clearTimeout(timeoutId)
    options?.signal?.removeEventListener('abort', abortHandler)
  }
}

export function isAbortError(error: unknown): boolean {
  return error instanceof DOMException
    ? error.name === 'AbortError'
    : error instanceof Error && error.name === 'AbortError'
}
