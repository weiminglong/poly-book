use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use object_store::path::Path as ObjectPath;
use object_store::ObjectStore;
use object_store::PutPayload;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use tokio::sync::mpsc;

use pb_types::OrderbookEvent;

use crate::error::StoreError;
use crate::schema::{events_to_record_batch, orderbook_schema};

const DEFAULT_FLUSH_INTERVAL: Duration = Duration::from_secs(300);
const ROW_GROUP_SIZE: usize = 65_536;

pub struct ParquetSink {
    rx: mpsc::Receiver<OrderbookEvent>,
    store: Arc<dyn ObjectStore>,
    base_path: String,
    flush_interval: Duration,
}

impl ParquetSink {
    pub fn new(
        rx: mpsc::Receiver<OrderbookEvent>,
        store: Arc<dyn ObjectStore>,
        base_path: String,
    ) -> Self {
        Self {
            rx,
            store,
            base_path,
            flush_interval: DEFAULT_FLUSH_INTERVAL,
        }
    }

    pub fn with_flush_interval(mut self, interval: Duration) -> Self {
        self.flush_interval = interval;
        self
    }

    pub async fn run(mut self) -> Result<(), StoreError> {
        let mut buffer: Vec<OrderbookEvent> = Vec::new();
        let mut interval = tokio::time::interval(self.flush_interval);
        interval.tick().await; // consume the immediate first tick

        loop {
            tokio::select! {
                event = self.rx.recv() => {
                    match event {
                        Some(e) => buffer.push(e),
                        None => {
                            // Channel closed, flush remaining
                            if !buffer.is_empty() {
                                self.flush(&mut buffer).await?;
                            }
                            tracing::info!("ParquetSink channel closed, shutting down");
                            return Ok(());
                        }
                    }
                }
                _ = interval.tick() => {
                    if !buffer.is_empty() {
                        self.flush(&mut buffer).await?;
                    }
                }
            }
        }
    }

    async fn flush(&self, buffer: &mut Vec<OrderbookEvent>) -> Result<(), StoreError> {
        let flush_start = std::time::Instant::now();
        // Group events by (asset_id, hour) to handle cross-hour flushes correctly
        let mut by_asset_hour: HashMap<(String, String), Vec<&OrderbookEvent>> = HashMap::new();
        for event in buffer.iter() {
            let dt = chrono::DateTime::from_timestamp_micros(event.recv_timestamp_us as i64)
                .unwrap_or_default();
            let hour_key = format!(
                "{}/{:02}/{:02}/{:02}",
                dt.format("%Y"),
                dt.format("%m"),
                dt.format("%d"),
                dt.format("%H"),
            );
            let asset_id = event.asset_id.as_str().to_string();
            by_asset_hour
                .entry((asset_id, hour_key))
                .or_default()
                .push(event);
        }

        for ((asset_id, hour_key), events) in &by_asset_hour {
            let first_ts_us = events[0].recv_timestamp_us;

            let path = format!(
                "{}/{}/events_{}_{}.parquet",
                self.base_path, hour_key, asset_id, first_ts_us,
            );

            let owned_events: Vec<OrderbookEvent> = events.iter().map(|e| (*e).clone()).collect();
            let batch = events_to_record_batch(&owned_events)?;

            let schema = Arc::new(orderbook_schema());
            let props = WriterProperties::builder()
                .set_compression(Compression::ZSTD(Default::default()))
                .set_max_row_group_size(ROW_GROUP_SIZE)
                .build();

            let mut buf = Vec::new();
            let mut writer = ArrowWriter::try_new(&mut buf, schema, Some(props))?;
            writer.write(&batch)?;
            writer.close()?;

            let object_path = ObjectPath::from(path.as_str());
            self.store.put(&object_path, PutPayload::from(buf)).await?;

            tracing::debug!(
                asset_id = %asset_id,
                rows = events.len(),
                path = %path,
                "Flushed parquet file"
            );
        }

        // Only clear buffer after all writes succeed
        buffer.clear();

        pb_metrics::record_storage_flush("parquet");
        pb_metrics::record_flush_duration_ms(flush_start.elapsed().as_millis() as f64);

        Ok(())
    }
}
