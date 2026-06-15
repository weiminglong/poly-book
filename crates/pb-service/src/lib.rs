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
    guard_sql, AnyQueryService, ClickHouseQueryService, DatasetSchema, QueryColumnInfo, QueryGuard,
    QueryResult, QueryService,
};

use pb_replay::ReplayError;
use pb_types::event::{ExecutionEvent, IngestEventKind, ReplayMode};
use pb_types::{AssetId, FixedPrice, FixedSize, IngestEvent, MarketDataWindow, ReplayValidation};

// ---------------------------------------------------------------------------
// Shared helpers used by both Parquet and ClickHouse backends
// ---------------------------------------------------------------------------

pub(crate) fn map_replay_error(error: ReplayError) -> ServiceError {
    match error {
        ReplayError::NoSnapshotFound {
            asset_id,
            timestamp_us,
        } => ServiceError::NotFound(format!(
            "no snapshot found for asset {asset_id} before timestamp {timestamp_us}"
        )),
        other => ServiceError::Internal(other.to_string()),
    }
}

pub(crate) fn ingest_to_continuity(event: &IngestEvent) -> ContinuityEvent {
    ContinuityEvent {
        kind: event.kind.to_string(),
        recv_timestamp_us: event.provenance.recv_timestamp_us,
        exchange_timestamp_us: event.provenance.exchange_timestamp_us,
        details: event.details.clone(),
    }
}

/// Build a `ReplayResult` from the replay engine output.
pub(crate) fn build_replay_result(
    asset_id: &AssetId,
    at_us: u64,
    depth: Option<usize>,
    result: pb_replay::ReplayResult,
) -> ReplayResult {
    let book = &result.book;
    let d = depth.unwrap_or(book.bid_depth().max(book.ask_depth()));

    ReplayResult {
        asset_id: asset_id.to_string(),
        timestamp_us: at_us,
        mode: result.mode,
        sequence: book.sequence.raw(),
        best_bid: book.best_bid(),
        best_ask: book.best_ask(),
        mid_price: book.mid_price(),
        spread: book.spread(),
        bid_depth: book.bid_depth(),
        ask_depth: book.ask_depth(),
        bids: book.top_bids(d),
        asks: book.top_asks(d),
        used_checkpoint: result.used_checkpoint,
        continuity_events: result
            .continuity_events
            .iter()
            .map(ingest_to_continuity)
            .collect(),
    }
}

/// Build an `IntegritySummary` from a market data window and validations.
pub(crate) fn build_integrity_summary(
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
    window: MarketDataWindow,
    validations: Vec<ReplayValidation>,
) -> IntegritySummary {
    let book_event_count = window.book_events.len();
    let ingest_event_count = window.ingest_events.len();

    let mut reconnect_count = 0usize;
    let mut gap_count = 0usize;
    let mut stale_snapshot_skip_count = 0usize;

    for event in &window.ingest_events {
        match event.kind {
            IngestEventKind::ReconnectStart | IngestEventKind::ReconnectSuccess => {
                reconnect_count += 1;
            }
            IngestEventKind::SequenceGap | IngestEventKind::SourceReset => {
                gap_count += 1;
            }
            IngestEventKind::StaleSnapshotSkip => {
                stale_snapshot_skip_count += 1;
            }
        }
    }

    let validation_count = validations.len();
    let validation_match_count = validations.iter().filter(|v| v.matched).count();

    let has_boundaries = reconnect_count > 0 || gap_count > 0;
    let completeness = if book_event_count == 0 {
        CompletenessLevel::Empty
    } else if has_boundaries {
        CompletenessLevel::Partial
    } else {
        CompletenessLevel::Full
    };

    let continuity_events = window
        .ingest_events
        .iter()
        .map(ingest_to_continuity)
        .collect();

    IntegritySummary {
        asset_id: asset_id.to_string(),
        start_us,
        end_us,
        book_event_count,
        trade_event_count: 0,
        ingest_event_count,
        checkpoint_count: 0,
        reconnect_count,
        gap_count,
        stale_snapshot_skip_count,
        validation_count,
        validation_match_count,
        completeness,
        continuity_events,
    }
}

