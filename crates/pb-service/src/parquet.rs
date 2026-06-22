//! Parquet-backed service implementations for historical queries.

use pb_replay::{EventReader, ParquetReader, ReplayEngine};
use pb_types::AssetId;

use crate::{
    build_execution_timeline, build_integrity_summary, build_replay_result, map_replay_error,
    ExecutionService, ExecutionTimeline, IntegrityService, IntegritySummary, ReplayResult,
    ReplayService, ServiceError,
};

// ---------------------------------------------------------------------------
// ParquetReplayService
// ---------------------------------------------------------------------------

/// Replay service backed by Parquet event files.
#[derive(Clone)]
pub struct ParquetReplayService {
    base_path: String,
}

impl ParquetReplayService {
    pub fn new(base_path: impl Into<String>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }
}

impl ReplayService for ParquetReplayService {
    async fn reconstruct(
        &self,
        asset_id: &AssetId,
        at_us: u64,
        mode: pb_types::event::ReplayMode,
        depth: Option<usize>,
    ) -> Result<ReplayResult, ServiceError> {
        let reader = ParquetReader::new(&self.base_path);
        let engine = ReplayEngine::new(reader);
        let result = engine
            .reconstruct_at(asset_id, at_us, mode)
            .await
            .map_err(map_replay_error)?;
        Ok(build_replay_result(asset_id, at_us, depth, result))
    }
}

// ---------------------------------------------------------------------------
// ParquetIntegrityService
// ---------------------------------------------------------------------------

/// Integrity service backed by Parquet event files.
#[derive(Clone)]
pub struct ParquetIntegrityService {
    base_path: String,
}

impl ParquetIntegrityService {
    pub fn new(base_path: impl Into<String>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }
}

impl IntegrityService for ParquetIntegrityService {
    async fn summary(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<IntegritySummary, ServiceError> {
        let reader = ParquetReader::new(&self.base_path);
        let window = reader
            .read_market_data(asset_id, start_us, end_us)
            .await
            .map_err(map_replay_error)?;
        let validations = reader
            .read_validations(asset_id, start_us, end_us)
            .await
            .map_err(map_replay_error)?;
        Ok(build_integrity_summary(
            asset_id,
            start_us,
            end_us,
            window,
            validations,
        ))
    }
}

// ---------------------------------------------------------------------------
// ParquetExecutionService
// ---------------------------------------------------------------------------

/// Execution service backed by Parquet event files.
#[derive(Clone)]
pub struct ParquetExecutionService {
    base_path: String,
}

impl ParquetExecutionService {
    pub fn new(base_path: impl Into<String>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }
}

impl ExecutionService for ParquetExecutionService {
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
        let reader = ParquetReader::new(&self.base_path);
        let events = reader
            .read_execution_events(order_id, start_us, end_us)
            .await
            .map_err(map_replay_error)?;
        Ok(build_execution_timeline(
            events, asset_id, limit, offset, descending,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use object_store::ObjectStore;
    use pb_store::ParquetRecordWriter;
    use pb_types::event::{
        BookEvent, BookEventKind, DataSource, EventProvenance, ExecutionEvent, ExecutionEventKind,
        IngestEventKind, LatencyTrace, PersistedRecord, ReplayMode, Side,
    };
    use pb_types::{AssetId, FixedPrice, FixedSize, IngestEvent, Sequence};

    use super::*;
    use crate::CompletenessLevel;

    fn parquet_writer(base_path: &str) -> ParquetRecordWriter {
        ParquetRecordWriter::new(
            Arc::new(object_store::local::LocalFileSystem::new()) as Arc<dyn ObjectStore>,
            base_path.to_string(),
        )
    }

    fn test_provenance(ts: u64, seq: u64) -> EventProvenance {
        EventProvenance {
            recv_timestamp_us: ts,
            exchange_timestamp_us: ts,
            source: DataSource::WebSocket,
            source_event_id: Some("snap-1".to_string()),
            source_session_id: Some("ws-session-1".to_string()),
            sequence: Some(Sequence::new(seq)),
            ingest_ordinal: None,
        }
    }

    #[tokio::test]
    async fn replay_service_reconstructs_book_from_parquet() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        writer
            .write_batch(&[
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("tok1"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Bid,
                    price: FixedPrice::new(5000).unwrap(),
                    size: FixedSize::from_f64(100.0).unwrap(),
                    provenance: test_provenance(base_ts, 0),
                }),
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("tok1"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Ask,
                    price: FixedPrice::new(5500).unwrap(),
                    size: FixedSize::from_f64(110.0).unwrap(),
                    provenance: test_provenance(base_ts, 1),
                }),
            ])
            .await
            .unwrap();

