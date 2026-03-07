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
}