/// Build an `ExecutionTimeline` from raw events with optional asset filter and limit.
pub(crate) fn build_execution_timeline(
    mut events: Vec<ExecutionEvent>,
    asset_id: Option<&AssetId>,
    limit: usize,
    offset: usize,
    descending: bool,
) -> ExecutionTimeline {
    if let Some(filter_id) = asset_id {
        events.retain(|e| {
            e.asset_id
                .as_ref()
                .map(|id| id == filter_id)
                .unwrap_or(false)
        });
    }

    let total_count = events.len();
    // Events arrive sorted ascending by the execution total-order tie-break
    // (event_timestamp_us, order_id, event_kind). To page from the most recent
    // event (what an execution inspector usually wants) reverse first, then apply
    // offset/limit. With `descending` + `offset = 0` this yields the most recent
    // `limit` events; increasing `offset` pages backwards in time. Ascending
    // order pages forward from the window start. `total_count` always reports the
    // true filtered total so callers know more exist beyond the page (A.65).
    if descending {
        events.reverse();
    }
    let events: Vec<ExecutionEvent> = events.into_iter().skip(offset).take(limit).collect();

    ExecutionTimeline {
        events,
        total_count,
    }
}

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
    ///
    /// `offset` skips that many events from the start of the ordered page and
    /// `descending` selects most-recent-first ordering, together providing
    /// server-side pagination over the full window (A.65).
    #[allow(clippy::too_many_arguments)]
    fn timeline(
        &self,
        asset_id: Option<&AssetId>,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
        limit: usize,
        offset: usize,
        descending: bool,
    ) -> impl std::future::Future<Output = Result<ExecutionTimeline, ServiceError>> + Send;
}

/// Execution timeline query result.
#[derive(Debug, Clone)]
pub struct ExecutionTimeline {
    pub events: Vec<ExecutionEvent>,
    pub total_count: usize,
}

// ---------------------------------------------------------------------------
// Shared request validation (enforced for ALL callers — HTTP and gRPC)
// ---------------------------------------------------------------------------

/// Maximum queryable time window. Bounds work (and `hour_paths` iteration) so a
/// hostile `end_us` cannot drive billions of iterations / OOM the process. This
/// lives in the service layer rather than the HTTP handler so gRPC inherits it
/// too (audit findings A.22/A.64).
pub const MAX_QUERY_WINDOW_US: u64 = 24 * 3_600 * 1_000_000; // 24 hours

/// Maximum number of execution events returned in a single timeline query.
pub const MAX_EXECUTION_LIMIT: usize = 10_000;