        let service = ParquetReplayService::new(&base_path);
        let asset_id = AssetId::new("tok1");
        let result = service
            .reconstruct(&asset_id, base_ts, ReplayMode::RecvTime, None)
            .await
            .unwrap();

        assert_eq!(result.asset_id, "tok1");
        assert_eq!(result.bid_depth, 1);
        assert_eq!(result.ask_depth, 1);
        assert!(result.best_bid.is_some());
        assert!(result.best_ask.is_some());
    }

    #[tokio::test]
    async fn replay_service_returns_not_found_for_missing_data() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let service = ParquetReplayService::new(&base_path);
        let asset_id = AssetId::new("nonexistent");

        let err = service
            .reconstruct(&asset_id, 1_000_000, ReplayMode::RecvTime, None)
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::NotFound(_)));
    }

    #[tokio::test]
    async fn replay_service_respects_depth_limit() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        writer
            .write_batch(&[
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("tok1"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Bid,
                    price: FixedPrice::new(5000).unwrap(),
                    size: FixedSize::from_f64(100.0).unwrap(),
                    provenance: test_provenance(base_ts, 0),
                }),
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("tok1"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Bid,
                    price: FixedPrice::new(4900).unwrap(),
                    size: FixedSize::from_f64(50.0).unwrap(),
                    provenance: test_provenance(base_ts, 1),
                }),
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("tok1"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Ask,
                    price: FixedPrice::new(5500).unwrap(),
                    size: FixedSize::from_f64(110.0).unwrap(),
                    provenance: test_provenance(base_ts, 2),
                }),
            ])
            .await
            .unwrap();

        let service = ParquetReplayService::new(&base_path);
        let asset_id = AssetId::new("tok1");
        let result = service
            .reconstruct(&asset_id, base_ts, ReplayMode::RecvTime, Some(1))
            .await
            .unwrap();

        assert_eq!(result.bids.len(), 1);
        assert_eq!(result.asks.len(), 1);
        assert_eq!(result.bid_depth, 2); // total depth is 2
    }

    #[tokio::test]
    async fn integrity_service_counts_events() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        writer
            .write_batch(&[
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("tok1"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Bid,
                    price: FixedPrice::new(5000).unwrap(),
                    size: FixedSize::from_f64(100.0).unwrap(),
                    provenance: test_provenance(base_ts, 0),
                }),
                PersistedRecord::Ingest(IngestEvent {
                    asset_id: Some(AssetId::new("tok1")),
                    kind: IngestEventKind::SequenceGap,
                    provenance: test_provenance(base_ts + 100, 0),
                    expected_sequence: Some(1),
                    observed_sequence: Some(3),
                    details: Some("gap".to_string()),
                }),
                PersistedRecord::Ingest(IngestEvent {
                    asset_id: None,
                    kind: IngestEventKind::ReconnectStart,
                    provenance: test_provenance(base_ts + 200, 0),
                    expected_sequence: None,
                    observed_sequence: None,
                    details: None,
                }),
            ])
            .await
            .unwrap();

        let service = ParquetIntegrityService::new(&base_path);
        let asset_id = AssetId::new("tok1");
        let end_ts = base_ts + 1_000_000;
        let summary = service.summary(&asset_id, base_ts, end_ts).await.unwrap();

        assert_eq!(summary.asset_id, "tok1");
        assert_eq!(summary.book_event_count, 1);
        assert!(summary.ingest_event_count >= 1);
        assert!(summary.gap_count >= 1);
        assert_eq!(summary.completeness, CompletenessLevel::Partial);
    }

    #[tokio::test]
    async fn integrity_service_full_completeness_without_gaps() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        writer
            .write_batch(&[PersistedRecord::Book(BookEvent {
                asset_id: AssetId::new("tok1"),
                kind: BookEventKind::Snapshot,
                side: Side::Bid,
                price: FixedPrice::new(5000).unwrap(),
                size: FixedSize::from_f64(100.0).unwrap(),
                provenance: test_provenance(base_ts, 0),
            })])
            .await
            .unwrap();

        let service = ParquetIntegrityService::new(&base_path);
        let asset_id = AssetId::new("tok1");
        let end_ts = base_ts + 1_000_000;
        let summary = service.summary(&asset_id, base_ts, end_ts).await.unwrap();

        assert_eq!(summary.completeness, CompletenessLevel::Full);
        assert_eq!(summary.gap_count, 0);
        assert_eq!(summary.reconnect_count, 0);
    }

    #[tokio::test]
    async fn execution_service_returns_timeline() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        writer
            .write_batch(&[
                PersistedRecord::Execution(ExecutionEvent {
                    event_timestamp_us: base_ts,
                    asset_id: Some(AssetId::new("tok1")),
                    order_id: "order-1".to_string(),
                    client_order_id: Some("client-1".to_string()),
                    venue_order_id: None,
                    kind: ExecutionEventKind::SubmitIntent,
                    side: Some(Side::Bid),
                    price: Some(FixedPrice::new(5000).unwrap()),
                    size: Some(FixedSize::from_f64(10.0).unwrap()),
                    status: None,
                    reason: None,
                    latency: LatencyTrace::default(),
                }),
                PersistedRecord::Execution(ExecutionEvent {
                    event_timestamp_us: base_ts + 100,
                    asset_id: Some(AssetId::new("tok1")),
                    order_id: "order-1".to_string(),
                    client_order_id: Some("client-1".to_string()),
                    venue_order_id: Some("venue-1".to_string()),
                    kind: ExecutionEventKind::ExchangeAck,
                    side: Some(Side::Bid),
                    price: Some(FixedPrice::new(5000).unwrap()),
                    size: Some(FixedSize::from_f64(10.0).unwrap()),
                    status: Some("accepted".to_string()),
                    reason: None,
                    latency: LatencyTrace::default(),
                }),
            ])
            .await
            .unwrap();

        let service = ParquetExecutionService::new(&base_path);
        let end_ts = base_ts + 1_000_000;
        let timeline = service
            .timeline(None, None, base_ts, end_ts, 100, 0, false)
            .await
            .unwrap();

        assert_eq!(timeline.total_count, 2);
        assert_eq!(timeline.events.len(), 2);
        assert_eq!(timeline.events[0].order_id, "order-1");
    }

    #[tokio::test]
    async fn execution_service_filters_by_asset() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        writer
            .write_batch(&[
                PersistedRecord::Execution(ExecutionEvent {
                    event_timestamp_us: base_ts,
                    asset_id: Some(AssetId::new("tok1")),
                    order_id: "order-A".to_string(),
                    client_order_id: None,
                    venue_order_id: None,
                    kind: ExecutionEventKind::SubmitIntent,
                    side: Some(Side::Bid),
                    price: None,
                    size: None,
                    status: None,
                    reason: None,
                    latency: LatencyTrace::default(),
                }),
                PersistedRecord::Execution(ExecutionEvent {
                    event_timestamp_us: base_ts + 50,
                    asset_id: Some(AssetId::new("tok2")),
                    order_id: "order-B".to_string(),
                    client_order_id: None,
                    venue_order_id: None,
                    kind: ExecutionEventKind::SubmitIntent,
                    side: Some(Side::Ask),
                    price: None,
                    size: None,
                    status: None,
                    reason: None,
                    latency: LatencyTrace::default(),
                }),
            ])
            .await
            .unwrap();

        let service = ParquetExecutionService::new(&base_path);
        let asset_id = AssetId::new("tok1");
        let end_ts = base_ts + 1_000_000;
        let timeline = service
            .timeline(Some(&asset_id), None, base_ts, end_ts, 100, 0, false)
            .await
            .unwrap();

        assert_eq!(timeline.total_count, 1);
        assert_eq!(timeline.events[0].order_id, "order-A");
    }

    #[tokio::test]
    async fn execution_service_respects_limit() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        let mut records = Vec::new();
        for i in 0..5 {
            records.push(PersistedRecord::Execution(ExecutionEvent {
                event_timestamp_us: base_ts + i * 10,
                asset_id: Some(AssetId::new("tok1")),
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
            }));
        }
        writer.write_batch(&records).await.unwrap();

        let service = ParquetExecutionService::new(&base_path);
        let end_ts = base_ts + 1_000_000;
        let timeline = service
            .timeline(None, None, base_ts, end_ts, 2, 0, true)
            .await
            .unwrap();

        assert_eq!(timeline.total_count, 5);
        assert_eq!(timeline.events.len(), 2);
        // Descending (most-recent-first) paging returns the two newest events,
        // newest first, not the two oldest.
        assert_eq!(timeline.events[0].order_id, "order-4");
        assert_eq!(timeline.events[1].order_id, "order-3");
    }
}
