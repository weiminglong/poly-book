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
