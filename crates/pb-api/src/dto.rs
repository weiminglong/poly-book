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

// --- Latency types ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencySummaryResponse {
    pub window_start_us: u64,
    pub window_end_us: u64,
    pub sample_count: u64,
    pub ws_latency_p50_us: Option<u64>,
    pub ws_latency_p99_us: Option<u64>,
    pub processing_p50_us: Option<u64>,
    pub processing_p99_us: Option<u64>,
    pub storage_flush_p50_us: Option<u64>,
    pub storage_flush_p99_us: Option<u64>,
}

// --- Execution timeline types ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencyTraceView {
    pub market_data_recv_us: Option<u64>,
    pub normalization_done_us: Option<u64>,
    pub strategy_decision_us: Option<u64>,
    pub order_submit_us: Option<u64>,
    pub exchange_ack_us: Option<u64>,
    pub exchange_fill_us: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionEventView {
    pub event_timestamp_us: u64,
    pub asset_id: Option<String>,
    pub order_id: String,
    pub client_order_id: Option<String>,
    pub venue_order_id: Option<String>,
    pub kind: String,
    pub side: Option<String>,
    pub price: Option<FixedPrice>,
    pub size: Option<FixedSize>,
    pub status: Option<String>,
    pub reason: Option<String>,
    pub latency: LatencyTraceView,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionTimelineResponse {
    pub events: Vec<ExecutionEventView>,
    pub total_count: u64,
}

// --- Query workbench types ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryColumn {
    pub name: String,
    pub data_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResultResponse {
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: u64,
    pub truncated: bool,
    pub execution_time_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetInfo {
    pub name: String,
    pub description: String,
    pub columns: Vec<QueryColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetSchemaResponse {
    pub datasets: Vec<DatasetInfo>,
}

// --- WebSocket streaming types ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookUpdateMessage {
    pub asset_id: String,
    pub sequence: u64,
    pub last_update_us: u64,
    pub bids: Vec<PriceLevelView>,
    pub asks: Vec<PriceLevelView>,
    pub mid_price: Option<f64>,
    pub spread: Option<f64>,
}
