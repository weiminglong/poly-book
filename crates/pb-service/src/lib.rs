//! Transport-neutral domain service layer for the poly-book workstation.
//!
//! Defines service traits that decouple business logic from HTTP transport.
//! Concrete implementations live alongside the traits; the `pb-api` crate
//! uses these as thin adapters (parse HTTP → call service → format response).

pub mod clickhouse;
mod error;
pub mod parquet;
pub mod query;

pub use clickhouse::{
    ClickHouseExecutionService, ClickHouseIntegrityService, ClickHouseReplayService,
};
pub use error::ServiceError;
pub use parquet::{ParquetExecutionService, ParquetIntegrityService, ParquetReplayService};
pub use query::{
    AnyQueryService, ClickHouseQueryService, DatasetSchema, QueryColumnInfo, QueryGuard,
    QueryResult, QueryService,
};

use pb_types::event::{ExecutionEvent, ReplayMode};
use pb_types::{AssetId, FixedPrice, FixedSize};

// ---------------------------------------------------------------------------
// BookService — live book queries
// ---------------------------------------------------------------------------

/// Live book read operations backed by the watch-based read model.
pub trait BookService: Send + Sync {
    /// Get feed connection status and active asset list.
    fn feed_status(
        &self,
    ) -> impl std::future::Future<Output = Result<FeedStatus, ServiceError>> + Send;

    /// Get active asset summaries with staleness info.
    fn active_assets(
        &self,
        stale_after_secs: u64,
    ) -> impl std::future::Future<Output = Result<Vec<AssetSummary>, ServiceError>> + Send;

    /// Check if an asset is currently active.
    fn is_asset_active(
        &self,
        asset_id: &str,
    ) -> impl std::future::Future<Output = Result<bool, ServiceError>> + Send;

    /// Get a point-in-time snapshot of an asset's order book.
    fn snapshot(
        &self,
        asset_id: &str,
        depth: usize,
        stale_after_secs: u64,
    ) -> impl std::future::Future<Output = Result<BookSnapshot, ServiceError>> + Send;
}

/// Feed connection status.
#[derive(Debug, Clone)]
pub struct FeedStatus {
    pub mode: String,
    pub session_status: String,
    pub current_session_id: Option<String>,
    pub active_asset_count: usize,
    pub active_asset_ids: Vec<String>,
    pub last_rotation_us: Option<u64>,
}

/// Summary of an active asset.
#[derive(Debug, Clone)]
pub struct AssetSummary {
    pub asset_id: String,
    pub last_recv_timestamp_us: Option<u64>,
    pub last_exchange_timestamp_us: Option<u64>,
    pub stale: bool,
    pub has_book: bool,
}

/// Order book snapshot for a single asset.
#[derive(Debug, Clone)]
pub struct BookSnapshot {
    pub asset_id: String,
    pub sequence: u64,
    pub last_update_us: u64,
    pub best_bid: Option<(FixedPrice, FixedSize)>,
    pub best_ask: Option<(FixedPrice, FixedSize)>,
    pub mid_price: Option<f64>,
    pub spread: Option<f64>,
    pub bid_depth: usize,
    pub ask_depth: usize,
    pub bids: Vec<(FixedPrice, FixedSize)>,
    pub asks: Vec<(FixedPrice, FixedSize)>,
    pub stale: bool,
}

// ---------------------------------------------------------------------------
// ReplayService — historical book reconstruction
// ---------------------------------------------------------------------------

/// Historical order book reconstruction from stored events.
pub trait ReplayService: Send + Sync {
    /// Reconstruct the order book at a specific timestamp.
    fn reconstruct(
        &self,
        asset_id: &AssetId,
        at_us: u64,
        mode: ReplayMode,
        depth: Option<usize>,
    ) -> impl std::future::Future<Output = Result<ReplayResult, ServiceError>> + Send;
}

/// Result of a historical reconstruction.
#[derive(Debug, Clone)]
pub struct ReplayResult {
    pub asset_id: String,
    pub timestamp_us: u64,
    pub mode: ReplayMode,
    pub sequence: u64,
    pub best_bid: Option<(FixedPrice, FixedSize)>,
    pub best_ask: Option<(FixedPrice, FixedSize)>,
    pub mid_price: Option<f64>,
    pub spread: Option<f64>,
    pub bid_depth: usize,
    pub ask_depth: usize,
    pub bids: Vec<(FixedPrice, FixedSize)>,
    pub asks: Vec<(FixedPrice, FixedSize)>,
    pub used_checkpoint: bool,
    pub continuity_events: Vec<ContinuityEvent>,
}

