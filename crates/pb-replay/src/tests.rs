use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, DataSource, EventProvenance, ExecutionEvent,
    MarketDataWindow, ReplayMode, ReplayValidation, Side, TradeEvent,
};
use pb_types::{AssetId, FixedPrice, FixedSize, PriceLevel, Sequence, TradeFidelity};

use crate::engine::ReplayEngine;
use crate::error::ReplayError;
use crate::reader::{EventReader, ParquetReader};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn test_asset_id() -> AssetId {
    AssetId::new("BTC-5M-YES")
}

fn test_provenance(recv_ts: u64, seq: u64) -> EventProvenance {
    EventProvenance {
        recv_timestamp_us: recv_ts,
        exchange_timestamp_us: recv_ts + 100,
        source: DataSource::WebSocket,
        source_event_id: Some("evt-1".into()),
        source_session_id: Some("sess-1".into()),
        sequence: Some(Sequence::new(seq)),
        ingest_ordinal: None,
    }
}

fn make_snapshot_event(recv_ts: u64, side: Side, price: u32, size: u64, seq: u64) -> BookEvent {
    BookEvent {
        asset_id: test_asset_id(),
        kind: BookEventKind::Snapshot,
        side,
        price: FixedPrice::new(price).unwrap(),
        size: FixedSize::new(size),
        provenance: test_provenance(recv_ts, seq),
    }
}

fn make_delta_event(recv_ts: u64, side: Side, price: u32, size: u64, seq: u64) -> BookEvent {
    BookEvent {
        asset_id: test_asset_id(),
        kind: BookEventKind::Delta,
        side,
        price: FixedPrice::new(price).unwrap(),
        size: FixedSize::new(size),
        provenance: test_provenance(recv_ts, seq),
    }
}

fn make_checkpoint(
    checkpoint_ts: u64,
    bids: Vec<(u32, u64)>,
    asks: Vec<(u32, u64)>,
) -> BookCheckpoint {
    BookCheckpoint {
        asset_id: test_asset_id(),
        checkpoint_timestamp_us: checkpoint_ts,
        provenance: EventProvenance {
            recv_timestamp_us: checkpoint_ts,
            exchange_timestamp_us: checkpoint_ts + 100,
            source: DataSource::RestSnapshot,
            source_event_id: None,
            source_session_id: None,
            sequence: None,
            ingest_ordinal: None,
        },
        bids: bids
            .into_iter()
            .map(|(p, s)| PriceLevel {
                price: FixedPrice::new(p).unwrap(),
                size: FixedSize::new(s),
            })
            .collect(),
        asks: asks
            .into_iter()
            .map(|(p, s)| PriceLevel {
                price: FixedPrice::new(p).unwrap(),
                size: FixedSize::new(s),
            })
            .collect(),
        wal_offset: None,
    }
}

/// Mock EventReader for unit testing the ReplayEngine without Parquet.
struct MockReader {
    market_data: MarketDataWindow,
    checkpoints: Vec<BookCheckpoint>,
    latest_checkpoint: Option<BookCheckpoint>,
    validations: Vec<ReplayValidation>,
    execution_events: Vec<ExecutionEvent>,
    market_data_calls: Arc<Mutex<Vec<(u64, u64)>>>,
}

