//! Cross-backend integration tests verifying Parquet and ClickHouse service
//! implementations return equivalent results for identical input data.
//!
//! Run with: `cargo test -p pb-integration-tests --test cross_backend_service -- --ignored`
//! Requires Docker.

use std::sync::Arc;

use object_store::ObjectStore;
use pb_service::{
    ClickHouseExecutionService, ClickHouseIntegrityService, ClickHouseReplayService,
    ExecutionService, IntegrityService, ParquetExecutionService, ParquetIntegrityService,
    ParquetReplayService, ReplayService,
};
use pb_store::{ClickHouseRecordWriter, ParquetRecordWriter};
use pb_types::event::{
    BookEvent, BookEventKind, DataSource, EventProvenance, ExecutionEvent, ExecutionEventKind,
    IngestEvent, IngestEventKind, LatencyTrace, PersistedRecord, ReplayMode, Side,
};
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::clickhouse::ClickHouse;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn provenance(recv_us: u64, exchange_us: u64, seq: Option<u64>) -> EventProvenance {
    EventProvenance {
        recv_timestamp_us: recv_us,
        exchange_timestamp_us: exchange_us,
        source: DataSource::WebSocket,
        source_event_id: None,
        source_session_id: Some("session-cross".to_string()),
        sequence: seq.map(Sequence::new),
        ingest_ordinal: None,
    }
}

/// Shared test data: book events forming a two-level book with a subsequent delta.
fn book_event_records(asset_id: &str, base_ts: u64) -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(asset_id);
    vec![
        // Snapshot bid level 1
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            provenance: provenance(base_ts, base_ts, Some(0)),
        }),
        // Snapshot bid level 2
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::new(4900).unwrap(),
            size: FixedSize::from_f64(50.0).unwrap(),
            provenance: provenance(base_ts, base_ts, Some(1)),
        }),
        // Snapshot ask level 1
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Ask,
            price: FixedPrice::new(5500).unwrap(),
            size: FixedSize::from_f64(80.0).unwrap(),
            provenance: provenance(base_ts, base_ts, Some(2)),
        }),
        // Snapshot ask level 2
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Ask,
            price: FixedPrice::new(5600).unwrap(),
            size: FixedSize::from_f64(60.0).unwrap(),
            provenance: provenance(base_ts, base_ts, Some(3)),
        }),
        // Delta: update bid level 1 size
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Delta,
            side: Side::Bid,
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::from_f64(120.0).unwrap(),
            provenance: provenance(base_ts + 1_000, base_ts + 1_000, Some(4)),
        }),
    ]
}

/// Ingest events for integrity testing: a reconnect and a gap.
fn ingest_event_records(asset_id: &str, base_ts: u64) -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(asset_id);
    vec![
        PersistedRecord::Ingest(IngestEvent {
            asset_id: Some(asset_id.clone()),
            kind: IngestEventKind::ReconnectSuccess,
            provenance: provenance(base_ts + 2_000, base_ts + 2_000, None),
            expected_sequence: None,
            observed_sequence: None,
            details: Some("reconnected".to_string()),
        }),
        PersistedRecord::Ingest(IngestEvent {
            asset_id: Some(asset_id),
            kind: IngestEventKind::SequenceGap,
            provenance: provenance(base_ts + 3_000, base_ts + 3_000, None),
            expected_sequence: Some(5),
            observed_sequence: Some(8),
            details: Some("gap detected".to_string()),
        }),
    ]
}

/// Execution events: submit, ack, fill for one order plus a second order's submit.
fn execution_event_records(asset_id: &str, base_ts: u64) -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(asset_id);
    vec![
        PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: base_ts,
            asset_id: Some(asset_id.clone()),
            order_id: "order-1".to_string(),
            client_order_id: Some("client-1".to_string()),
            venue_order_id: None,
            kind: ExecutionEventKind::SubmitIntent,
            side: Some(Side::Bid),
            price: Some(FixedPrice::new(5050).unwrap()),
            size: Some(FixedSize::from_f64(10.0).unwrap()),
            status: None,
            reason: None,
            latency: LatencyTrace::default(),
        }),
        PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: base_ts + 100,
            asset_id: Some(asset_id.clone()),
            order_id: "order-1".to_string(),
            client_order_id: Some("client-1".to_string()),
            venue_order_id: Some("venue-1".to_string()),
            kind: ExecutionEventKind::ExchangeAck,
            side: Some(Side::Bid),
            price: Some(FixedPrice::new(5050).unwrap()),
            size: Some(FixedSize::from_f64(10.0).unwrap()),
            status: Some("accepted".to_string()),
            reason: None,
            latency: LatencyTrace::default(),
        }),
        PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: base_ts + 500,
            asset_id: Some(asset_id.clone()),
            order_id: "order-1".to_string(),
            client_order_id: Some("client-1".to_string()),
            venue_order_id: Some("venue-1".to_string()),
            kind: ExecutionEventKind::Fill,
            side: Some(Side::Bid),
            price: Some(FixedPrice::new(5050).unwrap()),
            size: Some(FixedSize::from_f64(10.0).unwrap()),
            status: Some("filled".to_string()),
            reason: None,
            latency: LatencyTrace::default(),
        }),
        PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: base_ts + 600,
            asset_id: Some(asset_id),
            order_id: "order-2".to_string(),
            client_order_id: Some("client-2".to_string()),
            venue_order_id: None,
            kind: ExecutionEventKind::SubmitIntent,
            side: Some(Side::Ask),
            price: Some(FixedPrice::new(5500).unwrap()),
            size: Some(FixedSize::from_f64(5.0).unwrap()),
            status: None,
            reason: None,
            latency: LatencyTrace::default(),
        }),
    ]
}

