//! Cross-backend benchmarks comparing Parquet vs ClickHouse query latency.
//!
//! Requires Docker for the ClickHouse testcontainer.
//! Run with: `cargo bench -p pb-service --bench cross_backend`

use criterion::{criterion_group, criterion_main, BenchmarkGroup, Criterion};
use pb_service::{
    ClickHouseExecutionService, ClickHouseIntegrityService, ClickHouseReplayService,
    ExecutionService, IntegrityService, ParquetExecutionService, ParquetIntegrityService,
    ParquetReplayService, ReplayService,
};
use pb_store::{ClickHouseRecordWriter, ParquetRecordWriter};
use pb_types::event::{
    BookEvent, BookEventKind, DataSource, EventProvenance, ExecutionEvent, ExecutionEventKind,
    IngestEventKind, LatencyTrace, PersistedRecord, ReplayMode, Side,
};
use pb_types::{AssetId, FixedPrice, FixedSize, IngestEvent, Sequence};
use std::sync::Arc;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::clickhouse::ClickHouse;

const ASSET_ID: &str = "bench-asset";
const BASE_TS: u64 = 1_700_000_000_000_000;

fn provenance(ts: u64, seq: u64) -> EventProvenance {
    EventProvenance {
        recv_timestamp_us: ts,
        exchange_timestamp_us: ts,
        source: DataSource::WebSocket,
        source_event_id: Some("bench-event".to_string()),
        source_session_id: Some("bench-session".to_string()),
        sequence: Some(Sequence::new(seq)),
    }
}

/// Generate ~100 book events (mix of snapshots and deltas, bids and asks).
fn generate_book_events() -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(ASSET_ID);
    let mut records = Vec::with_capacity(100);

    // Initial snapshot: 25 bid levels + 25 ask levels = 50 events
    for i in 0..25 {
        records.push(PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::new(5000 - (i as u32) * 10).unwrap(),
            size: FixedSize::from_f64(100.0 + i as f64).unwrap(),
            provenance: provenance(BASE_TS + i as u64, i as u64),
        }));
    }
    for i in 0..25 {
        records.push(PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Ask,
            price: FixedPrice::new(5100 + (i as u32) * 10).unwrap(),
            size: FixedSize::from_f64(100.0 + i as f64).unwrap(),
            provenance: provenance(BASE_TS + 25 + i as u64, 25 + i as u64),
        }));
    }

    // Deltas: 25 bid deltas + 25 ask deltas = 50 events
    for i in 0..25 {
        let ts_offset = 1_000_000 + i as u64 * 1_000;
        records.push(PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Delta,
            side: Side::Bid,
            price: FixedPrice::new(5000 - (i as u32) * 10).unwrap(),
            size: FixedSize::from_f64(110.0 + i as f64).unwrap(),
            provenance: provenance(BASE_TS + ts_offset, 50 + i as u64),
        }));
    }
    for i in 0..25 {
        let ts_offset = 1_000_000 + 25_000 + i as u64 * 1_000;
        records.push(PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Delta,
            side: Side::Ask,
            price: FixedPrice::new(5100 + (i as u32) * 10).unwrap(),
            size: FixedSize::from_f64(110.0 + i as f64).unwrap(),
            provenance: provenance(BASE_TS + ts_offset, 75 + i as u64),
        }));
    }

    records
}