impl MockReader {
    fn new() -> Self {
        Self {
            market_data: MarketDataWindow::default(),
            checkpoints: vec![],
            latest_checkpoint: None,
            validations: vec![],
            execution_events: vec![],
            market_data_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_market_data(mut self, data: MarketDataWindow) -> Self {
        self.market_data = data;
        self
    }

    fn with_latest_checkpoint(mut self, cp: Option<BookCheckpoint>) -> Self {
        self.latest_checkpoint = cp;
        self
    }

    fn with_checkpoints(mut self, cps: Vec<BookCheckpoint>) -> Self {
        self.checkpoints = cps;
        self
    }

    fn with_call_log(mut self, calls: Arc<Mutex<Vec<(u64, u64)>>>) -> Self {
        self.market_data_calls = calls;
        self
    }
}

impl EventReader for MockReader {
    async fn read_market_data(
        &self,
        _asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<MarketDataWindow, ReplayError> {
        self.market_data_calls
            .lock()
            .unwrap()
            .push((start_us, end_us));
        Ok(self.market_data.clone())
    }

    async fn read_checkpoints(
        &self,
        _asset_id: &AssetId,
        _start_us: u64,
        _end_us: u64,
    ) -> Result<Vec<BookCheckpoint>, ReplayError> {
        Ok(self.checkpoints.clone())
    }

    async fn read_latest_checkpoint(
        &self,
        _asset_id: &AssetId,
        _at_us: u64,
    ) -> Result<Option<BookCheckpoint>, ReplayError> {
        Ok(self.latest_checkpoint.clone())
    }

    async fn read_validations(
        &self,
        _asset_id: &AssetId,
        _start_us: u64,
        _end_us: u64,
    ) -> Result<Vec<ReplayValidation>, ReplayError> {
        Ok(self.validations.clone())
    }

    async fn read_execution_events(
        &self,
        _order_id: Option<&str>,
        _start_us: u64,
        _end_us: u64,
    ) -> Result<Vec<ExecutionEvent>, ReplayError> {
        Ok(self.execution_events.clone())
    }
}

fn source_reset_event(recv_ts: u64) -> pb_types::IngestEvent {
    pb_types::IngestEvent {
        asset_id: None,
        kind: pb_types::event::IngestEventKind::SourceReset,
        provenance: EventProvenance {
            recv_timestamp_us: recv_ts,
            exchange_timestamp_us: 0,
            source: DataSource::WebSocket,
            source_event_id: None,
            source_session_id: Some("sess-reset".into()),
            sequence: None,
            ingest_ordinal: None,
        },
        expected_sequence: None,
        observed_sequence: None,
        details: Some("reset".into()),
    }
}

// 2025-06-15 12:30:00 UTC in microseconds
const BASE_TS: u64 = 1_750_000_200_000_000;

// ---------------------------------------------------------------------------
// ReplayEngine tests with MockReader
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replay_engine_reconstruct_from_snapshot() {
    let snapshot_ts = BASE_TS;
    let target_ts = BASE_TS + 100_000;

    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(snapshot_ts, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(snapshot_ts, Side::Ask, 5100, 2_000_000, 1),
            make_delta_event(snapshot_ts + 50_000, Side::Bid, 4900, 500_000, 2),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), target_ts, ReplayMode::RecvTime)
        .await
        .unwrap();

    assert!(!result.used_checkpoint);
    assert_eq!(result.mode, ReplayMode::RecvTime);
    // Should have 2 bid levels and 1 ask level
    assert_eq!(result.book.bid_depth(), 2);
    assert_eq!(result.book.ask_depth(), 1);
}

#[tokio::test]
async fn reconstruct_flags_crossed_book_in_continuity() {
    // A snapshot with bid 0.60 and ask 0.50 reconstructs to a crossed book; the
    // engine must surface it as a continuity marker rather than silently
    // returning a crossed book.
    let snapshot_ts = BASE_TS;
    let target_ts = BASE_TS + 100_000;
    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(snapshot_ts, Side::Bid, 6000, 1_000_000, 1),
            make_snapshot_event(snapshot_ts, Side::Ask, 5000, 2_000_000, 1),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };
    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), target_ts, ReplayMode::RecvTime)
        .await
        .unwrap();

    assert!(
        result.continuity_events.iter().any(|e| e
            .details
            .as_deref()
            .map(|d| d.contains("crossed"))
            .unwrap_or(false)),
        "crossed book should be surfaced as a continuity event"
    );
}

#[tokio::test]
async fn replay_engine_reconstruct_from_checkpoint() {
    let checkpoint_ts = BASE_TS;
    let delta_ts = BASE_TS + 50_000;
    let target_ts = BASE_TS + 100_000;

    let checkpoint = make_checkpoint(
        checkpoint_ts,
        vec![(5000, 1_000_000)],
        vec![(5100, 2_000_000)],
    );

    let market_data = MarketDataWindow {
        book_events: vec![make_delta_event(delta_ts, Side::Bid, 4900, 500_000, 2)],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let reader = MockReader::new()
        .with_market_data(market_data)
        .with_latest_checkpoint(Some(checkpoint));
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), target_ts, ReplayMode::RecvTime)
        .await
        .unwrap();

    assert!(result.used_checkpoint);
    assert_eq!(result.book.bid_depth(), 2);
    assert_eq!(result.book.ask_depth(), 1);
}

#[tokio::test]
async fn replay_engine_uses_checkpoint_timestamp_as_market_data_floor() {
    let checkpoint_ts = BASE_TS;
    let target_ts = BASE_TS + 100_000;
    let checkpoint = make_checkpoint(
        checkpoint_ts,
        vec![(5000, 1_000_000)],
        vec![(5100, 2_000_000)],
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let reader = MockReader::new()
        .with_call_log(calls.clone())
        .with_market_data(MarketDataWindow::default())
        .with_latest_checkpoint(Some(checkpoint));
    let engine = ReplayEngine::new(reader);

    let _ = engine
        .reconstruct_at(&test_asset_id(), target_ts, ReplayMode::RecvTime)
        .await;

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], (checkpoint_ts, target_ts));
}

