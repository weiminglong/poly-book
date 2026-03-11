use std::sync::Arc;
use std::time::Duration;

use arrow::datatypes::DataType;
use futures_util::StreamExt;
use object_store::local::LocalFileSystem;
use object_store::ObjectStore;
use object_store::ObjectStoreExt;
use parquet::basic::Compression;
use parquet::file::reader::FileReader;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, DataSource, EventProvenance, ExecutionEvent,
    ExecutionEventKind, IngestEvent, IngestEventKind, LatencyTrace, PersistedRecord, ReplayMode,
    ReplayValidation, Side, TradeEvent,
};
use pb_types::{AssetId, FixedPrice, FixedSize, PriceLevel, Sequence, TradeFidelity};

use crate::schema::*;
use crate::writer::ParquetRecordWriter;
use crate::ParquetSink;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn test_provenance(recv_ts: u64) -> EventProvenance {
    EventProvenance {
        recv_timestamp_us: recv_ts,
        exchange_timestamp_us: recv_ts + 100,
        source: DataSource::WebSocket,
        source_event_id: Some("evt-1".into()),
        source_session_id: Some("sess-1".into()),
        sequence: Some(Sequence::new(1)),
    }
}

fn test_asset_id() -> AssetId {
    AssetId::new("BTC-5M-YES")
}

fn make_book_event(recv_ts: u64) -> BookEvent {
    BookEvent {
        asset_id: test_asset_id(),
        kind: BookEventKind::Delta,
        side: Side::Bid,
        price: FixedPrice::new(5000).unwrap(),
        size: FixedSize::new(1_000_000),
        provenance: test_provenance(recv_ts),
    }
}

fn make_trade_event(recv_ts: u64) -> TradeEvent {
    TradeEvent {
        asset_id: test_asset_id(),
        price: FixedPrice::new(5100).unwrap(),
        size: Some(FixedSize::new(500_000)),
        side: Some(Side::Ask),
        trade_id: Some("trade-001".into()),
        fidelity: TradeFidelity::Full,
        provenance: test_provenance(recv_ts),
    }
}

fn make_ingest_event(recv_ts: u64) -> IngestEvent {
    IngestEvent {
        asset_id: Some(test_asset_id()),
        kind: IngestEventKind::ReconnectStart,
        provenance: test_provenance(recv_ts),
        expected_sequence: Some(10),
        observed_sequence: Some(12),
        details: Some("test gap".into()),
    }
}

fn make_checkpoint(checkpoint_ts: u64) -> BookCheckpoint {
    BookCheckpoint {
        asset_id: test_asset_id(),
        checkpoint_timestamp_us: checkpoint_ts,
        provenance: test_provenance(checkpoint_ts),
        bids: vec![PriceLevel {
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::new(1_000_000),
        }],
        asks: vec![PriceLevel {
            price: FixedPrice::new(5100).unwrap(),
            size: FixedSize::new(2_000_000),
        }],
        wal_offset: Some(42),
    }
}

fn make_validation(persisted_at: u64) -> ReplayValidation {
    ReplayValidation {
        asset_id: test_asset_id(),
        mode: ReplayMode::RecvTime,
        replay_timestamp_us: persisted_at - 1000,
        reference_timestamp_us: persisted_at - 500,
        matched: true,
        mismatch_summary: None,
        persisted_at_us: persisted_at,
    }
}

fn make_execution_event(event_ts: u64) -> ExecutionEvent {
    ExecutionEvent {
        event_timestamp_us: event_ts,
        asset_id: Some(test_asset_id()),
        order_id: "order-001".into(),
        client_order_id: Some("client-001".into()),
        venue_order_id: Some("venue-001".into()),
        kind: ExecutionEventKind::SubmitIntent,
        side: Some(Side::Bid),
        price: Some(FixedPrice::new(4900).unwrap()),
        size: Some(FixedSize::new(100_000)),
        status: Some("pending".into()),
        reason: None,
        latency: LatencyTrace {
            market_data_recv_us: Some(event_ts - 50),
            normalization_done_us: Some(event_ts - 30),
            strategy_decision_us: Some(event_ts - 10),
            order_submit_us: Some(event_ts),
            exchange_ack_us: None,
            exchange_fill_us: None,
        },
    }
}

fn local_store(dir: &TempDir) -> Arc<dyn ObjectStore> {
    Arc::new(LocalFileSystem::new_with_prefix(dir.path()).unwrap())
}