/// Generate ~10 ingest events (reconnects and gaps).
fn generate_ingest_events() -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(ASSET_ID);
    let kinds = [
        IngestEventKind::ReconnectStart,
        IngestEventKind::ReconnectSuccess,
        IngestEventKind::SequenceGap,
        IngestEventKind::ReconnectStart,
        IngestEventKind::ReconnectSuccess,
        IngestEventKind::SourceReset,
        IngestEventKind::StaleSnapshotSkip,
        IngestEventKind::ReconnectStart,
        IngestEventKind::ReconnectSuccess,
        IngestEventKind::SequenceGap,
    ];

    kinds
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            let ts = BASE_TS + 2_000_000 + i as u64 * 500;
            PersistedRecord::Ingest(IngestEvent {
                asset_id: Some(asset_id.clone()),
                kind: *kind,
                provenance: EventProvenance {
                    recv_timestamp_us: ts,
                    exchange_timestamp_us: ts,
                    source: DataSource::WebSocket,
                    source_event_id: None,
                    source_session_id: Some("bench-session".to_string()),
                    sequence: None,
                },
                expected_sequence: if *kind == IngestEventKind::SequenceGap {
                    Some(i as u64 * 10)
                } else {
                    None
                },
                observed_sequence: if *kind == IngestEventKind::SequenceGap {
                    Some(i as u64 * 10 + 3)
                } else {
                    None
                },
                details: Some(format!("bench ingest event {i}")),
            })
        })
        .collect()
}

/// Generate ~20 execution events across multiple orders.
fn generate_execution_events() -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(ASSET_ID);
    let mut records = Vec::with_capacity(20);

    for order_idx in 0..5 {
        let order_id = format!("bench-order-{order_idx}");
        let order_base_ts = BASE_TS + 3_000_000 + order_idx as u64 * 10_000;

        let lifecycle = [
            (ExecutionEventKind::SubmitIntent, "open"),
            (ExecutionEventKind::ExchangeAck, "accepted"),
            (ExecutionEventKind::PartialFill, "partial"),
            (ExecutionEventKind::Fill, "filled"),
        ];

        for (step, (kind, status)) in lifecycle.iter().enumerate() {
            let ts = order_base_ts + step as u64 * 500;
            records.push(PersistedRecord::Execution(ExecutionEvent {
                event_timestamp_us: ts,
                asset_id: Some(asset_id.clone()),
                order_id: order_id.clone(),
                client_order_id: Some(format!("client-{order_idx}")),
                venue_order_id: Some(format!("venue-{order_idx}")),
                kind: *kind,
                side: Some(Side::Bid),
                price: Some(FixedPrice::new(5000).unwrap()),
                size: Some(FixedSize::from_f64(10.0 + order_idx as f64).unwrap()),
                status: Some(status.to_string()),
                reason: None,
                latency: LatencyTrace::default(),
            }));
        }
    }

    records
}

struct SetupResult {
    _container: testcontainers::ContainerAsync<ClickHouse>,
    ch_url: String,
    ch_db: String,
    parquet_path: String,
    _tmp_dir: tempfile::TempDir,
}

async fn try_setup() -> Option<SetupResult> {
    // Start ClickHouse container — fails gracefully if Docker is unavailable.
    let container = match ClickHouse::default().start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Could not start ClickHouse container: {e}");
            return None;
        }
    };

    let port = container.get_host_port_ipv4(8123).await.ok()?;
    let ch_url = format!("http://127.0.0.1:{port}");

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let ch_db = format!("bench_{nanos}");

    let client = clickhouse::Client::default().with_url(&ch_url);
    client
        .query(&format!("CREATE DATABASE {ch_db}"))
        .execute()
        .await
        .ok()?;
    let client = client.with_database(&ch_db);

    // Generate test data.
    let mut all_records = generate_book_events();
    all_records.extend(generate_ingest_events());
    all_records.extend(generate_execution_events());

    // Write to ClickHouse.
    let ch_writer = ClickHouseRecordWriter::new(client);
    ch_writer.ensure_tables().await.ok()?;
    ch_writer.write_batch(&all_records).await.ok()?;

    // Write to Parquet.
    let tmp_dir = tempfile::tempdir().ok()?;
    let parquet_path = tmp_dir.path().to_string_lossy().to_string();
    let store =
        Arc::new(object_store::local::LocalFileSystem::new()) as Arc<dyn object_store::ObjectStore>;
    let pq_writer = ParquetRecordWriter::new(store, parquet_path.clone());
    pq_writer.write_batch(&all_records).await.ok()?;

    Some(SetupResult {
        _container: container,
        ch_url,
        ch_db,
        parquet_path,
        _tmp_dir: tmp_dir,
    })
}