#[tokio::test]
async fn replay_engine_ignores_checkpoint_older_than_lookback_floor() {
    let checkpoint_ts = BASE_TS;
    let target_ts = BASE_TS + 100_000;
    let lookback_us = 10_000;
    let checkpoint = make_checkpoint(
        checkpoint_ts,
        vec![(5000, 1_000_000)],
        vec![(5100, 2_000_000)],
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let reader = MockReader::new()
        .with_call_log(calls.clone())
        .with_market_data(MarketDataWindow::default())
        .with_latest_checkpoint(Some(checkpoint));
    let engine = ReplayEngine::new(reader).with_lookback_us(lookback_us);

    let _ = engine
        .reconstruct_at(&test_asset_id(), target_ts, ReplayMode::RecvTime)
        .await;

    let calls = calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], (target_ts - lookback_us, target_ts));
}

#[tokio::test]
async fn replay_engine_no_snapshot_returns_error() {
    let market_data = MarketDataWindow {
        book_events: vec![
            // Only deltas, no snapshot
            make_delta_event(BASE_TS, Side::Bid, 5000, 1_000_000, 1),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), BASE_TS + 100_000, ReplayMode::RecvTime)
        .await;

    assert!(result.is_err());
    match result {
        Err(ReplayError::NoSnapshotFound { .. }) => {}
        other => panic!("expected NoSnapshotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn replay_engine_does_not_stitch_across_source_reset_without_new_snapshot() {
    let snapshot_ts = BASE_TS;
    let reset_ts = BASE_TS + 50_000;
    let target_ts = BASE_TS + 100_000;
    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(snapshot_ts, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(snapshot_ts, Side::Ask, 5100, 2_000_000, 1),
            make_delta_event(target_ts, Side::Bid, 4900, 500_000, 2),
        ],
        trade_events: vec![],
        ingest_events: vec![source_reset_event(reset_ts)],
    };

    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), target_ts, ReplayMode::RecvTime)
        .await;

    assert!(matches!(result, Err(ReplayError::NoSnapshotFound { .. })));
}

#[tokio::test]
async fn replay_engine_uses_post_reset_snapshot_when_available() {
    let pre_reset_ts = BASE_TS;
    let reset_ts = BASE_TS + 50_000;
    let post_reset_snapshot_ts = BASE_TS + 60_000;
    let target_ts = BASE_TS + 100_000;
    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(pre_reset_ts, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(pre_reset_ts, Side::Ask, 5100, 2_000_000, 1),
            make_snapshot_event(post_reset_snapshot_ts, Side::Bid, 4900, 700_000, 1),
            make_snapshot_event(post_reset_snapshot_ts, Side::Ask, 5200, 900_000, 1),
            make_delta_event(target_ts, Side::Bid, 4800, 300_000, 2),
        ],
        trade_events: vec![],
        ingest_events: vec![source_reset_event(reset_ts)],
    };

    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), target_ts, ReplayMode::RecvTime)
        .await
        .unwrap();

    assert_eq!(result.book.bid_depth(), 2);
    assert_eq!(result.book.ask_depth(), 1);
    let (best_bid_price, _) = result.book.best_bid().unwrap();
    assert_eq!(best_bid_price, FixedPrice::new(4900).unwrap());
}

#[tokio::test]
async fn replay_engine_empty_window_returns_error() {
    let reader = MockReader::new();
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), BASE_TS, ReplayMode::RecvTime)
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn replay_engine_single_snapshot_produces_correct_book() {
    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(BASE_TS, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(BASE_TS, Side::Ask, 5100, 2_000_000, 1),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), BASE_TS + 1, ReplayMode::RecvTime)
        .await
        .unwrap();

    assert_eq!(result.book.bid_depth(), 1);
    assert_eq!(result.book.ask_depth(), 1);
    let (best_bid_price, best_bid_size) = result.book.best_bid().unwrap();
    assert_eq!(best_bid_price, FixedPrice::new(5000).unwrap());
    assert_eq!(best_bid_size, FixedSize::new(1_000_000));
}

#[tokio::test]
async fn replay_engine_deltas_applied_in_order() {
    let t0 = BASE_TS;
    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(t0, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(t0, Side::Ask, 5100, 2_000_000, 1),
            make_delta_event(t0 + 1000, Side::Bid, 5000, 1_500_000, 2),
            make_delta_event(t0 + 2000, Side::Bid, 4900, 800_000, 3),
            make_delta_event(t0 + 3000, Side::Ask, 5200, 300_000, 4),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), t0 + 10_000, ReplayMode::RecvTime)
        .await
        .unwrap();

    // After deltas: bid 5000=1.5M, bid 4900=0.8M, ask 5100=2M, ask 5200=0.3M
    assert_eq!(result.book.bid_depth(), 2);
    assert_eq!(result.book.ask_depth(), 2);
    let (best_bid_price, best_bid_size) = result.book.best_bid().unwrap();
    assert_eq!(best_bid_price, FixedPrice::new(5000).unwrap());
    assert_eq!(best_bid_size, FixedSize::new(1_500_000));
}

