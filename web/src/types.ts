export type FeedMode = 'fixed_tokens' | 'auto_rotate'
export type SessionStatus = 'starting' | 'connected' | 'reconnecting'
export type ReplayMode = 'recv_time' | 'exchange_time'
export type DataSourceMode = 'api' | 'demo'

export interface ContinuityWarning {
  kind: string
  recv_timestamp_us: number
  exchange_timestamp_us: number
  details: string | null
}

export interface FeedStatusResponse {
  mode: FeedMode
  session_status: SessionStatus
  current_session_id: string | null
  active_asset_count: number
  active_assets: string[]
  last_rotation_us: number | null
  latest_global_warning: ContinuityWarning | null
}

export interface ActiveAssetSummary {
  asset_id: string
  last_recv_timestamp_us: number | null
  last_exchange_timestamp_us: number | null
  stale: boolean
  has_book: boolean
}

export interface PriceLevelView {
  price: string
  size: string
}

export interface LiveOrderBookSnapshot {
  asset_id: string
  sequence: number
  last_update_us: number
  best_bid: PriceLevelView | null
  best_ask: PriceLevelView | null
  mid_price: number | null
  spread: number | null
  bid_depth: number
  ask_depth: number
  bids: PriceLevelView[]
  asks: PriceLevelView[]
  stale: boolean
  latest_warning: ContinuityWarning | null
}

export interface ReplayReconstructionResponse {
  asset_id: string
  mode: ReplayMode
  used_checkpoint: boolean
  sequence: number
  last_update_us: number
  best_bid: PriceLevelView | null
  best_ask: PriceLevelView | null
  mid_price: number | null
  spread: number | null
  bid_depth: number
  ask_depth: number
  bids: PriceLevelView[]
  asks: PriceLevelView[]
  continuity_events: ContinuityWarning[]
}

export interface ApiErrorResponse {
  error: string
}

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

export interface RequestOptions {
  signal?: AbortSignal
}

// --- Integrity types ---

export type CompletenessLabel = 'complete' | 'best_effort'

export interface IntegritySummaryResponse {
  asset_id: string
  start_us: number
  end_us: number
  total_book_events: number
  total_ingest_events: number
  reconnect_count: number
  gap_count: number
  stale_snapshot_skip_count: number
  validation_count: number
  validations_matched: number
  validations_mismatched: number
  completeness: CompletenessLabel
  continuity_events: ContinuityWarning[]
}

// --- Latency types ---

export interface LatencySummaryResponse {
  window_start_us: number
  window_end_us: number
  sample_count: number
  ws_latency_p50_us: number | null
  ws_latency_p99_us: number | null
  processing_p50_us: number | null
  processing_p99_us: number | null
  storage_flush_p50_us: number | null
  storage_flush_p99_us: number | null
}

// --- Execution timeline types ---

export interface LatencyTraceView {
  market_data_recv_us: number | null
  normalization_done_us: number | null
  strategy_decision_us: number | null
  order_submit_us: number | null
  exchange_ack_us: number | null
  exchange_fill_us: number | null
}

export interface ExecutionEventView {
  event_timestamp_us: number
  asset_id: string | null
  order_id: string
  client_order_id: string | null
  venue_order_id: string | null
  kind: string
  side: string | null
  price: string | null
  size: string | null
  status: string | null
  reason: string | null
  latency: LatencyTraceView
}

export interface ExecutionTimelineResponse {
  events: ExecutionEventView[]
  total_count: number
}

// --- Query workbench types ---

export interface QueryColumn {
  name: string
  data_type: string
}

export interface QueryResultResponse {
  columns: QueryColumn[]
  rows: unknown[][]
  row_count: number
  truncated: boolean
  execution_time_ms: number
}

export interface DatasetInfo {
  name: string
  description: string
  columns: QueryColumn[]
}

export interface DatasetSchemaResponse {
  datasets: DatasetInfo[]
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

export interface WorkstationClient {
  getFeedStatus(options?: RequestOptions): Promise<FeedStatusResponse>
  getActiveAssets(options?: RequestOptions): Promise<ActiveAssetSummary[]>
  getOrderBookSnapshot(
    assetId: string,
    depth: number,
    options?: RequestOptions,
  ): Promise<LiveOrderBookSnapshot>
  getReplayReconstruction(
    request: ReplayRequest,
    options?: RequestOptions,
  ): Promise<ReplayReconstructionResponse>
  getIntegritySummary(
    request: IntegrityRequest,
    options?: RequestOptions,
  ): Promise<IntegritySummaryResponse>
  getExecutionTimeline(
    request: ExecutionRequest,
    options?: RequestOptions,
  ): Promise<ExecutionTimelineResponse>
}