// 2025-06-15 12:30:00 UTC in microseconds
const FIXED_TS_US: u64 = 1_750_000_200_000_000;

// ---------------------------------------------------------------------------
// Schema field tests
// ---------------------------------------------------------------------------

#[test]
fn book_event_schema_has_correct_fields() {
    let schema = book_event_schema();
    assert_eq!(schema.fields().len(), 11);
    assert_eq!(schema.field(0).name(), "recv_timestamp_us");
    assert_eq!(schema.field(0).data_type(), &DataType::UInt64);
    assert!(!schema.field(0).is_nullable());
    assert_eq!(schema.field(3).name(), "event_kind");
    assert_eq!(schema.field(3).data_type(), &DataType::UInt8);
    assert!(schema.field(7).is_nullable()); // sequence
}

#[test]
fn trade_event_schema_has_correct_fields() {
    let schema = trade_event_schema();
    assert_eq!(schema.fields().len(), 12);
    assert_eq!(schema.field(3).name(), "price");
    assert_eq!(schema.field(4).name(), "size");
    assert!(schema.field(4).is_nullable()); // size is nullable for trades
}

#[test]
fn ingest_event_schema_has_correct_fields() {
    let schema = ingest_event_schema();
    assert_eq!(schema.fields().len(), 11);
    assert!(schema.field(2).is_nullable()); // asset_id nullable
    assert_eq!(schema.field(3).name(), "event_kind");
    assert_eq!(schema.field(3).data_type(), &DataType::Utf8);
}

#[test]
fn checkpoint_schema_has_correct_fields() {
    let schema = checkpoint_schema();
    assert_eq!(schema.fields().len(), 10);
    assert_eq!(schema.field(0).name(), "checkpoint_timestamp_us");
    assert_eq!(schema.field(7).name(), "bids_json");
    assert_eq!(schema.field(8).name(), "asks_json");
    assert!(schema.field(9).is_nullable()); // wal_offset
}

#[test]
fn replay_validation_schema_has_correct_fields() {
    let schema = replay_validation_schema();
    assert_eq!(schema.fields().len(), 7);
    assert_eq!(schema.field(4).name(), "matched");
    assert_eq!(schema.field(4).data_type(), &DataType::Boolean);
    assert!(schema.field(5).is_nullable()); // mismatch_summary
}

#[test]
fn execution_event_schema_has_correct_fields() {
    let schema = execution_event_schema();
    assert_eq!(schema.fields().len(), 12);
    assert_eq!(schema.field(0).name(), "event_timestamp_us");
    assert!(schema.field(1).is_nullable()); // asset_id
    assert_eq!(schema.field(11).name(), "latency_json");
}

#[test]
fn schema_for_record_dispatches_correctly() {
    let book = PersistedRecord::Book(make_book_event(FIXED_TS_US));
    assert_eq!(schema_for_record(&book).fields().len(), 11);

    let trade = PersistedRecord::Trade(make_trade_event(FIXED_TS_US));
    assert_eq!(schema_for_record(&trade).fields().len(), 12);

    let ingest = PersistedRecord::Ingest(make_ingest_event(FIXED_TS_US));
    assert_eq!(schema_for_record(&ingest).fields().len(), 11);

    let checkpoint = PersistedRecord::Checkpoint(make_checkpoint(FIXED_TS_US));
    assert_eq!(schema_for_record(&checkpoint).fields().len(), 10);

    let validation = PersistedRecord::Validation(make_validation(FIXED_TS_US));
    assert_eq!(schema_for_record(&validation).fields().len(), 7);

    let execution = PersistedRecord::Execution(make_execution_event(FIXED_TS_US));
    assert_eq!(schema_for_record(&execution).fields().len(), 12);
}

// ---------------------------------------------------------------------------
// Record batch conversion tests
// ---------------------------------------------------------------------------

#[test]
fn book_event_record_batch_roundtrip() {
    let record = PersistedRecord::Book(make_book_event(FIXED_TS_US));
    let batch = records_to_record_batch(&[&record]).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 11);
}

#[test]
fn trade_event_record_batch_roundtrip() {
    let record = PersistedRecord::Trade(make_trade_event(FIXED_TS_US));
    let batch = records_to_record_batch(&[&record]).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 12);
}

#[test]
fn ingest_event_record_batch_roundtrip() {
    let record = PersistedRecord::Ingest(make_ingest_event(FIXED_TS_US));
    let batch = records_to_record_batch(&[&record]).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 11);
}