#[tokio::test]
async fn replay_engine_delta_removes_level_on_zero_size() {
    let t0 = BASE_TS;
    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(t0, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(t0, Side::Ask, 5100, 2_000_000, 1),
            // Zero size should remove the bid level
            make_delta_event(t0 + 1000, Side::Bid, 5000, 0, 2),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), t0 + 10_000, ReplayMode::RecvTime)
        .await
        .unwrap();

    assert_eq!(result.book.bid_depth(), 0);
    assert_eq!(result.book.ask_depth(), 1);
}

#[tokio::test]
async fn replay_engine_exchange_time_mode() {
    let t0 = BASE_TS;
    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(t0, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(t0, Side::Ask, 5100, 2_000_000, 1),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), t0 + 200, ReplayMode::ExchangeTime)
        .await
        .unwrap();

    assert_eq!(result.mode, ReplayMode::ExchangeTime);
    assert_eq!(result.book.bid_depth(), 1);
}

#[tokio::test]
async fn replay_engine_replay_window_returns_market_data() {
    let market_data = MarketDataWindow {
        book_events: vec![make_snapshot_event(BASE_TS, Side::Bid, 5000, 1_000_000, 1)],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let reader = MockReader::new().with_market_data(market_data.clone());
    let engine = ReplayEngine::new(reader);

    let window = engine
        .replay_window(&test_asset_id(), BASE_TS - 1000, BASE_TS + 1000)
        .await
        .unwrap();

    assert_eq!(window.book_events.len(), 1);
}

#[tokio::test]
async fn replay_engine_validate_matching_checkpoint() {
    let t0 = BASE_TS;
    // Snapshot produces a book that matches the checkpoint
    let snapshot_ts = t0;
    let checkpoint_ts = t0 + 1000;

    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(snapshot_ts, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(snapshot_ts, Side::Ask, 5100, 2_000_000, 1),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let checkpoint = make_checkpoint(
        checkpoint_ts,
        vec![(5000, 1_000_000)],
        vec![(5100, 2_000_000)],
    );

    let reader = MockReader::new()
        .with_market_data(market_data)
        .with_checkpoints(vec![checkpoint]);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .validate_at(&test_asset_id(), t0, ReplayMode::RecvTime)
        .await
        .unwrap();

    let validation = result.unwrap();
    assert!(validation.matched);
    assert!(validation.mismatch_summary.is_none());
}

#[tokio::test]
async fn replay_engine_validate_detects_divergence() {
    // Regression test for the vacuous-validation bug: the reconstructed
    // book must be compared against an INDEPENDENT reference checkpoint, so a
    // replay that diverges from the reference is reported as a mismatch. Under
    // the old code (which seeded reconstruction from the reference checkpoint
    // itself) `matched` was always true and this test would fail.
    let t0 = BASE_TS;
    let snapshot_ts = t0;
    let checkpoint_ts = t0 + 1000;

    // Replayed market data yields bid size 1_000_000 at 5000.
    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(snapshot_ts, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(snapshot_ts, Side::Ask, 5100, 2_000_000, 1),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };

    // Reference checkpoint disagrees: bid size 2_000_000 at 5000.
    let checkpoint = make_checkpoint(
        checkpoint_ts,
        vec![(5000, 2_000_000)],
        vec![(5100, 2_000_000)],
    );

    let reader = MockReader::new()
        .with_market_data(market_data)
        .with_checkpoints(vec![checkpoint]);
    let engine = ReplayEngine::new(reader);

    let validation = engine
        .validate_at(&test_asset_id(), t0, ReplayMode::RecvTime)
        .await
        .unwrap()
        .unwrap();

    assert!(
        !validation.matched,
        "a replay that diverges from the reference checkpoint must not match"
    );
    assert!(validation.mismatch_summary.is_some());
}

#[tokio::test]
async fn replay_engine_validate_no_future_checkpoint_returns_none() {
    let reader = MockReader::new()
        .with_market_data(MarketDataWindow::default())
        .with_checkpoints(vec![]);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .validate_at(&test_asset_id(), BASE_TS, ReplayMode::RecvTime)
        .await
        .unwrap();

    assert!(result.is_none());
}

#[tokio::test]
async fn replay_engine_with_custom_lookback() {
    let market_data = MarketDataWindow {
        book_events: vec![
            make_snapshot_event(BASE_TS, Side::Bid, 5000, 1_000_000, 1),
            make_snapshot_event(BASE_TS, Side::Ask, 5100, 2_000_000, 1),
        ],
        trade_events: vec![],
        ingest_events: vec![],
    };

    let reader = MockReader::new().with_market_data(market_data);
    let engine = ReplayEngine::new(reader).with_lookback_us(60_000_000); // 60s

    let result = engine
        .reconstruct_at(&test_asset_id(), BASE_TS + 1000, ReplayMode::RecvTime)
        .await
        .unwrap();

    assert_eq!(result.book.bid_depth(), 1);
}

// ---------------------------------------------------------------------------
// ParquetReader hour_paths tests
// ---------------------------------------------------------------------------

#[test]
fn hour_paths_single_hour() {
    let reader = ParquetReader::new("/data");
    // 1 hour range within the same hour
    let start = 1_750_000_200_000_000u64; // 2025-06-15 12:xx:xx
    let end = start + 100_000; // still in same hour

    let paths = reader.hour_paths("book_events", start, end);
    assert_eq!(paths.len(), 1);
    let path_str = paths[0].to_str().unwrap();
    assert!(path_str.contains("book_events"));
}

#[test]
fn hour_paths_multi_hour() {
    let reader = ParquetReader::new("/data");
    // span 3 hours
    let start = 1_750_000_200_000_000u64;
    let end = start + 3 * 3_600_000_000u64;

    let paths = reader.hour_paths("book_events", start, end);
    assert!(
        paths.len() >= 3,
        "should cover at least 3 hours, got {}",
        paths.len()
    );
}

#[test]
fn hour_paths_midnight_crossing() {
    let reader = ParquetReader::new("/data");
    // 2025-06-15 23:30:00 UTC to 2025-06-16 00:30:00 UTC
    let start_23 = 1_750_030_200_000_000u64; // 23:30
    let end_00 = start_23 + 3_600_000_000u64; // +1 hour = next day 00:30

    let paths = reader.hour_paths("book_events", start_23, end_00);
    assert!(paths.len() >= 2, "should cross midnight boundary");

    // Verify the paths contain different day patterns
    let path_strs: Vec<String> = paths
        .iter()
        .map(|p| p.to_str().unwrap().to_string())
        .collect();
    // At least one path should contain /23 (hour 23)
    assert!(
        path_strs.iter().any(|p| p.contains("/23")),
        "should have hour 23 path: {:?}",
        path_strs
    );
}

#[test]
fn hour_paths_empty_range_returns_single() {
    let reader = ParquetReader::new("/data");
    let ts = 1_750_000_200_000_000u64;
    let paths = reader.hour_paths("book_events", ts, ts);
    assert_eq!(paths.len(), 1, "zero-length range should produce one path");
}

// ---------------------------------------------------------------------------
// Parquet write-then-read integration tests
// ---------------------------------------------------------------------------

/// Helper: write records to parquet files using pb-store, then read them
/// back with pb-replay's ParquetReader.
async fn write_parquet_records(base_path: &std::path::Path, records: &[pb_types::PersistedRecord]) {
    use object_store::local::LocalFileSystem;
    use pb_store::writer::ParquetRecordWriter;

    let store = Arc::new(LocalFileSystem::new_with_prefix(base_path).unwrap());
    let writer = ParquetRecordWriter::new(store, "");
    writer.write_batch(records).await.unwrap();
}

#[tokio::test]
async fn parquet_reader_reads_book_events() {
    let dir = TempDir::new().unwrap();
    let base_path = dir.path();

    let t0 = BASE_TS;
    let book = BookEvent {
        asset_id: test_asset_id(),
        kind: BookEventKind::Snapshot,
        side: Side::Bid,
        price: FixedPrice::new(5000).unwrap(),
        size: FixedSize::new(1_000_000),
        provenance: test_provenance(t0, 1),
    };
    write_parquet_records(base_path, &[pb_types::PersistedRecord::Book(book.clone())]).await;

    let reader = ParquetReader::new(base_path);
    let window = reader
        .read_market_data(&test_asset_id(), t0 - 1000, t0 + 1000)
        .await
        .unwrap();

    assert_eq!(window.book_events.len(), 1);
    assert_eq!(window.book_events[0].asset_id, test_asset_id());
    assert_eq!(window.book_events[0].kind, BookEventKind::Snapshot);
    assert_eq!(window.book_events[0].side, Side::Bid);
    assert_eq!(window.book_events[0].price, FixedPrice::new(5000).unwrap());
}

#[tokio::test]
async fn parquet_reader_preserves_ingest_ordinal() {
    // The ingest ordinal must survive the Parquet write→read round-trip so replay
    // can use it as the authoritative arrival-order tiebreaker.
    let dir = TempDir::new().unwrap();
    let base_path = dir.path();
    let t0 = BASE_TS;

    let mut prov = test_provenance(t0, 1);
    prov.ingest_ordinal = Some(987_654);
    let book = BookEvent {
        asset_id: test_asset_id(),
        kind: BookEventKind::Snapshot,
        side: Side::Bid,
        price: FixedPrice::new(5000).unwrap(),
        size: FixedSize::new(1_000_000),
        provenance: prov,
    };
    write_parquet_records(base_path, &[pb_types::PersistedRecord::Book(book)]).await;

    let reader = ParquetReader::new(base_path);
    let window = reader
        .read_market_data(&test_asset_id(), t0 - 1000, t0 + 1000)
        .await
        .unwrap();

    assert_eq!(window.book_events.len(), 1);
    assert_eq!(
        window.book_events[0].provenance.ingest_ordinal,
        Some(987_654),
        "ingest_ordinal must round-trip through Parquet"
    );
}

#[tokio::test]
async fn parquet_reader_reads_trade_events() {
    let dir = TempDir::new().unwrap();
    let base_path = dir.path();

    let t0 = BASE_TS;
    let trade = TradeEvent {
        asset_id: test_asset_id(),
        price: FixedPrice::new(5050).unwrap(),
        size: Some(FixedSize::new(500_000)),
        side: Some(Side::Ask),
        trade_id: Some("trade-001".into()),
        fidelity: TradeFidelity::Full,
        provenance: test_provenance(t0, 1),
    };
    write_parquet_records(base_path, &[pb_types::PersistedRecord::Trade(trade)]).await;

    let reader = ParquetReader::new(base_path);
    let window = reader
        .read_market_data(&test_asset_id(), t0 - 1000, t0 + 1000)
        .await
        .unwrap();

    assert_eq!(window.trade_events.len(), 1);
    assert_eq!(window.trade_events[0].price, FixedPrice::new(5050).unwrap());
    assert_eq!(window.trade_events[0].fidelity, TradeFidelity::Full);
}

#[tokio::test]
async fn parquet_reader_reads_checkpoints() {
    let dir = TempDir::new().unwrap();
    let base_path = dir.path();

    let t0 = BASE_TS;
    let checkpoint = make_checkpoint(t0, vec![(5000, 1_000_000)], vec![(5100, 2_000_000)]);
    write_parquet_records(
        base_path,
        &[pb_types::PersistedRecord::Checkpoint(checkpoint.clone())],
    )
    .await;

    let reader = ParquetReader::new(base_path);
    let checkpoints = reader
        .read_checkpoints(&test_asset_id(), t0 - 1000, t0 + 1000)
        .await
        .unwrap();

    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].bids.len(), 1);
    assert_eq!(checkpoints[0].asks.len(), 1);
    assert_eq!(checkpoints[0].bids[0].price, FixedPrice::new(5000).unwrap());
}