async fn setup_clickhouse() -> (
    testcontainers::ContainerAsync<ClickHouse>,
    clickhouse::Client,
    String,
    String,
) {
    let container = ClickHouse::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(8123).await.unwrap();
    let url = format!("http://127.0.0.1:{port}");

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_name = format!("test_{nanos}");

    let client = clickhouse::Client::default().with_url(&url);
    client
        .query(&format!("CREATE DATABASE {db_name}"))
        .execute()
        .await
        .unwrap();

    (container, client.with_database(&db_name), url, db_name)
}

fn setup_parquet() -> (tempfile::TempDir, String) {
    let tmp_dir = tempfile::tempdir().unwrap();
    let base_path = tmp_dir.path().to_string_lossy().to_string();
    (tmp_dir, base_path)
}

async fn write_to_parquet(base_path: &str, records: &[PersistedRecord]) {
    let store = Arc::new(object_store::local::LocalFileSystem::new()) as Arc<dyn ObjectStore>;
    let writer = ParquetRecordWriter::new(store, base_path.to_string());
    writer.write_batch(records).await.unwrap();
}

async fn write_to_clickhouse(client: clickhouse::Client, records: &[PersistedRecord]) {
    let writer = ClickHouseRecordWriter::new(client);
    writer.ensure_tables().await.unwrap();
    writer.write_batch(records).await.unwrap();
}

// ---------------------------------------------------------------------------
// Test 1: Replay equivalence
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn cross_backend_replay_equivalence() {
    let base_ts = 1_700_000_000_000_000u64;
    let asset_str = "cross-replay-asset";
    let asset_id = AssetId::new(asset_str);
    let records = book_event_records(asset_str, base_ts);

    // Write identical data to both backends.
    let (_tmp_dir, parquet_path) = setup_parquet();
    write_to_parquet(&parquet_path, &records).await;

    let (_container, ch_client, ch_url, ch_db) = setup_clickhouse().await;
    write_to_clickhouse(ch_client, &records).await;

    // Query via service traits.
    let pq_service = ParquetReplayService::new(&parquet_path);
    let ch_service = ClickHouseReplayService::new(&ch_url, &ch_db);

    let replay_at = base_ts + 1_000; // after the delta
    let pq_result = pq_service
        .reconstruct(&asset_id, replay_at, ReplayMode::RecvTime, None)
        .await
        .unwrap();
    let ch_result = ch_service
        .reconstruct(&asset_id, replay_at, ReplayMode::RecvTime, None)
        .await
        .unwrap();

    // Assert equivalent book state.
    assert_eq!(pq_result.best_bid, ch_result.best_bid, "best_bid mismatch");
    assert_eq!(pq_result.best_ask, ch_result.best_ask, "best_ask mismatch");
    assert_eq!(
        pq_result.mid_price, ch_result.mid_price,
        "mid_price mismatch"
    );
    assert_eq!(pq_result.spread, ch_result.spread, "spread mismatch");
    assert_eq!(
        pq_result.bid_depth, ch_result.bid_depth,
        "bid_depth mismatch"
    );
    assert_eq!(
        pq_result.ask_depth, ch_result.ask_depth,
        "ask_depth mismatch"
    );
    assert_eq!(pq_result.bids, ch_result.bids, "bids mismatch");
    assert_eq!(pq_result.asks, ch_result.asks, "asks mismatch");

    // Sanity-check the book is non-trivial.
    assert_eq!(pq_result.bid_depth, 2);
    assert_eq!(pq_result.ask_depth, 2);
    assert!(pq_result.best_bid.is_some());
    assert!(pq_result.best_ask.is_some());
}

