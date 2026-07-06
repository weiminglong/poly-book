import { z } from 'zod'

// --- Enums ---

export const feedModeSchema = z.enum(['fixed_tokens', 'auto_rotate'])
export const sessionStatusSchema = z.enum(['starting', 'connected', 'reconnecting'])
export const replayModeSchema = z.enum(['recv_time', 'exchange_time'])
export const dataSourceModeSchema = z.enum(['api', 'demo'])
export const completenessLabelSchema = z.enum(['complete', 'best_effort'])

// --- Shared schemas ---

export const continuityWarningSchema = z.object({
  kind: z.string(),
  recv_timestamp_us: z.number(),
  exchange_timestamp_us: z.number(),
  details: z.string().nullable(),
})

export const priceLevelViewSchema = z.object({
  price: z.string(),
  size: z.string(),
})

// --- API response schemas ---

export const assetRefSchema = z.object({
  asset_id: z.string(),
  slug: z.string().optional(),
})

export const feedStatusResponseSchema = z.object({
  mode: feedModeSchema,
  session_status: sessionStatusSchema,
  current_session_id: z.string().nullable(),
  active_asset_count: z.number(),
  active_assets: z.array(assetRefSchema),
  last_rotation_us: z.number().nullable(),
  latest_global_warning: continuityWarningSchema.nullable(),
})

export const activeAssetSummarySchema = z.object({
  asset_id: z.string(),
  slug: z.string().optional(),
  label: z.string().optional(),
  last_recv_timestamp_us: z.number().nullable(),
  last_exchange_timestamp_us: z.number().nullable(),
  stale: z.boolean(),
  has_book: z.boolean(),
})

export const activeAssetsResponseSchema = z.array(activeAssetSummarySchema)

export const liveOrderBookSnapshotSchema = z.object({
  asset_id: z.string(),
  slug: z.string().optional(),
  sequence: z.number(),
  last_update_us: z.number(),
  best_bid: priceLevelViewSchema.nullable(),
  best_ask: priceLevelViewSchema.nullable(),
  mid_price: z.number().nullable(),
  spread: z.number().nullable(),
  bid_depth: z.number(),
  ask_depth: z.number(),
  bids: z.array(priceLevelViewSchema),
  asks: z.array(priceLevelViewSchema),
  stale: z.boolean(),
  latest_warning: continuityWarningSchema.nullable(),
})

export const replayReconstructionResponseSchema = z.object({
  asset_id: z.string(),
  slug: z.string().optional(),
  mode: replayModeSchema,
  used_checkpoint: z.boolean(),
  sequence: z.number(),
  last_update_us: z.number(),
  best_bid: priceLevelViewSchema.nullable(),
  best_ask: priceLevelViewSchema.nullable(),
  mid_price: z.number().nullable(),
  spread: z.number().nullable(),
  bid_depth: z.number(),
  ask_depth: z.number(),
  bids: z.array(priceLevelViewSchema),
  asks: z.array(priceLevelViewSchema),
  continuity_events: z.array(continuityWarningSchema),
})

export const latencyTraceViewSchema = z.object({
  market_data_recv_us: z.number().nullable(),
  normalization_done_us: z.number().nullable(),
  strategy_decision_us: z.number().nullable(),
  order_submit_us: z.number().nullable(),
  exchange_ack_us: z.number().nullable(),
  exchange_fill_us: z.number().nullable(),
})

export const executionEventViewSchema = z.object({
  event_timestamp_us: z.number(),
  asset_id: z.string().nullable(),
  order_id: z.string(),
  client_order_id: z.string().nullable(),
  venue_order_id: z.string().nullable(),
  kind: z.string(),
  side: z.string().nullable(),
  price: z.string().nullable(),
  size: z.string().nullable(),
  status: z.string().nullable(),
  reason: z.string().nullable(),
  latency: latencyTraceViewSchema,
})

export const executionTimelineResponseSchema = z.object({
  events: z.array(executionEventViewSchema),
  total_count: z.number(),
})

export const integritySummaryResponseSchema = z.object({
  asset_id: z.string(),
  slug: z.string().optional(),
  start_us: z.number(),
  end_us: z.number(),
  total_book_events: z.number(),
  total_ingest_events: z.number(),
  reconnect_count: z.number(),
  gap_count: z.number(),
  stale_snapshot_skip_count: z.number(),
  validation_count: z.number(),
  validations_matched: z.number(),
  validations_mismatched: z.number(),
  completeness: completenessLabelSchema,
  continuity_events: z.array(continuityWarningSchema),
})

export const queryColumnSchema = z.object({
  name: z.string(),
  data_type: z.string(),
})

export const queryResultResponseSchema = z.object({
  columns: z.array(queryColumnSchema),
  rows: z.array(z.array(z.unknown())),
  row_count: z.number(),
  truncated: z.boolean(),
  execution_time_ms: z.number(),
})

export const datasetInfoSchema = z.object({
  name: z.string(),
  description: z.string(),
  columns: z.array(queryColumnSchema),
})

export const datasetSchemaResponseSchema = z.object({
  datasets: z.array(datasetInfoSchema),
})

export const apiErrorResponseSchema = z.object({
  error: z.string(),
})

// --- WebSocket message schema ---

export const bookUpdateMessageSchema = z.object({
  asset_id: z.string(),
  slug: z.string().optional(),
  sequence: z.number(),
  last_update_us: z.number(),
  // True total book depth — distinct from bids/asks length, which
  // is capped at the streamed depth.
  bid_depth: z.number(),
  ask_depth: z.number(),
  bids: z.array(priceLevelViewSchema),
  asks: z.array(priceLevelViewSchema),
  mid_price: z.number().nullable(),
  spread: z.number().nullable(),
})