#[tokio::test]
async fn parquet_reader_reads_latest_checkpoint() {
    let dir = TempDir::new().unwrap();
    let base_path = dir.path();

    let t0 = BASE_TS;
    let cp1 = make_checkpoint(t0, vec![(5000, 1_000_000)], vec![(5100, 2_000_000)]);
    let cp2 = make_checkpoint(t0 + 1000, vec![(5050, 1_500_000)], vec![(5150, 2_500_000)]);
    write_parquet_records(
        base_path,
        &[
            pb_types::PersistedRecord::Checkpoint(cp1),
            pb_types::PersistedRecord::Checkpoint(cp2.clone()),
        ],
    )
    .await;

    let reader = ParquetReader::new(base_path);
    let latest = reader
        .read_latest_checkpoint(&test_asset_id(), t0 + 2000)
        .await
        .unwrap();

    assert!(latest.is_some());
    let latest = latest.unwrap();
    assert_eq!(latest.checkpoint_timestamp_us, t0 + 1000);
}

#[tokio::test]
async fn parquet_reader_missing_directory_returns_empty() {
    let dir = TempDir::new().unwrap();
    let reader = ParquetReader::new(dir.path().join("nonexistent"));

    let window = reader
        .read_market_data(&test_asset_id(), BASE_TS - 1000, BASE_TS + 1000)
        .await
        .unwrap();

    assert!(window.book_events.is_empty());
    assert!(window.trade_events.is_empty());
    assert!(window.ingest_events.is_empty());
}

