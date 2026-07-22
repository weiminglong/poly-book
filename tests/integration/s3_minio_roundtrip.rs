//! End-to-end persistence test against an S3-compatible object store (MinIO).
//!
//! This proves the S3 object-store wiring works against a real S3
//! API, not just that an `AmazonS3` type is constructed: an `s3://` base path is
//! resolved with `object_store::parse_url_opts(url, std::env::vars())` (the same
//! call `pb_bin::commands::pipeline::build_object_store` makes), the production
//! `ParquetSink` writes Parquet objects into the bucket, a *fresh* store handle
//! (simulating a process restart) still sees them, and the production
//! `ParquetReader` reads the original records through object-store range reads.
//! It also asserts no local `s3:`-named directory is created.
//!
//! Run with a MinIO/LocalStack endpoint:
//!   PB_TEST_S3_ENDPOINT=http://127.0.0.1:9100 \
//!   cargo test -p pb-integration-tests --test s3_minio_roundtrip -- --ignored
//!
//! Optional overrides: PB_TEST_S3_BUCKET (default poly-book-test),
//! PB_TEST_S3_ACCESS_KEY / PB_TEST_S3_SECRET_KEY (default minioadmin).

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use object_store::ObjectStore;
use pb_replay::{EventReader, ParquetReader};
use pb_store::ParquetSink;
use pb_types::event::{
    BookEvent, BookEventKind, DataSource, EventProvenance, PersistedRecord, Side, TradeEvent,
    TradeFidelity,
};
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

fn book_records(asset_id: &str, base_ts: u64) -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(asset_id);
    let provenance = |recv: u64, seq: u64| EventProvenance {
        recv_timestamp_us: recv,
        exchange_timestamp_us: recv,
        source: DataSource::WebSocket,
        source_event_id: None,
        source_session_id: Some("session-1".to_string()),
        sequence: Some(Sequence::new(seq)),
        ingest_ordinal: None,
    };
    vec![
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            provenance: provenance(base_ts, 0),
        }),
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Ask,
            price: FixedPrice::new(5500).unwrap(),
            size: FixedSize::from_f64(110.0).unwrap(),
            provenance: provenance(base_ts, 1),
        }),
        PersistedRecord::Trade(TradeEvent {
            asset_id,
            price: FixedPrice::new(5200).unwrap(),
            size: Some(FixedSize::from_f64(5.0).unwrap()),
            side: Some(Side::Bid),
            trade_id: Some("trade-1".to_string()),
            fidelity: TradeFidelity::Full,
            provenance: provenance(base_ts + 1_000_000, 2),
        }),
    ]
}

/// Mirror of `build_object_store` for `s3://` URLs: configuration (endpoint,
/// region, credentials) is taken from the process environment.
fn build_s3_store(base_path: &str) -> (Arc<dyn ObjectStore>, String) {
    let url = url::Url::parse(base_path).unwrap();
    let (store, prefix) = object_store::parse_url_opts(&url, std::env::vars())
        .expect("parse_url_opts must build an S3 store from env config");
    (Arc::from(store), prefix.to_string())
}

async fn list_parquet_objects(store: &Arc<dyn ObjectStore>, prefix: &str) -> Vec<String> {
    let prefix_path = object_store::path::Path::parse(prefix).unwrap();
    let mut stream = store.list(Some(&prefix_path));
    let mut out = Vec::new();
    while let Some(meta) = stream.next().await {
        let meta = meta.expect("listing S3 objects must succeed");
        let loc = meta.location.to_string();
        if loc.ends_with(".parquet") {
            out.push(loc);
        }
    }
    out
}

#[tokio::test]
#[ignore]
async fn s3_base_path_persists_parquet_and_survives_restart() {
    let endpoint = std::env::var("PB_TEST_S3_ENDPOINT")
        .expect("PB_TEST_S3_ENDPOINT is required for this explicitly ignored S3 integration test");
    let bucket =
        std::env::var("PB_TEST_S3_BUCKET").unwrap_or_else(|_| "poly-book-test".to_string());
    let access =
        std::env::var("PB_TEST_S3_ACCESS_KEY").unwrap_or_else(|_| "minioadmin".to_string());
    let secret =
        std::env::var("PB_TEST_S3_SECRET_KEY").unwrap_or_else(|_| "minioadmin".to_string());

    // Configure the AWS provider via env so parse_url_opts(url, env::vars())
    // wires endpoint + credentials + path-style addressing into the builder.
    // This test owns its own test binary, so process-global env is safe here.
    std::env::set_var("AWS_ENDPOINT", &endpoint);
    std::env::set_var("AWS_ACCESS_KEY_ID", &access);
    std::env::set_var("AWS_SECRET_ACCESS_KEY", &secret);
    std::env::set_var("AWS_REGION", "us-east-1");
    std::env::set_var("AWS_ALLOW_HTTP", "true");
    // MinIO/LocalStack need path-style requests, not virtual-hosted buckets.
    std::env::set_var("AWS_VIRTUAL_HOSTED_STYLE_REQUEST", "false");

    // A unique prefix per run keeps reruns independent within the shared bucket.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base_path = format!("s3://{bucket}/orderbook-{nanos}");
    let base_ts = 1_700_000_000_000_000;
    let records = book_records("token-s3", base_ts);

    // Capture cwd so we can assert no literal `s3:` directory leaks locally.
    let cwd_before = std::env::current_dir().unwrap();

    // --- Write through the production ParquetSink to the S3 store ---
    let (store, prefix) = build_s3_store(&base_path);
    let (tx, rx) = tokio::sync::mpsc::channel::<PersistedRecord>(64);
    let sink = ParquetSink::new(rx, store.clone(), prefix.clone())
        .with_flush_interval(Duration::from_millis(50));
    let sink_handle = tokio::spawn(async move { sink.run().await.unwrap() });
    for record in &records {
        tx.send(record.clone()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(250)).await;
    drop(tx);
    sink_handle.await.unwrap();

    // The s3:// path must NOT have created a local directory named `s3:`.
    assert!(
        !cwd_before.join("s3:").exists(),
        "an s3:// base path must never create a local s3: directory"
    );

    // Objects must actually exist in the bucket under our prefix.
    let written = list_parquet_objects(&store, &prefix).await;
    assert!(
        written.iter().any(|p| p.contains("book_events")),
        "book_events parquet must be persisted to S3, got: {written:?}"
    );
    assert!(
        written.iter().any(|p| p.contains("trade_events")),
        "trade_events parquet must be persisted to S3, got: {written:?}"
    );

    // --- Simulate a process restart: a brand-new store handle still sees them ---
    let (store2, prefix2) = build_s3_store(&base_path);
    let after_restart = list_parquet_objects(&store2, &prefix2).await;
    assert_eq!(
        after_restart.len(),
        written.len(),
        "objects must persist across a fresh store handle (restart): before={written:?} after={after_restart:?}"
    );

    // --- Application round-trip: the production reader must read S3 directly ---
    let reader = ParquetReader::from_store(store2, prefix2);
    let window = reader
        .read_market_data(&AssetId::new("token-s3"), base_ts - 1, base_ts + 2_000_000)
        .await
        .unwrap();
    assert_eq!(window.book_events.len(), 2);
    assert_eq!(window.trade_events.len(), 1);
}