fn bench_replay(c: &mut Criterion, setup: &SetupResult) {
    let mut group: BenchmarkGroup<'_, criterion::measurement::WallTime> =
        c.benchmark_group("replay");

    let asset_id = AssetId::new(ASSET_ID);
    // Target timestamp is after all deltas so the full book is reconstructed.
    let target_ts = BASE_TS + 2_000_000;
    let rt = tokio::runtime::Runtime::new().unwrap();

    group.bench_function("parquet", |b| {
        let service = ParquetReplayService::new(&setup.parquet_path);
        b.iter(|| {
            rt.block_on(async {
                let _ = service
                    .reconstruct(&asset_id, target_ts, ReplayMode::RecvTime, None)
                    .await;
            });
        });
    });

    group.bench_function("clickhouse", |b| {
        let service = ClickHouseReplayService::new(&setup.ch_url, &setup.ch_db);
        b.iter(|| {
            rt.block_on(async {
                let _ = service
                    .reconstruct(&asset_id, target_ts, ReplayMode::RecvTime, None)
                    .await;
            });
        });
    });

    group.finish();
}

fn bench_integrity(c: &mut Criterion, setup: &SetupResult) {
    let mut group: BenchmarkGroup<'_, criterion::measurement::WallTime> =
        c.benchmark_group("integrity");

    let asset_id = AssetId::new(ASSET_ID);
    let start_us = BASE_TS;
    let end_us = BASE_TS + 10_000_000;
    let rt = tokio::runtime::Runtime::new().unwrap();

    group.bench_function("parquet", |b| {
        let service = ParquetIntegrityService::new(&setup.parquet_path);
        b.iter(|| {
            rt.block_on(async {
                let _ = service.summary(&asset_id, start_us, end_us).await;
            });
        });
    });

    group.bench_function("clickhouse", |b| {
        let service = ClickHouseIntegrityService::new(&setup.ch_url, &setup.ch_db);
        b.iter(|| {
            rt.block_on(async {
                let _ = service.summary(&asset_id, start_us, end_us).await;
            });
        });
    });

    group.finish();
}

fn bench_execution(c: &mut Criterion, setup: &SetupResult) {
    let mut group: BenchmarkGroup<'_, criterion::measurement::WallTime> =
        c.benchmark_group("execution");

    let asset_id = AssetId::new(ASSET_ID);
    let start_us = BASE_TS;
    let end_us = BASE_TS + 10_000_000;
    let rt = tokio::runtime::Runtime::new().unwrap();

    group.bench_function("parquet", |b| {
        let service = ParquetExecutionService::new(&setup.parquet_path);
        b.iter(|| {
            rt.block_on(async {
                let _ = service
                    .timeline(Some(&asset_id), None, start_us, end_us, 100, 0, true)
                    .await;
            });
        });
    });

    group.bench_function("clickhouse", |b| {
        let service = ClickHouseExecutionService::new(&setup.ch_url, &setup.ch_db);
        b.iter(|| {
            rt.block_on(async {
                let _ = service
                    .timeline(Some(&asset_id), None, start_us, end_us, 100, 0, true)
                    .await;
            });
        });
    });

    group.finish();
}

fn cross_backend_benchmarks(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let setup = rt.block_on(async { try_setup().await });

    let setup = match setup {
        Some(s) => s,
        None => {
            eprintln!("Skipping cross-backend bench: Docker/ClickHouse unavailable");
            return;
        }
    };

    bench_replay(c, &setup);
    bench_integrity(c, &setup);
    bench_execution(c, &setup);
}

criterion_group!(benches, cross_backend_benchmarks);
criterion_main!(benches);