// ---------------------------------------------------------------------------
// IntegrityService — data integrity queries
// ---------------------------------------------------------------------------

/// Data integrity and completeness assessment.
pub trait IntegrityService: Send + Sync {
    /// Summarize data integrity for an asset over a time range.
    fn summary(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<IntegritySummary, ServiceError>> + Send;
}

/// Data integrity summary for an asset over a time range.
#[derive(Debug, Clone)]
pub struct IntegritySummary {
    pub asset_id: String,
    pub start_us: u64,
    pub end_us: u64,
    pub book_event_count: usize,
    pub trade_event_count: usize,
    pub ingest_event_count: usize,
    pub checkpoint_count: usize,
    pub reconnect_count: usize,
    pub gap_count: usize,
    pub stale_snapshot_skip_count: usize,
    pub validation_count: usize,
    pub validation_match_count: usize,
    pub completeness: CompletenessLevel,
    pub continuity_events: Vec<ContinuityEvent>,
}

/// Data completeness assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletenessLevel {
    Full,
    Partial,
    Sparse,
    Empty,
}

/// A continuity event (reconnect, gap, etc.) surfaced from the data layer.
#[derive(Debug, Clone)]
pub struct ContinuityEvent {
    pub kind: String,
    pub recv_timestamp_us: u64,
    pub exchange_timestamp_us: u64,
    pub details: Option<String>,
}

// ---------------------------------------------------------------------------
// ExecutionService — execution timeline queries
// ---------------------------------------------------------------------------

/// Execution event timeline queries.
pub trait ExecutionService: Send + Sync {
    /// Query execution events over a time range with optional filters.
    fn timeline(
        &self,
        asset_id: Option<&AssetId>,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
        limit: usize,
    ) -> impl std::future::Future<Output = Result<ExecutionTimeline, ServiceError>> + Send;
}

/// Execution timeline query result.
#[derive(Debug, Clone)]
pub struct ExecutionTimeline {
    pub events: Vec<ExecutionEvent>,
    pub total_count: usize,
}

// ---------------------------------------------------------------------------
// Backend dispatch enums (enum-based polymorphism for non-dyn traits)
// ---------------------------------------------------------------------------

/// Dispatch enum for replay service backends.
#[derive(Clone)]
pub enum AnyReplayService {
    Parquet(ParquetReplayService),
    ClickHouse(ClickHouseReplayService),
}

impl ReplayService for AnyReplayService {
    async fn reconstruct(
        &self,
        asset_id: &AssetId,
        at_us: u64,
        mode: ReplayMode,
        depth: Option<usize>,
    ) -> Result<ReplayResult, ServiceError> {
        match self {
            Self::Parquet(s) => s.reconstruct(asset_id, at_us, mode, depth).await,
            Self::ClickHouse(s) => s.reconstruct(asset_id, at_us, mode, depth).await,
        }
    }
}

/// Dispatch enum for integrity service backends.
#[derive(Clone)]
pub enum AnyIntegrityService {
    Parquet(ParquetIntegrityService),
    ClickHouse(ClickHouseIntegrityService),
}

impl IntegrityService for AnyIntegrityService {
    async fn summary(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<IntegritySummary, ServiceError> {
        match self {
            Self::Parquet(s) => s.summary(asset_id, start_us, end_us).await,
            Self::ClickHouse(s) => s.summary(asset_id, start_us, end_us).await,
        }
    }
}

/// Dispatch enum for execution service backends.
#[derive(Clone)]
pub enum AnyExecutionService {
    Parquet(ParquetExecutionService),
    ClickHouse(ClickHouseExecutionService),
}

impl ExecutionService for AnyExecutionService {
    async fn timeline(
        &self,
        asset_id: Option<&AssetId>,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
        limit: usize,
    ) -> Result<ExecutionTimeline, ServiceError> {
        match self {
            Self::Parquet(s) => s.timeline(asset_id, order_id, start_us, end_us, limit).await,
            Self::ClickHouse(s) => s.timeline(asset_id, order_id, start_us, end_us, limit).await,
        }
    }
}
