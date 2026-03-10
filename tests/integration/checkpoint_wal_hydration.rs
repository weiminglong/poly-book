//! Integration test: write checkpoint + WAL segment, hydrate serve runtime,
//! verify book state matches expected hydrated state.

use std::sync::Arc;
use std::time::Duration;

use pb_api::hydration;
use pb_api::{FeedMode, LiveReadModel};
use pb_replay::reader::ParquetReader;
use pb_store::ParquetSink;
use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, DataSource, EventProvenance, PersistedRecord, Side,
};
use pb_types::{AssetId, FixedPrice, FixedSize, PriceLevel, Sequence};

fn provenance(recv: u64, exchange: u64, seq: u64) -> EventProvenance {
    EventProvenance {
        recv_timestamp_us: recv,
        exchange_timestamp_us: exchange,
        source: DataSource::WebSocket,
        source_event_id: None,
        source_session_id: Some("session-1".to_string()),
        sequence: Some(Sequence::new(seq)),
    }
}

#[tokio::test]
async fn hydrate_from_checkpoint_and_wal() {
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().to_str().unwrap().to_string();
    let wal_path = dir.path().join("wal");
    let asset_id_str = "hydration-test-asset";
    let asset_id = AssetId::new(asset_id_str);

    // -----------------------------------------------------------------------
    // Step 1: Write a checkpoint to Parquet via ParquetSink.
    // -----------------------------------------------------------------------
    let base_ts = 1_700_000_000_000_000u64; // some fixed timestamp in microseconds
    let checkpoint = PersistedRecord::Checkpoint(BookCheckpoint {
        asset_id: asset_id.clone(),
        checkpoint_timestamp_us: base_ts,
        provenance: EventProvenance {
            recv_timestamp_us: base_ts + 100,
            exchange_timestamp_us: base_ts,
            source: DataSource::RestSnapshot,
            source_event_id: Some("cp-1".to_string()),
            source_session_id: None,
            sequence: None,
        },
        wal_offset: Some(0),
        bids: vec![PriceLevel {
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
        }],
        asks: vec![PriceLevel {
            price: FixedPrice::new(5500).unwrap(),
            size: FixedSize::from_f64(200.0).unwrap(),
        }],
    });

    let (tx, rx) = tokio::sync::mpsc::channel::<PersistedRecord>(128);
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let sink = ParquetSink::new(rx, store, base_path.clone())
        .with_flush_interval(Duration::from_millis(10));

    let sink_handle = tokio::spawn(async move {
        let _ = sink.run().await;
    });

    tx.send(checkpoint).await.unwrap();
    // Give sink time to flush.
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(tx);
    let _ = sink_handle.await;

    // -----------------------------------------------------------------------
    // Step 2: Write WAL records (delta updates after the checkpoint).
    // -----------------------------------------------------------------------
    let wal_config = pb_wal::WalConfig {
        base_path: wal_path.clone(),
        segment_size: 4096,
        max_segments: 4,
        ..pb_wal::WalConfig::default()
    };
    let mut wal_writer = pb_wal::WalWriter::open(wal_config).unwrap();

    // Delta: add a new bid level at 4900.
    let delta1 = PersistedRecord::Book(BookEvent {
        asset_id: asset_id.clone(),
        kind: BookEventKind::Delta,
        side: Side::Bid,
        price: FixedPrice::new(4900).unwrap(),
        size: FixedSize::from_f64(50.0).unwrap(),
        provenance: provenance(base_ts + 1_000_000, base_ts + 1_000_000, 10),
    });

    // Delta: update the original bid level size.
    let delta2 = PersistedRecord::Book(BookEvent {
        asset_id: asset_id.clone(),
        kind: BookEventKind::Delta,
        side: Side::Bid,
        price: FixedPrice::new(5000).unwrap(),
        size: FixedSize::from_f64(150.0).unwrap(),
        provenance: provenance(base_ts + 2_000_000, base_ts + 2_000_000, 11),
    });

    wal_writer
        .append(&pb_wal::codec::encode(&delta1).unwrap())
        .unwrap();
    wal_writer
        .append(&pb_wal::codec::encode(&delta2).unwrap())
        .unwrap();
    wal_writer.flush().unwrap();
    drop(wal_writer);

    // -----------------------------------------------------------------------
    // Step 3: Hydrate a fresh LiveReadModel from checkpoint + WAL.
    // -----------------------------------------------------------------------
    let model = LiveReadModel::new(FeedMode::FixedTokens);
    model
        .set_active_assets(vec![asset_id_str.to_string()])
        .await;

    let reader = ParquetReader::new(&base_path);
    let hydration_start = std::time::Instant::now();
    let result = hydration::hydrate(
        &model,
        Some(&reader),
        Some(wal_path.as_path()),
        &[asset_id_str.to_string()],
    )
    .await;
    let hydration_elapsed = hydration_start.elapsed();

    // -----------------------------------------------------------------------
    // Step 4: Verify hydration result and book state.
    // -----------------------------------------------------------------------
    assert_eq!(result.checkpoints_loaded, 1, "should load one checkpoint");
    assert_eq!(
        result.wal_records_replayed, 2,
        "should replay two WAL records"
    );

    // Log cold-start time for observability. In release builds, checkpoint
    // hydration + 2 WAL records should complete well under 100ms. Debug builds
    // are significantly slower due to Parquet I/O without optimizations.
    eprintln!("hydration cold-start time: {:?}", hydration_elapsed);

    // The model should now be hydrated and ready.
    let snapshot = model
        .snapshot(asset_id_str, 50, 86400)
        .await
        .expect("snapshot should succeed after hydration");

    // Verify book state reflects checkpoint + WAL deltas:
    // - Original checkpoint had bid@5000 = 100.0
    // - Delta1 added bid@4900 = 50.0
    // - Delta2 updated bid@5000 = 150.0
    // - Ask@5500 = 200.0 (from checkpoint, unchanged)

    // Best bid should be 5000 (highest bid price).
    assert_eq!(
        snapshot.best_bid.as_ref().map(|l| l.price),
        Some(FixedPrice::new(5000).unwrap()),
        "best bid price after hydration"
    );

    // Best ask should be 5500.
    assert_eq!(
        snapshot.best_ask.as_ref().map(|l| l.price),
        Some(FixedPrice::new(5500).unwrap()),
        "best ask price after hydration"
    );

    // Should have 2 bid levels (5000, 4900).
    assert_eq!(snapshot.bid_depth, 2, "bid depth after hydration");

    // Should have 1 ask level (5500).
    assert_eq!(snapshot.ask_depth, 1, "ask depth after hydration");
}

