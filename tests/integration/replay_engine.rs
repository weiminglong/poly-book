//! Test the replay engine: write events to Parquet, then reconstruct book state.

use std::sync::Arc;
use std::time::Duration;

use pb_replay::engine::ReplayEngine;
use pb_replay::reader::ParquetReader;
use pb_store::ParquetSink;
use pb_types::event::{EventType, OrderbookEvent, Side};
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

/// Create a sequence of events: 1 snapshot + N deltas.
fn make_replay_events(asset_id: &str, base_ts: u64) -> Vec<OrderbookEvent> {
    let aid = AssetId::new(asset_id);
    let mut events = Vec::new();
    let mut seq = 0u64;

    // Snapshot: 3 bids, 2 asks
    for (price, size) in [(5000u32, 100.0), (4900, 200.0), (4800, 300.0)] {
        events.push(OrderbookEvent {
            recv_timestamp_us: base_ts,
            exchange_timestamp_us: base_ts,
            asset_id: aid.clone(),
            event_type: EventType::Snapshot,
            side: Some(Side::Bid),
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::from_f64(size),
            sequence: Sequence::new(seq),
        });
        seq += 1;
    }
    for (price, size) in [(5500u32, 150.0), (5600, 250.0)] {
        events.push(OrderbookEvent {
            recv_timestamp_us: base_ts,
            exchange_timestamp_us: base_ts,
            asset_id: aid.clone(),
            event_type: EventType::Snapshot,
            side: Some(Side::Ask),
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::from_f64(size),
            sequence: Sequence::new(seq),
        });
        seq += 1;
    }

    // Delta 1: update best bid at T+1s
    events.push(OrderbookEvent {
        recv_timestamp_us: base_ts + 1_000_000,
        exchange_timestamp_us: base_ts + 1_000_000,
        asset_id: aid.clone(),
        event_type: EventType::Delta,
        side: Some(Side::Bid),
        price: FixedPrice::new(5000).unwrap(),
        size: FixedSize::from_f64(500.0),
        sequence: Sequence::new(seq),
    });
    seq += 1;

    // Delta 2: add new ask level at T+2s
    events.push(OrderbookEvent {
        recv_timestamp_us: base_ts + 2_000_000,
        exchange_timestamp_us: base_ts + 2_000_000,
        asset_id: aid.clone(),
        event_type: EventType::Delta,
        side: Some(Side::Ask),
        price: FixedPrice::new(5200).unwrap(),
        size: FixedSize::from_f64(75.0),
        sequence: Sequence::new(seq),
    });
    seq += 1;

    // Delta 3: remove a bid level at T+3s
    events.push(OrderbookEvent {
        recv_timestamp_us: base_ts + 3_000_000,
        exchange_timestamp_us: base_ts + 3_000_000,
        asset_id: aid.clone(),
        event_type: EventType::Delta,
        side: Some(Side::Bid),
        price: FixedPrice::new(4800).unwrap(),
        size: FixedSize::ZERO,
        sequence: Sequence::new(seq),
    });

    events
}

#[tokio::test]
async fn test_replay_reconstruct_at_snapshot() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let base_path = tmp_dir.path().to_string_lossy().to_string();

    // Use a timestamp that maps to a known hour directory
    let base_ts: u64 = 1_700_000_000_000_000; // ~2023-11-14
    let events = make_replay_events("replay-token", base_ts);

    // Write to Parquet
    let (tx, rx) = tokio::sync::mpsc::channel::<OrderbookEvent>(1000);
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let sink = ParquetSink::new(rx, store, base_path.clone())
        .with_flush_interval(Duration::from_millis(50));

    let handle = tokio::spawn(async move { sink.run().await.unwrap() });

    for event in &events {
        tx.send(event.clone()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(tx);
    handle.await.unwrap();

    // Replay at exact snapshot time — should get just the snapshot state
    let reader = ParquetReader::new(&base_path);
    let engine = ReplayEngine::new(reader);

    let book = engine
        .reconstruct_at(&AssetId::new("replay-token"), base_ts)
        .await
        .unwrap();

    assert_eq!(book.bid_depth(), 3);
    assert_eq!(book.ask_depth(), 2);
    assert_eq!(book.best_bid().unwrap().0.raw(), 5000);
    assert_eq!(book.best_ask().unwrap().0.raw(), 5500);
}

#[tokio::test]
async fn test_replay_reconstruct_after_deltas() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let base_path = tmp_dir.path().to_string_lossy().to_string();

    let base_ts: u64 = 1_700_000_000_000_000;
    let events = make_replay_events("replay-token-2", base_ts);

    let (tx, rx) = tokio::sync::mpsc::channel::<OrderbookEvent>(1000);
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let sink = ParquetSink::new(rx, store, base_path.clone())
        .with_flush_interval(Duration::from_millis(50));

    let handle = tokio::spawn(async move { sink.run().await.unwrap() });

    for event in &events {
        tx.send(event.clone()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(tx);
    handle.await.unwrap();

    // Replay at T+3s — should include all deltas
    let reader = ParquetReader::new(&base_path);
    let engine = ReplayEngine::new(reader);

    let book = engine
        .reconstruct_at(&AssetId::new("replay-token-2"), base_ts + 3_000_000)
        .await
        .unwrap();

    // Delta 1: bid at 5000 updated to 500 size
    assert_eq!(book.best_bid().unwrap().1.raw(), 500_000_000);

    // Delta 2: new ask at 5200 added (becomes best ask)
    assert_eq!(book.best_ask().unwrap().0.raw(), 5200);

    // Delta 3: bid at 4800 removed -> 2 bids remain
    assert_eq!(book.bid_depth(), 2);

    // 3 asks total: 5200, 5500, 5600
    assert_eq!(book.ask_depth(), 3);
}

#[tokio::test]
async fn test_replay_no_snapshot_error() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let base_path = tmp_dir.path().to_string_lossy().to_string();

    let reader = ParquetReader::new(&base_path);
    let engine = ReplayEngine::new(reader);

    let result = engine
        .reconstruct_at(&AssetId::new("nonexistent"), 1_000_000)
        .await;

    assert!(result.is_err());
}