#[tokio::test]
async fn parquet_reader_filters_by_time_range() {
    let dir = TempDir::new().unwrap();
    let base_path = dir.path();

    let t0 = BASE_TS;
    // Two events at different times
    let records = vec![
        pb_types::PersistedRecord::Book(make_snapshot_event(t0, Side::Bid, 5000, 1_000_000, 1)),
        pb_types::PersistedRecord::Book(make_snapshot_event(
            t0 + 100_000,
            Side::Ask,
            5100,
            2_000_000,
            2,
        )),
    ];
    write_parquet_records(base_path, &records).await;

    let reader = ParquetReader::new(base_path);
    // Query only the first event's time range
    let window = reader
        .read_market_data(&test_asset_id(), t0, t0 + 1)
        .await
        .unwrap();

    assert_eq!(window.book_events.len(), 1);
    assert_eq!(window.book_events[0].side, Side::Bid);
}

#[tokio::test]
async fn parquet_reader_filters_by_asset_id() {
    let dir = TempDir::new().unwrap();
    let base_path = dir.path();

    let t0 = BASE_TS;
    let other_asset = AssetId::new("ETH-5M-NO");
    let book1 = BookEvent {
        asset_id: test_asset_id(),
        kind: BookEventKind::Snapshot,
        side: Side::Bid,
        price: FixedPrice::new(5000).unwrap(),
        size: FixedSize::new(1_000_000),
        provenance: test_provenance(t0, 1),
    };
    let book2 = BookEvent {
        asset_id: other_asset.clone(),
        kind: BookEventKind::Snapshot,
        side: Side::Bid,
        price: FixedPrice::new(3000).unwrap(),
        size: FixedSize::new(500_000),
        provenance: test_provenance(t0, 1),
    };

    // Write each separately (different asset partition)
    write_parquet_records(base_path, &[pb_types::PersistedRecord::Book(book1)]).await;
    write_parquet_records(base_path, &[pb_types::PersistedRecord::Book(book2)]).await;

    let reader = ParquetReader::new(base_path);
    let window = reader
        .read_market_data(&test_asset_id(), t0 - 1000, t0 + 1000)
        .await
        .unwrap();

    assert_eq!(window.book_events.len(), 1);
    assert_eq!(window.book_events[0].asset_id, test_asset_id());
}