#[test]
fn checkpoint_record_batch_roundtrip() {
    let record = PersistedRecord::Checkpoint(make_checkpoint(FIXED_TS_US));
    let batch = records_to_record_batch(&[&record]).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 10);
}

#[test]
fn validation_record_batch_roundtrip() {
    let record = PersistedRecord::Validation(make_validation(FIXED_TS_US));
    let batch = records_to_record_batch(&[&record]).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 7);
}

#[test]
fn execution_event_record_batch_roundtrip() {
    let record = PersistedRecord::Execution(make_execution_event(FIXED_TS_US));
    let batch = records_to_record_batch(&[&record]).unwrap();
    assert_eq!(batch.num_rows(), 1);
    assert_eq!(batch.num_columns(), 12);
}

#[test]
fn multiple_book_events_in_single_batch() {
    let r1 = PersistedRecord::Book(make_book_event(FIXED_TS_US));
    let r2 = PersistedRecord::Book(make_book_event(FIXED_TS_US + 1_000_000));
    let r3 = PersistedRecord::Book(make_book_event(FIXED_TS_US + 2_000_000));
    let batch = records_to_record_batch(&[&r1, &r2, &r3]).unwrap();
    assert_eq!(batch.num_rows(), 3);
}

#[test]
fn mixed_record_types_rejected() {
    let book = PersistedRecord::Book(make_book_event(FIXED_TS_US));
    let trade = PersistedRecord::Trade(make_trade_event(FIXED_TS_US));
    let result = records_to_record_batch(&[&book, &trade]);
    assert!(result.is_err());
}

#[test]
fn empty_record_batch_rejected() {
    let empty: Vec<&PersistedRecord> = vec![];
    let result = records_to_record_batch(&empty);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// ParquetRecordWriter tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn writer_empty_batch_no_files() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let writer = ParquetRecordWriter::new(store.clone(), "data");

    writer.write_batch(&[]).await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert!(entries.is_empty(), "empty batch should not create files");
}

#[tokio::test]
async fn writer_single_book_event_creates_file() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let writer = ParquetRecordWriter::new(store.clone(), "data");

    let record = PersistedRecord::Book(make_book_event(FIXED_TS_US));
    writer.write_record(record).await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].location.as_ref().ends_with(".parquet"));
}

#[tokio::test]
async fn writer_path_includes_date_partition() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let writer = ParquetRecordWriter::new(store.clone(), "data");

    let record = PersistedRecord::Book(make_book_event(FIXED_TS_US));
    writer.write_record(record).await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    let path = entries[0].location.as_ref();
    assert!(
        path.contains("book_events"),
        "path should contain dataset: {path}"
    );
    assert!(
        path.contains("BTC-5M-YES"),
        "path should contain asset: {path}"
    );
}

#[tokio::test]
async fn writer_groups_by_dataset_and_hour() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let writer = ParquetRecordWriter::new(store.clone(), "data");

    let records = vec![
        PersistedRecord::Book(make_book_event(FIXED_TS_US)),
        PersistedRecord::Trade(make_trade_event(FIXED_TS_US)),
    ];
    writer.write_batch(&records).await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 2, "should produce one file per dataset");
}

#[tokio::test]
async fn writer_zstd_compression_applied() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let writer = ParquetRecordWriter::new(store.clone(), "data");

    let record = PersistedRecord::Book(make_book_event(FIXED_TS_US));
    writer.write_record(record).await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    let data = store
        .get(&entries[0].location)
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let reader =
        parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(data.to_vec()))
            .unwrap();
    let row_group = reader.metadata().row_group(0);
    let has_zstd = (0..row_group.num_columns())
        .any(|i| matches!(row_group.column(i).compression(), Compression::ZSTD(_)));
    assert!(
        has_zstd,
        "expected at least one column with ZSTD compression"
    );
}

#[tokio::test]
async fn writer_multiple_flushes_produce_separate_files() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);

    let writer1 = ParquetRecordWriter::new(store.clone(), "data");
    let r1 = PersistedRecord::Book(make_book_event(FIXED_TS_US));
    writer1.write_record(r1).await.unwrap();

    let writer2 = ParquetRecordWriter::new(store.clone(), "data");
    let r2 = PersistedRecord::Book(make_book_event(FIXED_TS_US + 1_000_000));
    writer2.write_record(r2).await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 2, "two flushes should produce two files");
}

