import type { z } from 'zod'
import type {
  activeAssetSummarySchema,
  bookUpdateMessageSchema,
  completenessLabelSchema,
  continuityWarningSchema,
  dataSourceModeSchema,
  datasetInfoSchema,
  datasetSchemaResponseSchema,
  executionEventViewSchema,
  executionTimelineResponseSchema,
  feedModeSchema,
  feedStatusResponseSchema,
  integritySummaryResponseSchema,
  latencyTraceViewSchema,
  liveOrderBookSnapshotSchema,
  priceLevelViewSchema,
  queryColumnSchema,
  queryResultResponseSchema,
  replayModeSchema,
  replayReconstructionResponseSchema,
  sessionStatusSchema,
} from '../shared/api/schemas'

// Inferred types from Zod schemas
export type FeedMode = z.infer<typeof feedModeSchema>
export type SessionStatus = z.infer<typeof sessionStatusSchema>
export type ReplayMode = z.infer<typeof replayModeSchema>
export type DataSourceMode = z.infer<typeof dataSourceModeSchema>
export type CompletenessLabel = z.infer<typeof completenessLabelSchema>

export type ContinuityWarning = z.infer<typeof continuityWarningSchema>
export type PriceLevelView = z.infer<typeof priceLevelViewSchema>

export type FeedStatusResponse = z.infer<typeof feedStatusResponseSchema>
export type ActiveAssetSummary = z.infer<typeof activeAssetSummarySchema>
export type LiveOrderBookSnapshot = z.infer<typeof liveOrderBookSnapshotSchema>
export type ReplayReconstructionResponse = z.infer<typeof replayReconstructionResponseSchema>
export type LatencyTraceView = z.infer<typeof latencyTraceViewSchema>
export type ExecutionEventView = z.infer<typeof executionEventViewSchema>
export type ExecutionTimelineResponse = z.infer<typeof executionTimelineResponseSchema>
export type IntegritySummaryResponse = z.infer<typeof integritySummaryResponseSchema>
export type QueryColumn = z.infer<typeof queryColumnSchema>
export type QueryResultResponse = z.infer<typeof queryResultResponseSchema>
export type DatasetInfo = z.infer<typeof datasetInfoSchema>
export type DatasetSchemaResponse = z.infer<typeof datasetSchemaResponseSchema>
export type BookUpdateMessage = z.infer<typeof bookUpdateMessageSchema>

// Client-only types (not from API)
export interface ReplayFormValues {
  assetId: string
  atUs: string
  mode: ReplayMode
  depth: number
}

export interface ReplayRequest {
  assetId: string
  atUs: number
  mode: ReplayMode
  depth: number
}

export interface IntegrityRequest {
  assetId: string
  startUs: number
  endUs: number
}

export interface ExecutionRequest {
  orderId?: string
  assetId?: string
  startUs: number
  endUs: number
  limit?: number
}

export interface RequestOptions {
  signal?: AbortSignal
  /** Per-request timeout override in ms. Defaults to a short timeout suitable
   *  for snappy reads; analytic endpoints (e.g. the SQL workbench) pass a longer
   *  value so legitimate queries are not aborted prematurely (A.73). */
  timeoutMs?: number
}