// ---------------------------------------------------------------------------
// End-to-end: write via pb-store, reconstruct via ReplayEngine+ParquetReader
// ---------------------------------------------------------------------------

#[tokio::test]
async fn end_to_end_write_and_reconstruct() {
    let dir = TempDir::new().unwrap();
    let base_path = dir.path();

    let t0 = BASE_TS;
    let records = vec![
        pb_types::PersistedRecord::Book(make_snapshot_event(t0, Side::Bid, 5000, 1_000_000, 1)),
        pb_types::PersistedRecord::Book(make_snapshot_event(t0, Side::Ask, 5100, 2_000_000, 1)),
        pb_types::PersistedRecord::Book(make_delta_event(t0 + 1000, Side::Bid, 4900, 500_000, 2)),
    ];
    write_parquet_records(base_path, &records).await;

    let reader = ParquetReader::new(base_path);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&test_asset_id(), t0 + 2000, ReplayMode::RecvTime)
        .await
        .unwrap();

    assert_eq!(result.book.bid_depth(), 2);
    assert_eq!(result.book.ask_depth(), 1);
    assert!(!result.used_checkpoint);
}

// ---------------------------------------------------------------------------
// Backfill unit tests
// ---------------------------------------------------------------------------

#[test]
fn checkpoint_from_rest_parses_correctly() {
    use crate::backfill::checkpoint_from_rest;
    use pb_types::wire::RestBookResponse;

    let response = RestBookResponse {
        market: None,
        asset_id: "BTC-5M-YES".into(),
        hash: Some("hash-123".into()),
        timestamp: Some("1750000200000".into()), // milliseconds
        bids: vec![pb_types::wire::RestOrderEntry {
            price: "0.5000".into(),
            size: "1.000000".into(),
        }],
        asks: vec![pb_types::wire::RestOrderEntry {
            price: "0.5100".into(),
            size: "2.000000".into(),
        }],
        tick_size: None,
        min_order_size: None,
        neg_risk: None,
        last_trade_price: None,
    };

    let checkpoint = checkpoint_from_rest(&response).unwrap();
    assert_eq!(checkpoint.asset_id, AssetId::new("BTC-5M-YES"));
    assert_eq!(checkpoint.bids.len(), 1);
    assert_eq!(checkpoint.asks.len(), 1);
    assert_eq!(checkpoint.bids[0].price, FixedPrice::new(5000).unwrap());
    assert_eq!(checkpoint.asks[0].price, FixedPrice::new(5100).unwrap());
}

#[test]
fn parse_timestamp_us_milliseconds() {
    // Test the private function indirectly through checkpoint_from_rest
    use crate::backfill::checkpoint_from_rest;
    use pb_types::wire::RestBookResponse;

    let response = RestBookResponse {
        market: None,
        asset_id: "TEST".into(),
        hash: None,
        timestamp: Some("1750000200000".into()), // ms < 10^13 => multiply by 1000
        bids: vec![],
        asks: vec![],
        tick_size: None,
        min_order_size: None,
        neg_risk: None,
        last_trade_price: None,
    };

    let cp = checkpoint_from_rest(&response).unwrap();
    // Should be converted to microseconds
    assert_eq!(cp.checkpoint_timestamp_us, 1_750_000_200_000_000);
}

#[test]
fn parse_timestamp_us_microseconds() {
    use crate::backfill::checkpoint_from_rest;
    use pb_types::wire::RestBookResponse;

    let response = RestBookResponse {
        market: None,
        asset_id: "TEST".into(),
        hash: None,
        timestamp: Some("1750000200000000".into()), // us >= 10^13 => keep as-is
        bids: vec![],
        asks: vec![],
        tick_size: None,
        min_order_size: None,
        neg_risk: None,
        last_trade_price: None,
    };

    let cp = checkpoint_from_rest(&response).unwrap();
    assert_eq!(cp.checkpoint_timestamp_us, 1_750_000_200_000_000);
}