// ---------------------------------------------------------------------------
// Test 2: Integrity equivalence
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn cross_backend_integrity_equivalence() {
    let base_ts = 1_700_000_100_000_000u64;
    let asset_str = "cross-integrity-asset";
    let asset_id = AssetId::new(asset_str);

    let mut records = book_event_records(asset_str, base_ts);
    records.extend(ingest_event_records(asset_str, base_ts));

    // Write identical data to both backends.
    let (_tmp_dir, parquet_path) = setup_parquet();
    write_to_parquet(&parquet_path, &records).await;

    let (_container, ch_client, ch_url, ch_db) = setup_clickhouse().await;
    write_to_clickhouse(ch_client, &records).await;

    // Query via service traits.
    let pq_service = ParquetIntegrityService::new(&parquet_path);
    let ch_service = ClickHouseIntegrityService::new(&ch_url, &ch_db);

    let start_us = base_ts;
    let end_us = base_ts + 1_000_000;
    let pq_summary = pq_service
        .summary(&asset_id, start_us, end_us)
        .await
        .unwrap();
    let ch_summary = ch_service
        .summary(&asset_id, start_us, end_us)
        .await
        .unwrap();

    // Compare fields that both backends populate equivalently.
    assert_eq!(
        pq_summary.book_event_count, ch_summary.book_event_count,
        "book_event_count mismatch"
    );
    assert_eq!(
        pq_summary.ingest_event_count, ch_summary.ingest_event_count,
        "ingest_event_count mismatch"
    );
    assert_eq!(
        pq_summary.gap_count, ch_summary.gap_count,
        "gap_count mismatch"
    );
    assert_eq!(
        pq_summary.reconnect_count, ch_summary.reconnect_count,
        "reconnect_count mismatch"
    );
    assert_eq!(
        pq_summary.completeness, ch_summary.completeness,
        "completeness mismatch"
    );
    assert_eq!(
        pq_summary.validation_count, ch_summary.validation_count,
        "validation_count mismatch"
    );

    // Sanity-check the counts are non-trivial.
    assert_eq!(pq_summary.book_event_count, 5);
    assert_eq!(pq_summary.ingest_event_count, 2);
    assert_eq!(pq_summary.reconnect_count, 1);
    assert_eq!(pq_summary.gap_count, 1);
    assert_eq!(
        pq_summary.completeness,
        pb_service::CompletenessLevel::Partial
    );
}

// ---------------------------------------------------------------------------
// Test 3: Execution equivalence
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn cross_backend_execution_equivalence() {
    let base_ts = 1_700_000_200_000_000u64;
    let asset_str = "cross-exec-asset";
    let asset_id = AssetId::new(asset_str);
    let records = execution_event_records(asset_str, base_ts);

    // Write identical data to both backends.
    let (_tmp_dir, parquet_path) = setup_parquet();
    write_to_parquet(&parquet_path, &records).await;

    let (_container, ch_client, ch_url, ch_db) = setup_clickhouse().await;
    write_to_clickhouse(ch_client, &records).await;

    let pq_service = ParquetExecutionService::new(&parquet_path);
    let ch_service = ClickHouseExecutionService::new(&ch_url, &ch_db);

    let start_us = base_ts;
    let end_us = base_ts + 1_000_000;

    // --- Unfiltered query ---
    let pq_timeline = pq_service
        .timeline(None, None, start_us, end_us, 100, 0, false)
        .await
        .unwrap();
    let ch_timeline = ch_service
        .timeline(None, None, start_us, end_us, 100, 0, false)
        .await
        .unwrap();

    assert_eq!(
        pq_timeline.total_count, ch_timeline.total_count,
        "total_count mismatch (unfiltered)"
    );
    assert_eq!(
        pq_timeline.events.len(),
        ch_timeline.events.len(),
        "events.len mismatch (unfiltered)"
    );
    assert_eq!(pq_timeline.total_count, 4);

    // Compare order IDs and kinds in order.
    let pq_ids: Vec<&str> = pq_timeline
        .events
        .iter()
        .map(|e| e.order_id.as_str())
        .collect();
    let ch_ids: Vec<&str> = ch_timeline
        .events
        .iter()
        .map(|e| e.order_id.as_str())
        .collect();
    assert_eq!(pq_ids, ch_ids, "order_ids mismatch (unfiltered)");

    let pq_kinds: Vec<ExecutionEventKind> = pq_timeline.events.iter().map(|e| e.kind).collect();
    let ch_kinds: Vec<ExecutionEventKind> = ch_timeline.events.iter().map(|e| e.kind).collect();
    assert_eq!(pq_kinds, ch_kinds, "event kinds mismatch (unfiltered)");

    // --- Filter by asset_id ---
    let pq_filtered = pq_service
        .timeline(Some(&asset_id), None, start_us, end_us, 100, 0, false)
        .await
        .unwrap();
    let ch_filtered = ch_service
        .timeline(Some(&asset_id), None, start_us, end_us, 100, 0, false)
        .await
        .unwrap();
    assert_eq!(
        pq_filtered.total_count, ch_filtered.total_count,
        "total_count mismatch (asset filter)"
    );
    assert_eq!(pq_filtered.total_count, 4);

    // --- With limit ---
    let pq_limited = pq_service
        .timeline(None, None, start_us, end_us, 2, 0, false)
        .await
        .unwrap();
    let ch_limited = ch_service
        .timeline(None, None, start_us, end_us, 2, 0, false)
        .await
        .unwrap();
    assert_eq!(
        pq_limited.total_count, ch_limited.total_count,
        "total_count mismatch (limited)"
    );
    assert_eq!(
        pq_limited.events.len(),
        ch_limited.events.len(),
        "events.len mismatch (limited)"
    );
    assert_eq!(pq_limited.events.len(), 2);
    assert_eq!(pq_limited.total_count, 4); // total_count reflects all, not the limit
}
