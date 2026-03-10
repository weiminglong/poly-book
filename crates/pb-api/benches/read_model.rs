use criterion::{criterion_group, criterion_main, Criterion};
use pb_api::{FeedMode, LiveReadModel};
use pb_types::event::{BookEventKind, DataSource, EventProvenance};
use pb_types::{AssetId, BookEvent, FixedPrice, FixedSize, PersistedRecord, Sequence, Side};
use std::hint::black_box;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

fn provenance(seq: u64) -> EventProvenance {
    EventProvenance {
        recv_timestamp_us: 1_000_000,
        exchange_timestamp_us: 999_000,
        source: DataSource::WebSocket,
        source_event_id: None,
        source_session_id: None,
        sequence: Some(Sequence::new(seq)),
    }
}

fn snapshot_record(asset_id: &str, side: Side, price: f64, size: f64, seq: u64) -> PersistedRecord {
    PersistedRecord::Book(BookEvent {
        asset_id: AssetId::new(asset_id),
        kind: BookEventKind::Snapshot,
        side,
        price: FixedPrice::from_f64(price).unwrap(),
        size: FixedSize::from_f64(size).unwrap(),
        provenance: provenance(seq),
    })
}

fn bench_snapshot_read(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let asset_id = "bench-asset";

    // Build and populate the model inside the tokio runtime.
    let live = rt.block_on(async {
        let live = LiveReadModel::new(FeedMode::FixedTokens);
        live.set_active_assets(vec![asset_id.to_string()]).await;
        live.mark_hydrated().await;

        let (tx, rx) = mpsc::channel(2048);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let handle = live.spawn_consumer(rx, shutdown.child_token());

        let mut seq = 0u64;
        for i in 0..50 {
            let bid_price = 0.50 - (i as f64 * 0.001);
            let ask_price = 0.51 + (i as f64 * 0.001);
            tx.send(snapshot_record(
                asset_id,
                Side::Bid,
                bid_price,
                100.0 + i as f64,
                seq,
            ))
            .await
            .unwrap();
            seq += 1;
            tx.send(snapshot_record(
                asset_id,
                Side::Ask,
                ask_price,
                100.0 + i as f64,
                seq,
            ))
            .await
            .unwrap();
            seq += 1;
        }
        // Delta triggers snapshot group materialization.
        tx.send(PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new(asset_id),
            kind: BookEventKind::Delta,
            side: Side::Bid,
            price: FixedPrice::from_f64(0.49).unwrap(),
            size: FixedSize::from_f64(50.0).unwrap(),
            provenance: provenance(seq),
        }))
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(tx);
        shutdown.cancel();
        let _ = handle.await;
        live
    });

    c.bench_function("LiveReadModel::snapshot (50 levels, depth=20)", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(live.snapshot(asset_id, 20, 3600).await.unwrap());
            });
        });
    });

    c.bench_function("LiveReadModel::feed_status_raw", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(live.feed_status_raw().await);
            });
        });
    });

    c.bench_function("LiveReadModel::active_assets", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(live.active_assets(15).await);
            });
        });
    });

    c.bench_function("LiveReadModel::is_asset_active", |b| {
        b.iter(|| {
            rt.block_on(async {
                black_box(live.is_asset_active(asset_id).await);
            });
        });
    });
}

criterion_group!(benches, bench_snapshot_read);
criterion_main!(benches);