// ---------------------------------------------------------------------------
// Golden replay determinism regression
// ---------------------------------------------------------------------------

/// A fixed, deterministic set of book events written to Parquet, with explicit
/// ingest ordinals — including a same-microsecond pre-snapshot delta that must
/// sort before its snapshot. Replaying this fixture must always produce
/// the same book; a book-logic change that alters the output will fail this test.
fn golden_book_records() -> Vec<pb_types::PersistedRecord> {
    fn ev(
        kind: BookEventKind,
        side: Side,
        price: u32,
        size: u64,
        recv_ts: u64,
        seq: u64,
        ordinal: u64,
    ) -> pb_types::PersistedRecord {
        let mut prov = test_provenance(recv_ts, seq);
        prov.ingest_ordinal = Some(ordinal);
        pb_types::PersistedRecord::Book(BookEvent {
            asset_id: test_asset_id(),
            kind,
            side,
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::new(size),
            provenance: prov,
        })
    }
    let t = BASE_TS;
    vec![
        // Initial snapshot at t.
        ev(BookEventKind::Snapshot, Side::Bid, 5000, 100, t, 0, 0),
        ev(BookEventKind::Snapshot, Side::Bid, 4900, 80, t, 0, 1),
        ev(BookEventKind::Snapshot, Side::Ask, 5100, 200, t, 0, 2),
        // A delta that arrived BEFORE a re-snapshot at the same microsecond
        // (ordinal 3 < 4): it must be applied first, then overwritten by the
        // snapshot — i.e. it must NOT win the tie.
        ev(BookEventKind::Delta, Side::Bid, 5000, 999, t + 10, 7, 3),
        ev(BookEventKind::Snapshot, Side::Bid, 5000, 110, t + 10, 0, 4),
        ev(BookEventKind::Snapshot, Side::Bid, 4900, 80, t + 10, 0, 5),
        ev(BookEventKind::Snapshot, Side::Ask, 5100, 200, t + 10, 0, 6),
        // Post-snapshot deltas.
        ev(BookEventKind::Delta, Side::Bid, 5000, 130, t + 20, 1, 7),
        ev(BookEventKind::Delta, Side::Ask, 5300, 150, t + 30, 2, 8),
    ]
}

async fn replay_golden(base_path: &std::path::Path) -> crate::engine::ReplayResult {
    let reader = ParquetReader::new(base_path);
    let engine = ReplayEngine::new(reader);
    engine
        .reconstruct_at(&test_asset_id(), BASE_TS + 1_000_000, ReplayMode::RecvTime)
        .await
        .unwrap()
}

#[tokio::test]
async fn golden_replay_produces_expected_book() {
    let dir = TempDir::new().unwrap();
    write_parquet_records(dir.path(), &golden_book_records()).await;

    let result = replay_golden(dir.path()).await;

    // Expected final book after the re-snapshot at t+10 and the two later deltas:
    //   bids: 5000=130 (delta @t+20 over snapshot 110), 4900=80
    //   asks: 5100=200, 5300=150
    // The stale pre-snapshot delta (5000=999) must have been overwritten.
    assert_eq!(result.book.bid_depth(), 2, "bids");
    assert_eq!(result.book.ask_depth(), 2, "asks");
    assert_eq!(
        result.book.best_bid(),
        Some((FixedPrice::new(5000).unwrap(), FixedSize::new(130)))
    );
    assert_eq!(
        result.book.best_ask(),
        Some((FixedPrice::new(5100).unwrap(), FixedSize::new(200)))
    );
}

#[tokio::test]
async fn golden_replay_is_deterministic_across_runs_and_input_order() {
    let records = golden_book_records();

    // Run 1: canonical order.
    let dir1 = TempDir::new().unwrap();
    write_parquet_records(dir1.path(), &records).await;
    let r1 = replay_golden(dir1.path()).await;

    // Run 2: same input, fresh store — must match run 1 exactly.
    let dir2 = TempDir::new().unwrap();
    write_parquet_records(dir2.path(), &records).await;
    let r2 = replay_golden(dir2.path()).await;

    // Run 3: reversed write order — the deterministic total order must
    // still yield byte-identical book state.
    let mut reversed = records.clone();
    reversed.reverse();
    let dir3 = TempDir::new().unwrap();
    write_parquet_records(dir3.path(), &reversed).await;
    let r3 = replay_golden(dir3.path()).await;

    let fingerprint = |r: &crate::engine::ReplayResult| {
        (
            r.book.bid_depth(),
            r.book.ask_depth(),
            r.book.best_bid(),
            r.book.best_ask(),
            r.book.sequence.raw(),
        )
    };
    assert_eq!(fingerprint(&r1), fingerprint(&r2), "run-to-run determinism");
    assert_eq!(
        fingerprint(&r1),
        fingerprint(&r3),
        "input-order independence"
    );
}
