//! Test writing events to Parquet and reading them back.

use std::sync::Arc;
use std::time::Duration;

use pb_store::ParquetSink;
use pb_types::event::{EventType, OrderbookEvent, Side};
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

fn make_test_events(asset_id: &str, count: usize, base_timestamp: u64) -> Vec<OrderbookEvent> {
    let mut events = Vec::with_capacity(count);
    for i in 0..count {
        events.push(OrderbookEvent {
            recv_timestamp_us: base_timestamp + i as u64 * 1000,
            exchange_timestamp_us: base_timestamp + i as u64 * 1000 - 100,
            asset_id: AssetId::new(asset_id),
            event_type: if i == 0 {
                EventType::Snapshot
            } else {
                EventType::Delta
            },
            side: Some(if i % 2 == 0 { Side::Bid } else { Side::Ask }),
            price: FixedPrice::new((5000 + (i as u32 % 50) * 10).min(10000)).unwrap(),
            size: FixedSize::from_f64(100.0 + i as f64),
            sequence: Sequence::new(i as u64),
        });
    }
    events
}

#[tokio::test]
async fn test_parquet_write_and_read() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let base_path = tmp_dir.path().to_string_lossy().to_string();

    let events = make_test_events("test-token-abc", 50, 1_700_000_000_000_000);

    // Write events via ParquetSink
    let (tx, rx) = tokio::sync::mpsc::channel::<OrderbookEvent>(1000);
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let sink = ParquetSink::new(rx, store, base_path.clone())
        .with_flush_interval(Duration::from_millis(50));

    let sink_handle = tokio::spawn(async move {
        sink.run().await.unwrap();
    });

    // Send events
    for event in &events {
        tx.send(event.clone()).await.unwrap();
    }

    // Wait for flush, then close
    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(tx);
    sink_handle.await.unwrap();

    // Verify parquet files were created
    let mut found_files = Vec::new();
    visit_dir_recursive(tmp_dir.path(), &mut found_files);
    assert!(!found_files.is_empty(), "no parquet files written");

    // Read back and verify
    for path in &found_files {
        let file = tokio::fs::File::open(path).await.unwrap();
        let builder = parquet::arrow::async_reader::ParquetRecordBatchStreamBuilder::new(file)
            .await
            .unwrap();
        let mut stream = builder.build().unwrap();

        use futures_util::StreamExt;
        let mut total_rows = 0;
        while let Some(batch) = stream.next().await {
            let batch = batch.unwrap();
            total_rows += batch.num_rows();

            // Verify schema
            let schema = batch.schema();
            assert!(schema.field_with_name("recv_timestamp_us").is_ok());
            assert!(schema.field_with_name("asset_id").is_ok());
            assert!(schema.field_with_name("price").is_ok());
            assert!(schema.field_with_name("size").is_ok());
            assert!(schema.field_with_name("sequence").is_ok());
        }
        assert!(total_rows > 0, "parquet file was empty");
    }
}

#[tokio::test]
async fn test_parquet_groups_by_asset_id() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let base_path = tmp_dir.path().to_string_lossy().to_string();

    // Create events for two different assets
    let mut events = make_test_events("token-a", 10, 1_700_000_000_000_000);
    events.extend(make_test_events("token-b", 10, 1_700_000_000_000_000));

    let (tx, rx) = tokio::sync::mpsc::channel::<OrderbookEvent>(1000);
    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let sink = ParquetSink::new(rx, store, base_path.clone())
        .with_flush_interval(Duration::from_millis(50));

    let sink_handle = tokio::spawn(async move {
        sink.run().await.unwrap();
    });

    for event in &events {
        tx.send(event.clone()).await.unwrap();
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    drop(tx);
    sink_handle.await.unwrap();

    // Should have files for both asset IDs
    let mut found_files = Vec::new();
    visit_dir_recursive(tmp_dir.path(), &mut found_files);

    let has_token_a = found_files.iter().any(|p| p.contains("token-a"));
    let has_token_b = found_files.iter().any(|p| p.contains("token-b"));
    assert!(has_token_a, "no parquet file for token-a");
    assert!(has_token_b, "no parquet file for token-b");
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