#[tokio::test]
async fn hydrate_with_no_checkpoint_tails_wal_from_beginning() {
    let dir = tempfile::tempdir().unwrap();
    let base_path = dir.path().to_str().unwrap().to_string();
    let wal_path = dir.path().join("wal");
    let asset_id_str = "wal-only-asset";
    let asset_id = AssetId::new(asset_id_str);

    // Write WAL records only (no checkpoint).
    let wal_config = pb_wal::WalConfig {
        base_path: wal_path.clone(),
        segment_size: 4096,
        max_segments: 4,
        ..pb_wal::WalConfig::default()
    };
    let mut wal_writer = pb_wal::WalWriter::open(wal_config).unwrap();

    // Snapshot records in WAL.
    let snap_bid = PersistedRecord::Book(BookEvent {
        asset_id: asset_id.clone(),
        kind: BookEventKind::Snapshot,
        side: Side::Bid,
        price: FixedPrice::new(4000).unwrap(),
        size: FixedSize::from_f64(75.0).unwrap(),
        provenance: provenance(1_000_000, 1_000_000, 0),
    });
    let snap_ask = PersistedRecord::Book(BookEvent {
        asset_id: asset_id.clone(),
        kind: BookEventKind::Snapshot,
        side: Side::Ask,
        price: FixedPrice::new(4500).unwrap(),
        size: FixedSize::from_f64(80.0).unwrap(),
        provenance: provenance(1_000_000, 1_000_000, 1),
    });
    // Delta to trigger materialization.
    let delta = PersistedRecord::Book(BookEvent {
        asset_id: asset_id.clone(),
        kind: BookEventKind::Delta,
        side: Side::Bid,
        price: FixedPrice::new(3900).unwrap(),
        size: FixedSize::from_f64(25.0).unwrap(),
        provenance: provenance(2_000_000, 2_000_000, 2),
    });

    for record in [&snap_bid, &snap_ask, &delta] {
        wal_writer
            .append(&pb_wal::codec::encode(record).unwrap())
            .unwrap();
    }
    wal_writer.flush().unwrap();
    drop(wal_writer);

    // Hydrate without checkpoints.
    let model = LiveReadModel::new(FeedMode::FixedTokens);
    model
        .set_active_assets(vec![asset_id_str.to_string()])
        .await;

    let reader = ParquetReader::new(&base_path);
    let result = hydration::hydrate(
        &model,
        Some(&reader),
        Some(wal_path.as_path()),
        &[asset_id_str.to_string()],
    )
    .await;

    assert_eq!(result.checkpoints_loaded, 0);
    assert_eq!(result.wal_records_replayed, 3);

    let snapshot = model
        .snapshot(asset_id_str, 50, 86400)
        .await
        .expect("snapshot should work after WAL-only hydration");

    assert_eq!(snapshot.bid_depth, 2, "should have 2 bid levels from WAL");
    assert_eq!(snapshot.ask_depth, 1, "should have 1 ask level from WAL");
}
