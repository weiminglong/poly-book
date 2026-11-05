//! Transport-neutral domain service layer for the poly-book workstation.
//!
//! [`HistoricalService`] answers replay and integrity queries over the Parquet
//! store; the `pb-api` crate uses it as a thin adapter (parse HTTP → call
//! service → format response). Domain result types live here so the API
//! layer can map them to DTOs without depending on storage internals.

mod error;
mod historical;

pub use error::ServiceError;
pub use historical::HistoricalService;

use pb_replay::ReplayError;
use pb_types::event::{IngestEventKind, ReplayMode};
use pb_types::{AssetId, FixedPrice, FixedSize, IngestEvent, MarketDataWindow, ReplayValidation};

// ---------------------------------------------------------------------------
// Shared helpers
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
    let ingest_events = &window.ingest_events;
    let validation_count = validations.len();
    let validation_match_count = validations.iter().filter(|v| v.matched).count();
    let ingest_event_count = ingest_events.len();

    let mut reconnect_count = 0usize;
    let mut gap_count = 0usize;
    let mut stale_snapshot_skip_count = 0usize;

    for event in ingest_events {
        match event.kind {
            IngestEventKind::ReconnectStart | IngestEventKind::ReconnectSuccess => {
                reconnect_count += 1;
            }
            IngestEventKind::SequenceGap
            | IngestEventKind::SourceReset
            | IngestEventKind::BookMismatch => {
                gap_count += 1;
            }
            IngestEventKind::StaleSnapshotSkip => {
                stale_snapshot_skip_count += 1;
            }
        }
    }

    let has_boundaries = reconnect_count > 0 || gap_count > 0;
    let completeness = if book_event_count == 0 {
        CompletenessLevel::Empty
    } else if has_boundaries {
        CompletenessLevel::Partial
    } else {
        CompletenessLevel::Full
    };

    let continuity_events = ingest_events.iter().map(ingest_to_continuity).collect();

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

// ---------------------------------------------------------------------------
// Domain result types
// ---------------------------------------------------------------------------

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
// Shared request validation
// ---------------------------------------------------------------------------

/// Maximum queryable time window. Bounds work (and `hour_paths` iteration) so a
/// hostile `end_us` cannot drive billions of iterations / OOM the process.
pub const MAX_QUERY_WINDOW_US: u64 = 24 * 3_600 * 1_000_000; // 24 hours

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
                ingest_ordinal: None,
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
                    ingest_ordinal: None,
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

    // -----------------------------------------------------------------------
    // CompletenessLevel
    // -----------------------------------------------------------------------
}
