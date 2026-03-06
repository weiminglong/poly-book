//! Test the replay engine with split datasets and checkpoints.

use std::sync::Arc;
use std::time::Duration;

use pb_replay::engine::ReplayEngine;
use pb_replay::reader::ParquetReader;
use pb_store::ParquetSink;
use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, DataSource, EventProvenance, PersistedRecord,
    ReplayMode, Side,
};
use pb_types::{AssetId, FixedPrice, FixedSize, PriceLevel, Sequence};

fn make_replay_records(asset_id: &str, base_ts: u64) -> Vec<PersistedRecord> {
    let aid = AssetId::new(asset_id);
    let mut records = Vec::new();
    let mut seq = 0u64;

    for (price, size) in [(5000u32, 100.0), (4900, 200.0)] {
        records.push(PersistedRecord::Book(BookEvent {
            asset_id: aid.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::from_f64(size).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: base_ts,
                exchange_timestamp_us: base_ts,
                source: DataSource::WebSocket,
                source_event_id: Some("snapshot-1".to_string()),
                source_session_id: Some("session-1".to_string()),
                sequence: Some(Sequence::new(seq)),
            },
        }));
        seq += 1;
    }
    for (price, size) in [(5500u32, 150.0), (5600, 250.0)] {
        records.push(PersistedRecord::Book(BookEvent {
            asset_id: aid.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Ask,
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::from_f64(size).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: base_ts,
                exchange_timestamp_us: base_ts,
                source: DataSource::WebSocket,
                source_event_id: Some("snapshot-1".to_string()),
                source_session_id: Some("session-1".to_string()),
                sequence: Some(Sequence::new(seq)),
            },
        }));
        seq += 1;
    }

    records.push(PersistedRecord::Book(BookEvent {
        asset_id: aid.clone(),
        kind: BookEventKind::Delta,
        side: Side::Bid,
        price: FixedPrice::new(5000).unwrap(),
        size: FixedSize::from_f64(500.0).unwrap(),
        provenance: EventProvenance {
            recv_timestamp_us: base_ts + 1_000_000,
            exchange_timestamp_us: base_ts + 1_000_000,
            source: DataSource::WebSocket,
            source_event_id: Some("delta-1".to_string()),
            source_session_id: Some("session-1".to_string()),
            sequence: Some(Sequence::new(seq)),
        },
    }));
    seq += 1;
    records.push(PersistedRecord::Book(BookEvent {
        asset_id: aid.clone(),
        kind: BookEventKind::Delta,
        side: Side::Ask,
        price: FixedPrice::new(5200).unwrap(),
        size: FixedSize::from_f64(75.0).unwrap(),
        provenance: EventProvenance {
            recv_timestamp_us: base_ts + 2_000_000,
            exchange_timestamp_us: base_ts + 2_000_000,
            source: DataSource::WebSocket,
            source_event_id: Some("delta-2".to_string()),
            source_session_id: Some("session-1".to_string()),
            sequence: Some(Sequence::new(seq)),
        },
    }));

    records.push(PersistedRecord::Checkpoint(BookCheckpoint {
        asset_id: aid,
        checkpoint_timestamp_us: base_ts + 2_000_000,
        provenance: EventProvenance {
            recv_timestamp_us: base_ts + 2_000_500,
            exchange_timestamp_us: base_ts + 2_000_000,
            source: DataSource::RestSnapshot,
            source_event_id: Some("checkpoint-1".to_string()),
            source_session_id: None,
            sequence: None,
        },
        bids: vec![
            PriceLevel {
                price: FixedPrice::new(5000).unwrap(),
                size: FixedSize::from_f64(500.0).unwrap(),
            },
            PriceLevel {
                price: FixedPrice::new(4900).unwrap(),
                size: FixedSize::from_f64(200.0).unwrap(),
            },
        ],
        asks: vec![
            PriceLevel {
                price: FixedPrice::new(5200).unwrap(),
                size: FixedSize::from_f64(75.0).unwrap(),
            },
            PriceLevel {
                price: FixedPrice::new(5500).unwrap(),
                size: FixedSize::from_f64(150.0).unwrap(),
            },
            PriceLevel {
                price: FixedPrice::new(5600).unwrap(),
                size: FixedSize::from_f64(250.0).unwrap(),
            },
        ],
    }));

    records
}

#[tokio::test]
async fn replay_reconstructs_from_snapshot_and_checkpoint() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let base_path = tmp_dir.path().to_string_lossy().to_string();
    let base_ts: u64 = 1_700_000_000_000_000;
    let records = make_replay_records("replay-token", base_ts);

    let (tx, rx) = tokio::sync::mpsc::channel::<PersistedRecord>(1000);
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let sink = ParquetSink::new(rx, store, base_path.clone())
        .with_flush_interval(Duration::from_millis(50));
    let handle = tokio::spawn(async move { sink.run().await.unwrap() });

    for record in &records {
        tx.send(record.clone()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(tx);
    handle.await.unwrap();

    let reader = ParquetReader::new(&base_path);
    let engine = ReplayEngine::new(reader);

    let replay = engine
        .reconstruct_at(&AssetId::new("replay-token"), base_ts, ReplayMode::RecvTime)
        .await
        .unwrap();
    assert!(!replay.used_checkpoint);
    assert_eq!(replay.book.bid_depth(), 2);
    assert_eq!(replay.book.ask_depth(), 2);
    assert_eq!(replay.book.best_bid().unwrap().0.raw(), 5000);
    assert_eq!(replay.book.best_ask().unwrap().0.raw(), 5500);

    let replay = engine
        .reconstruct_at(
            &AssetId::new("replay-token"),
            base_ts + 2_000_000,
            ReplayMode::ExchangeTime,
        )
        .await
        .unwrap();
    assert!(replay.used_checkpoint);
    assert_eq!(replay.book.best_bid().unwrap().1.raw(), 500_000_000);
    assert_eq!(replay.book.best_ask().unwrap().0.raw(), 5200);
    assert_eq!(replay.book.ask_depth(), 3);
}

#[tokio::test]
async fn replay_validation_uses_next_checkpoint() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let base_path = tmp_dir.path().to_string_lossy().to_string();
    let base_ts: u64 = 1_700_000_000_000_000;
    let records = make_replay_records("replay-validate", base_ts);

    let (tx, rx) = tokio::sync::mpsc::channel::<PersistedRecord>(1000);
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let sink = ParquetSink::new(rx, store, base_path.clone())
        .with_flush_interval(Duration::from_millis(50));
    let handle = tokio::spawn(async move { sink.run().await.unwrap() });

    for record in &records {
        tx.send(record.clone()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(tx);
    handle.await.unwrap();

    let reader = ParquetReader::new(&base_path);
    let engine = ReplayEngine::new(reader);
    let validation = engine
        .validate_at(
            &AssetId::new("replay-validate"),
            base_ts,
            ReplayMode::RecvTime,
        )
        .await
        .unwrap()
        .unwrap();

    assert!(validation.matched);
    assert_eq!(validation.reference_timestamp_us, base_ts + 2_000_000);
}
