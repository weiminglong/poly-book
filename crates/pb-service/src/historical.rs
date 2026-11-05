//! Parquet-backed replay and integrity queries.

use pb_replay::{EventReader, ParquetReader, ReplayEngine};
use pb_types::AssetId;

use crate::{
    build_integrity_summary, build_replay_result, map_replay_error, validate_time_window,
    IntegritySummary, ReplayResult, ServiceError,
};

/// Historical queries over the Parquet event store: point-in-time book
/// reconstruction and integrity summaries. Cheap to clone; shares one
/// configured object-store reader (local files or S3).
#[derive(Clone)]
pub struct HistoricalService {
    reader: ParquetReader,
}

impl HistoricalService {
    /// Local-path convenience constructor.
    pub fn new(base_path: impl Into<String>) -> Self {
        Self::from_reader(ParquetReader::new(base_path.into()))
    }

    pub fn from_reader(reader: ParquetReader) -> Self {
        Self { reader }
    }

    /// Reconstruct the order book at a specific timestamp.
    pub async fn reconstruct(
        &self,
        asset_id: &AssetId,
        at_us: u64,
        mode: pb_types::event::ReplayMode,
        depth: Option<usize>,
    ) -> Result<ReplayResult, ServiceError> {
        let engine = ReplayEngine::new(self.reader.clone());
        let result = engine
            .reconstruct_at(asset_id, at_us, mode)
            .await
            .map_err(map_replay_error)?;
        Ok(build_replay_result(asset_id, at_us, depth, result))
    }

    /// Summarize data integrity for an asset over a bounded time window.
    pub async fn integrity_summary(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<IntegritySummary, ServiceError> {
        validate_time_window(start_us, end_us)?;
        let window = self
            .reader
            .read_market_data(asset_id, start_us, end_us)
            .await
            .map_err(map_replay_error)?;
        let validations = self
            .reader
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use object_store::ObjectStore;
    use pb_store::ParquetRecordWriter;
    use pb_types::event::{
        BookEvent, BookEventKind, DataSource, EventProvenance, IngestEventKind, PersistedRecord,
        ReplayMode, Side,
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
    async fn historical_reconstructs_book_from_parquet() {
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

        let service = HistoricalService::new(&base_path);
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
    async fn historical_returns_not_found_for_missing_data() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let service = HistoricalService::new(&base_path);
        let asset_id = AssetId::new("nonexistent");

        let err = service
            .reconstruct(&asset_id, 1_000_000, ReplayMode::RecvTime, None)
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::NotFound(_)));
    }

    #[tokio::test]
    async fn historical_respects_depth_limit() {
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

        let service = HistoricalService::new(&base_path);
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
    async fn historical_integrity_counts_events() {
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

        let service = HistoricalService::new(&base_path);
        let asset_id = AssetId::new("tok1");
        let end_ts = base_ts + 1_000_000;
        let summary = service
            .integrity_summary(&asset_id, base_ts, end_ts)
            .await
            .unwrap();

        assert_eq!(summary.asset_id, "tok1");
        assert_eq!(summary.book_event_count, 1);
        assert!(summary.ingest_event_count >= 1);
        assert!(summary.gap_count >= 1);
        assert_eq!(summary.completeness, CompletenessLevel::Partial);
    }

    #[tokio::test]
    async fn historical_integrity_full_completeness_without_gaps() {
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

        let service = HistoricalService::new(&base_path);
        let asset_id = AssetId::new("tok1");
        let end_ts = base_ts + 1_000_000;
        let summary = service
            .integrity_summary(&asset_id, base_ts, end_ts)
            .await
            .unwrap();

        assert_eq!(summary.completeness, CompletenessLevel::Full);
        assert_eq!(summary.gap_count, 0);
        assert_eq!(summary.reconnect_count, 0);
    }
}