/// Validate a `[start_us, end_us)` query window.
pub fn validate_time_window(start_us: u64, end_us: u64) -> Result<(), ServiceError> {
    if start_us >= end_us {
        return Err(ServiceError::InvalidParams(
            "start_us must be less than end_us".to_string(),
        ));
    }
    if end_us - start_us > MAX_QUERY_WINDOW_US {
        return Err(ServiceError::InvalidParams(format!(
            "time window exceeds maximum of {} hours",
            MAX_QUERY_WINDOW_US / 3_600_000_000
        )));
    }
    Ok(())
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
        validate_time_window(start_us, end_us)?;
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
        offset: usize,
        descending: bool,
    ) -> Result<ExecutionTimeline, ServiceError> {
        validate_time_window(start_us, end_us)?;
        // Clamp the caller-supplied limit so a gRPC client cannot request an
        // unbounded result set (A.64). 0 is treated as "use the max".
        let limit = if limit == 0 {
            MAX_EXECUTION_LIMIT
        } else {
            limit.min(MAX_EXECUTION_LIMIT)
        };
        match self {
            Self::Parquet(s) => {
                s.timeline(
                    asset_id, order_id, start_us, end_us, limit, offset, descending,
                )
                .await
            }
            Self::ClickHouse(s) => {
                s.timeline(
                    asset_id, order_id, start_us, end_us, limit, offset, descending,
                )
                .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb_types::event::{DataSource, EventProvenance, IngestEventKind};

    // -----------------------------------------------------------------------
    // validate_time_window
    // -----------------------------------------------------------------------

    #[test]
    fn validate_time_window_accepts_valid_range() {
        assert!(validate_time_window(1, 1 + MAX_QUERY_WINDOW_US).is_ok());
        assert!(validate_time_window(1_000, 2_000).is_ok());
    }

    #[test]
    fn validate_time_window_rejects_inverted_and_oversized() {
        assert!(validate_time_window(5, 5).is_err());
        assert!(validate_time_window(10, 5).is_err());
        // A far-future end_us (the OOM vector) must be rejected.
        assert!(validate_time_window(0, u64::MAX).is_err());
        assert!(validate_time_window(0, MAX_QUERY_WINDOW_US + 1).is_err());
    }

    // -----------------------------------------------------------------------
    // map_replay_error
    // -----------------------------------------------------------------------

    #[test]
    fn map_replay_error_no_snapshot_becomes_not_found() {
        let err = ReplayError::NoSnapshotFound {
            asset_id: "tok1".to_string(),
            timestamp_us: 42,
        };
        let mapped = map_replay_error(err);
        assert!(matches!(mapped, ServiceError::NotFound(ref msg) if msg.contains("tok1")));
    }

    #[test]
    fn map_replay_error_io_becomes_internal() {
        let err = ReplayError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        let mapped = map_replay_error(err);
        assert!(matches!(mapped, ServiceError::Internal(_)));
    }

    #[test]
    fn map_replay_error_json_becomes_internal() {
        let json_err: serde_json::Error =
            serde_json::from_str::<serde_json::Value>("{{bad}}").unwrap_err();
        let err = ReplayError::Json(json_err);
        let mapped = map_replay_error(err);
        assert!(matches!(mapped, ServiceError::Internal(_)));
    }

    // -----------------------------------------------------------------------
    // ingest_to_continuity
    // -----------------------------------------------------------------------

    fn test_ingest_event(kind: IngestEventKind, details: Option<&str>) -> pb_types::IngestEvent {
        pb_types::IngestEvent {
            asset_id: Some(AssetId::new("tok1")),
            kind,
            provenance: EventProvenance {
                recv_timestamp_us: 1000,
                exchange_timestamp_us: 900,
                source: DataSource::WebSocket,
                source_event_id: None,
                source_session_id: None,
                sequence: None,
            },
            expected_sequence: None,
            observed_sequence: None,
            details: details.map(String::from),
        }
    }

    #[test]
    fn ingest_to_continuity_maps_all_fields() {
        let event = test_ingest_event(IngestEventKind::SequenceGap, Some("gap detail"));
        let cont = ingest_to_continuity(&event);
        assert_eq!(cont.kind, "sequence_gap");
        assert_eq!(cont.recv_timestamp_us, 1000);
        assert_eq!(cont.exchange_timestamp_us, 900);
        assert_eq!(cont.details.as_deref(), Some("gap detail"));
    }

    #[test]
    fn ingest_to_continuity_handles_no_details() {
        let event = test_ingest_event(IngestEventKind::ReconnectStart, None);
        let cont = ingest_to_continuity(&event);
        assert_eq!(cont.kind, "reconnect_start");
        assert!(cont.details.is_none());
    }

    #[test]
    fn ingest_to_continuity_all_kinds() {
        for kind in [
            IngestEventKind::ReconnectStart,
            IngestEventKind::ReconnectSuccess,
            IngestEventKind::SequenceGap,
            IngestEventKind::StaleSnapshotSkip,
            IngestEventKind::SourceReset,
        ] {
            let event = test_ingest_event(kind, None);
            let cont = ingest_to_continuity(&event);
            assert!(!cont.kind.is_empty());
        }
    }

    // -----------------------------------------------------------------------
    // build_integrity_summary
    // -----------------------------------------------------------------------

    fn make_window(
        book_count: usize,
        ingest_events: Vec<pb_types::IngestEvent>,
    ) -> MarketDataWindow {
        use pb_types::event::{BookEvent, BookEventKind, Side};
        let book_events: Vec<BookEvent> = (0..book_count)
            .map(|i| BookEvent {
                asset_id: AssetId::new("tok1"),
                kind: BookEventKind::Snapshot,
                side: Side::Bid,
                price: FixedPrice::new(5000).unwrap(),
                size: FixedSize::from_f64(1.0).unwrap(),
                provenance: EventProvenance {
                    recv_timestamp_us: 1000 + i as u64,
                    exchange_timestamp_us: 900 + i as u64,
                    source: DataSource::WebSocket,
                    source_event_id: None,
                    source_session_id: None,
                    sequence: None,
                },
            })
            .collect();
        MarketDataWindow {
            book_events,
            trade_events: Vec::new(),
            ingest_events,
        }
    }

    #[test]
    fn build_integrity_summary_empty_window() {
        let window = make_window(0, vec![]);
        let summary = build_integrity_summary(&AssetId::new("tok1"), 100, 200, window, vec![]);
        assert_eq!(summary.completeness, CompletenessLevel::Empty);
        assert_eq!(summary.book_event_count, 0);
        assert_eq!(summary.ingest_event_count, 0);
        assert_eq!(summary.reconnect_count, 0);
        assert_eq!(summary.gap_count, 0);
        assert_eq!(summary.stale_snapshot_skip_count, 0);
    }

    #[test]
    fn build_integrity_summary_full_completeness() {
        let window = make_window(5, vec![]);
        let summary = build_integrity_summary(&AssetId::new("tok1"), 100, 200, window, vec![]);
        assert_eq!(summary.completeness, CompletenessLevel::Full);
        assert_eq!(summary.book_event_count, 5);
    }

    #[test]
    fn build_integrity_summary_partial_with_gaps() {
        let ingest = vec![
            test_ingest_event(IngestEventKind::SequenceGap, Some("gap")),
            test_ingest_event(IngestEventKind::SourceReset, None),
        ];
        let window = make_window(3, ingest);
        let summary = build_integrity_summary(&AssetId::new("tok1"), 100, 200, window, vec![]);
        assert_eq!(summary.completeness, CompletenessLevel::Partial);
        assert_eq!(summary.gap_count, 2);
        assert_eq!(summary.reconnect_count, 0);
    }

    #[test]
    fn build_integrity_summary_partial_with_reconnects() {
        let ingest = vec![
            test_ingest_event(IngestEventKind::ReconnectStart, None),
            test_ingest_event(IngestEventKind::ReconnectSuccess, None),
        ];
        let window = make_window(2, ingest);
        let summary = build_integrity_summary(&AssetId::new("tok1"), 100, 200, window, vec![]);
        assert_eq!(summary.completeness, CompletenessLevel::Partial);
        assert_eq!(summary.reconnect_count, 2);
        assert_eq!(summary.gap_count, 0);
    }

    #[test]
    fn build_integrity_summary_counts_stale_snapshot_skips() {
        let ingest = vec![
            test_ingest_event(IngestEventKind::StaleSnapshotSkip, None),
            test_ingest_event(IngestEventKind::StaleSnapshotSkip, None),
        ];
        let window = make_window(1, ingest);
        let summary = build_integrity_summary(&AssetId::new("tok1"), 100, 200, window, vec![]);
        assert_eq!(summary.stale_snapshot_skip_count, 2);
        // StaleSnapshotSkip does not count as gap/reconnect
        assert_eq!(summary.completeness, CompletenessLevel::Full);
    }

    #[test]
    fn build_integrity_summary_validation_counts() {
        use pb_types::event::ReplayMode;
        let validations = vec![
            ReplayValidation {
                asset_id: AssetId::new("tok1"),
                mode: ReplayMode::RecvTime,
                replay_timestamp_us: 150,
                reference_timestamp_us: 150,
                matched: true,
                mismatch_summary: None,
                persisted_at_us: 200,
            },
            ReplayValidation {
                asset_id: AssetId::new("tok1"),
                mode: ReplayMode::RecvTime,
                replay_timestamp_us: 160,
                reference_timestamp_us: 160,
                matched: false,
                mismatch_summary: Some("mismatch".to_string()),
                persisted_at_us: 201,
            },
        ];
        let window = make_window(1, vec![]);
        let summary = build_integrity_summary(&AssetId::new("tok1"), 100, 200, window, validations);
        assert_eq!(summary.validation_count, 2);
        assert_eq!(summary.validation_match_count, 1);
    }

    #[test]
    fn build_integrity_summary_mixed_events() {
        let ingest = vec![
            test_ingest_event(IngestEventKind::ReconnectStart, None),
            test_ingest_event(IngestEventKind::SequenceGap, Some("gap")),
            test_ingest_event(IngestEventKind::StaleSnapshotSkip, None),
        ];
        let window = make_window(10, ingest);
        let summary = build_integrity_summary(&AssetId::new("tok1"), 100, 200, window, vec![]);
        assert_eq!(summary.book_event_count, 10);
        assert_eq!(summary.ingest_event_count, 3);
        assert_eq!(summary.reconnect_count, 1);
        assert_eq!(summary.gap_count, 1);
        assert_eq!(summary.stale_snapshot_skip_count, 1);
        assert_eq!(summary.completeness, CompletenessLevel::Partial);
        assert_eq!(summary.continuity_events.len(), 3);
    }

    // -----------------------------------------------------------------------
    // build_execution_timeline
    // -----------------------------------------------------------------------

    fn make_execution_events(
        count: usize,
        asset_id: Option<&str>,
    ) -> Vec<pb_types::ExecutionEvent> {
        use pb_types::event::{ExecutionEventKind, LatencyTrace, Side};
        (0..count)
            .map(|i| pb_types::ExecutionEvent {
                event_timestamp_us: 1000 + i as u64,
                asset_id: asset_id.map(AssetId::new),
                order_id: format!("order-{i}"),
                client_order_id: None,
                venue_order_id: None,
                kind: ExecutionEventKind::SubmitIntent,
                side: Some(Side::Bid),
                price: None,
                size: None,
                status: None,
                reason: None,
                latency: LatencyTrace::default(),
            })
            .collect()
    }

    #[test]
    fn build_execution_timeline_empty() {
        let timeline = build_execution_timeline(vec![], None, 10, 0, false);
        assert_eq!(timeline.total_count, 0);
        assert!(timeline.events.is_empty());
    }

    #[test]
    fn build_execution_timeline_no_filter() {
        let events = make_execution_events(5, Some("tok1"));
        let timeline = build_execution_timeline(events, None, 100, 0, false);
        assert_eq!(timeline.total_count, 5);
        assert_eq!(timeline.events.len(), 5);
    }

    #[test]
    fn build_execution_timeline_with_asset_filter() {
        let mut events = make_execution_events(3, Some("tok1"));
        events.extend(make_execution_events(2, Some("tok2")));
        let filter_id = AssetId::new("tok1");
        let timeline = build_execution_timeline(events, Some(&filter_id), 100, 0, false);
        assert_eq!(timeline.total_count, 3);
        assert!(timeline
            .events
            .iter()
            .all(|e| e.asset_id.as_ref().unwrap().as_str() == "tok1"));
    }

    #[test]
    fn build_execution_timeline_filter_removes_none_asset() {
        let mut events = make_execution_events(2, Some("tok1"));
        events.push(pb_types::ExecutionEvent {
            event_timestamp_us: 9999,
            asset_id: None,
            order_id: "orphan".to_string(),
            client_order_id: None,
            venue_order_id: None,
            kind: pb_types::event::ExecutionEventKind::SubmitIntent,
            side: None,
            price: None,
            size: None,
            status: None,
            reason: None,
            latency: pb_types::event::LatencyTrace::default(),
        });
        let filter_id = AssetId::new("tok1");
        let timeline = build_execution_timeline(events, Some(&filter_id), 100, 0, false);
        assert_eq!(timeline.total_count, 2);
    }

    #[test]
    fn build_execution_timeline_respects_limit() {
        let events = make_execution_events(10, Some("tok1"));
        let timeline = build_execution_timeline(events, None, 3, 0, false);
        assert_eq!(timeline.total_count, 10);
        assert_eq!(timeline.events.len(), 3);
    }

    #[test]
    fn build_execution_timeline_limit_larger_than_events() {
        let events = make_execution_events(2, Some("tok1"));
        let timeline = build_execution_timeline(events, None, 100, 0, false);
        assert_eq!(timeline.total_count, 2);
        assert_eq!(timeline.events.len(), 2);
    }

    #[test]
    fn build_execution_timeline_filter_then_limit() {
        let mut events = make_execution_events(5, Some("tok1"));
        events.extend(make_execution_events(5, Some("tok2")));
        let filter_id = AssetId::new("tok1");
        let timeline = build_execution_timeline(events, Some(&filter_id), 2, 0, false);
        assert_eq!(timeline.total_count, 5);
        assert_eq!(timeline.events.len(), 2);
    }

    #[test]
    fn build_execution_timeline_descending_returns_most_recent_first() {
        // make_execution_events stamps event_timestamp_us = 1000 + i, so
        // ascending order is 1000..1010. Descending with offset 0 must return the
        // newest events first.
        let events = make_execution_events(10, Some("tok1"));
        let timeline = build_execution_timeline(events, None, 3, 0, true);
        assert_eq!(timeline.total_count, 10);
        let ts: Vec<u64> = timeline
            .events
            .iter()
            .map(|e| e.event_timestamp_us)
            .collect();
        assert_eq!(ts, vec![1009, 1008, 1007]);
    }

    #[test]
    fn build_execution_timeline_offset_pages_forward() {
        let events = make_execution_events(10, Some("tok1"));
        // Ascending, skip the first 4, take 3 → timestamps 1004,1005,1006.
        let timeline = build_execution_timeline(events, None, 3, 4, false);
        assert_eq!(timeline.total_count, 10);
        let ts: Vec<u64> = timeline
            .events
            .iter()
            .map(|e| e.event_timestamp_us)
            .collect();
        assert_eq!(ts, vec![1004, 1005, 1006]);
    }

    #[test]
    fn build_execution_timeline_offset_beyond_total_is_empty() {
        let events = make_execution_events(5, Some("tok1"));
        let timeline = build_execution_timeline(events, None, 10, 100, false);
        assert_eq!(timeline.total_count, 5);
        assert!(timeline.events.is_empty());
    }

    // -----------------------------------------------------------------------
    // ServiceError Display
    // -----------------------------------------------------------------------

    #[test]
    fn service_error_display_variants() {
        let nf = ServiceError::NotFound("x".into());
        assert!(nf.to_string().contains("x"));

        let ip = ServiceError::InvalidParams("bad".into());
        assert!(ip.to_string().contains("bad"));

        let un = ServiceError::Unavailable("down".into());
        assert!(un.to_string().contains("down"));

        let int = ServiceError::Internal("oops".into());
        assert!(int.to_string().contains("oops"));
    }

    // -----------------------------------------------------------------------
    // CompletenessLevel
    // -----------------------------------------------------------------------

    #[test]
    fn completeness_level_debug_and_eq() {
        assert_eq!(CompletenessLevel::Full, CompletenessLevel::Full);
        assert_ne!(CompletenessLevel::Full, CompletenessLevel::Partial);
        assert_ne!(CompletenessLevel::Partial, CompletenessLevel::Sparse);
        assert_ne!(CompletenessLevel::Sparse, CompletenessLevel::Empty);
    }
}