#[tokio::test]
async fn writer_all_record_types_produce_valid_parquet() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let writer = ParquetRecordWriter::new(store.clone(), "data");

    let records = vec![
        PersistedRecord::Book(make_book_event(FIXED_TS_US)),
        PersistedRecord::Trade(make_trade_event(FIXED_TS_US)),
        PersistedRecord::Ingest(make_ingest_event(FIXED_TS_US)),
        PersistedRecord::Checkpoint(make_checkpoint(FIXED_TS_US)),
        PersistedRecord::Validation(make_validation(FIXED_TS_US)),
        PersistedRecord::Execution(make_execution_event(FIXED_TS_US)),
    ];
    writer.write_batch(&records).await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 6, "six datasets should produce six files");

    for entry in &entries {
        let data = store
            .get(&entry.location)
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let reader =
            parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(data.to_vec()))
                .unwrap();
        assert!(reader.metadata().num_row_groups() > 0);
    }
}

#[tokio::test]
async fn writer_large_batch_succeeds() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let writer = ParquetRecordWriter::new(store.clone(), "data");

    let records: Vec<PersistedRecord> = (0..1000)
        .map(|i| PersistedRecord::Book(make_book_event(FIXED_TS_US + i * 1000)))
        .collect();
    writer.write_batch(&records).await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(entries.len(), 1, "same hour partition should be one file");
}

#[tokio::test]
async fn writer_checkpoint_schema_has_bids_asks_json() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let writer = ParquetRecordWriter::new(store.clone(), "data");

    writer
        .write_record(PersistedRecord::Checkpoint(make_checkpoint(FIXED_TS_US)))
        .await
        .unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    let data = store
        .get(&entries[0].location)
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let reader =
        parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(data.to_vec()))
            .unwrap();
    let schema = reader.metadata().file_metadata().schema_descr();
    let col_names: Vec<_> = schema.columns().iter().map(|c| c.name()).collect();
    assert!(col_names.contains(&"bids_json"));
    assert!(col_names.contains(&"asks_json"));
    assert!(col_names.contains(&"wal_offset"));
}

#[tokio::test]
async fn writer_parquet_row_count_matches_input() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let writer = ParquetRecordWriter::new(store.clone(), "data");

    let n = 50;
    let records: Vec<PersistedRecord> = (0..n)
        .map(|i| PersistedRecord::Trade(make_trade_event(FIXED_TS_US + i * 1000)))
        .collect();
    writer.write_batch(&records).await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    let data = store
        .get(&entries[0].location)
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let reader =
        parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(data.to_vec()))
            .unwrap();
    assert_eq!(reader.metadata().file_metadata().num_rows(), n as i64);
}

// ---------------------------------------------------------------------------
// ParquetSink lifecycle tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parquet_sink_flushes_on_channel_close() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let (tx, rx) = mpsc::channel::<PersistedRecord>(32);

    let sink = ParquetSink::new(rx, store.clone(), "data".into())
        .with_flush_interval(Duration::from_secs(300));

    tx.send(PersistedRecord::Book(make_book_event(FIXED_TS_US)))
        .await
        .unwrap();
    drop(tx);

    sink.run().await.unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "should flush buffered records on channel close"
    );
}

#[tokio::test]
async fn parquet_sink_flushes_on_cancellation() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let (tx, rx) = mpsc::channel::<PersistedRecord>(32);
    let token = CancellationToken::new();

    let sink = ParquetSink::new(rx, store.clone(), "data".into())
        .with_flush_interval(Duration::from_secs(300));

    tx.send(PersistedRecord::Book(make_book_event(FIXED_TS_US)))
        .await
        .unwrap();

    let token_clone = token.clone();
    let handle = tokio::spawn(async move { sink.run_with_token(token_clone).await });

    tokio::time::sleep(Duration::from_millis(50)).await;
    token.cancel();

    handle.await.unwrap().unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "should flush buffered records on cancellation"
    );
}

#[tokio::test]
async fn parquet_sink_empty_channel_no_files() {
    let dir = TempDir::new().unwrap();
    let store = local_store(&dir);
    let (_tx, rx) = mpsc::channel::<PersistedRecord>(32);
    let token = CancellationToken::new();

    let sink = ParquetSink::new(rx, store.clone(), "data".into())
        .with_flush_interval(Duration::from_secs(300));

    let token_clone = token.clone();
    let handle = tokio::spawn(async move { sink.run_with_token(token_clone).await });

    token.cancel();
    handle.await.unwrap().unwrap();

    let entries: Vec<_> = store
        .list(None)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
    assert!(entries.is_empty(), "no records sent, no files should exist");
}
