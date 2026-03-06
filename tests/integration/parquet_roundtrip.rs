//! Test writing split datasets to Parquet and reading them back.

use std::sync::Arc;
use std::time::Duration;

use pb_replay::reader::{EventReader, ParquetReader};
use pb_store::ParquetSink;
use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, DataSource, EventProvenance, IngestEvent,
    IngestEventKind, PersistedRecord, Side, TradeEvent, TradeFidelity,
};
use pb_types::{AssetId, FixedPrice, FixedSize, PriceLevel, Sequence};

fn make_records(asset_id: &str, base_timestamp: u64) -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(asset_id);
    let provenance = |recv: u64, exchange: u64, sequence: u64| EventProvenance {
        recv_timestamp_us: recv,
        exchange_timestamp_us: exchange,
        source: DataSource::WebSocket,
        source_event_id: None,
        source_session_id: Some("session-1".to_string()),
        sequence: Some(Sequence::new(sequence)),
    };

    vec![
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            provenance: provenance(base_timestamp, base_timestamp, 0),
        }),
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Ask,
            price: FixedPrice::new(5500).unwrap(),
            size: FixedSize::from_f64(110.0).unwrap(),
            provenance: provenance(base_timestamp, base_timestamp, 1),
        }),
        PersistedRecord::Trade(TradeEvent {
            asset_id: asset_id.clone(),
            price: FixedPrice::new(5200).unwrap(),
            size: Some(FixedSize::from_f64(5.0).unwrap()),
            side: Some(Side::Bid),
            trade_id: Some("trade-1".to_string()),
            fidelity: TradeFidelity::Full,
            provenance: provenance(base_timestamp + 1_000_000, base_timestamp + 1_000_000, 2),
        }),
        PersistedRecord::Ingest(IngestEvent {
            asset_id: Some(asset_id.clone()),
            kind: IngestEventKind::ReconnectSuccess,
            provenance: EventProvenance {
                recv_timestamp_us: base_timestamp + 2_000_000,
                exchange_timestamp_us: 0,
                source: DataSource::WebSocket,
                source_event_id: None,
                source_session_id: Some("session-2".to_string()),
                sequence: None,
            },
            expected_sequence: None,
            observed_sequence: None,
            details: Some("reconnected".to_string()),
        }),
        PersistedRecord::Checkpoint(BookCheckpoint {
            asset_id,
            checkpoint_timestamp_us: base_timestamp + 3_000_000,
            recv_timestamp_us: base_timestamp + 3_000_100,
            exchange_timestamp_us: base_timestamp + 3_000_000,
            source: DataSource::RestSnapshot,
            source_event_id: Some("checkpoint-1".to_string()),
            source_session_id: None,
            bids: vec![PriceLevel {
                price: FixedPrice::new(5000).unwrap(),
                size: FixedSize::from_f64(100.0).unwrap(),
            }],
            asks: vec![PriceLevel {
                price: FixedPrice::new(5500).unwrap(),
                size: FixedSize::from_f64(110.0).unwrap(),
            }],
        }),
    ]
}

#[tokio::test]
async fn parquet_sink_writes_split_dataset_paths_and_reader_roundtrips() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let base_path = tmp_dir.path().to_string_lossy().to_string();
    let records = make_records("token-a", 1_700_000_000_000_000);

    let (tx, rx) = tokio::sync::mpsc::channel::<PersistedRecord>(1000);
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let sink = ParquetSink::new(rx, store, base_path.clone())
        .with_flush_interval(Duration::from_millis(50));
    let sink_handle = tokio::spawn(async move { sink.run().await.unwrap() });

    for record in &records {
        tx.send(record.clone()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(tx);
    sink_handle.await.unwrap();

    let mut found_files = Vec::new();
    visit_dir_recursive(tmp_dir.path(), &mut found_files);
    assert!(found_files.iter().any(|path| path.contains("book_events")));
    assert!(found_files.iter().any(|path| path.contains("trade_events")));
    assert!(found_files
        .iter()
        .any(|path| path.contains("ingest_events")));
    assert!(found_files
        .iter()
        .any(|path| path.contains("book_checkpoints")));

    let reader = ParquetReader::new(&base_path);
    let asset_id = AssetId::new("token-a");
    let window = reader
        .read_market_data(
            &asset_id,
            1_700_000_000_000_000,
            1_700_000_000_000_000 + 5_000_000,
        )
        .await
        .unwrap();
    assert_eq!(window.book_events.len(), 2);
    assert_eq!(window.trade_events.len(), 1);
    assert_eq!(window.ingest_events.len(), 1);

    let checkpoints = reader
        .read_checkpoints(
            &asset_id,
            1_700_000_000_000_000,
            1_700_000_000_000_000 + 5_000_000,
        )
        .await
        .unwrap();
    assert_eq!(checkpoints.len(), 1);
}

fn visit_dir_recursive(dir: &std::path::Path, files: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit_dir_recursive(&path, files);
            } else if path.extension().map(|e| e == "parquet").unwrap_or(false) {
                files.push(path.to_string_lossy().to_string());
            }
        }
    }
}
