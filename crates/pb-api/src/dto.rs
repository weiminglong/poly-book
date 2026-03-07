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
pub struct FeedStatusResponse {
    pub mode: FeedMode,
    pub session_status: SessionStatus,
    pub current_session_id: Option<String>,
    pub active_asset_count: usize,
    pub active_assets: Vec<String>,
    pub last_rotation_us: Option<u64>,
    pub latest_global_warning: Option<ContinuityWarning>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveAssetSummary {
    pub asset_id: String,
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
