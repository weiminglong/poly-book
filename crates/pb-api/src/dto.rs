use pb_types::{FixedPrice, FixedSize};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedMode {
    FixedTokens,
    AutoRotate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Starting,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuityWarning {
    pub kind: String,
    pub recv_timestamp_us: u64,
    pub exchange_timestamp_us: u64,
    pub details: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetRef {
    pub asset_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedStatusResponse {
    pub mode: FeedMode,
    pub session_status: SessionStatus,
    pub current_session_id: Option<String>,
    pub active_asset_count: usize,
    pub active_assets: Vec<AssetRef>,
    pub last_rotation_us: Option<u64>,
    pub latest_global_warning: Option<ContinuityWarning>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssetResolveResponse {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveAssetSummary {
    pub asset_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub last_recv_timestamp_us: Option<u64>,
    pub last_exchange_timestamp_us: Option<u64>,
    pub stale: bool,
    pub has_book: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceLevelView {
    pub price: FixedPrice,
    pub size: FixedSize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveOrderBookSnapshot {
    pub asset_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub sequence: u64,
    pub last_update_us: u64,
    pub best_bid: Option<PriceLevelView>,
    pub best_ask: Option<PriceLevelView>,
    pub mid_price: Option<f64>,
    pub spread: Option<f64>,
    pub bid_depth: usize,
    pub ask_depth: usize,
    pub bids: Vec<PriceLevelView>,
    pub asks: Vec<PriceLevelView>,
    pub stale: bool,
    pub latest_warning: Option<ContinuityWarning>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayReconstructionResponse {
    pub asset_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub mode: String,
    pub used_checkpoint: bool,
    pub sequence: u64,
    pub last_update_us: u64,
    pub best_bid: Option<PriceLevelView>,
    pub best_ask: Option<PriceLevelView>,
    pub mid_price: Option<f64>,
    pub spread: Option<f64>,
    pub bid_depth: usize,
    pub ask_depth: usize,
    pub bids: Vec<PriceLevelView>,
    pub asks: Vec<PriceLevelView>,
    pub continuity_events: Vec<ContinuityWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorResponse {
    pub error: String,
}

// --- Integrity types ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletenessLabel {
    Complete,
    BestEffort,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntegritySummaryResponse {
    pub asset_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub start_us: u64,
    pub end_us: u64,
    pub total_book_events: u64,
    pub total_ingest_events: u64,
    pub reconnect_count: u32,
    pub gap_count: u32,
    pub stale_snapshot_skip_count: u32,
    pub validation_count: u32,
    pub validations_matched: u32,
    pub validations_mismatched: u32,
    pub completeness: CompletenessLabel,
    pub continuity_events: Vec<ContinuityWarning>,
}

// --- WebSocket streaming types ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookUpdateMessage {
    pub asset_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub sequence: u64,
    pub last_update_us: u64,
    // True total book depth, distinct from `bids.len()`/`asks.len()` which are
    // capped at the streamed depth. Carrying them lets the client report the real
    // depth instead of under-reporting it from the truncated arrays
    // (matches the HTTP snapshot, which already exposes these).
    pub bid_depth: usize,
    pub ask_depth: usize,
    pub bids: Vec<PriceLevelView>,
    pub asks: Vec<PriceLevelView>,
    pub mid_price: Option<f64>,
    pub spread: Option<f64>,
}
