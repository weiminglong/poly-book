use std::path::PathBuf;

use arrow::array::{Array, AsArray};
use arrow::datatypes::{UInt32Type, UInt64Type};
use parquet::arrow::async_reader::ParquetRecordBatchStreamBuilder;
use tokio::fs::File;
use tracing::debug;

use pb_types::event::{EventType, OrderbookEvent, Side};
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

use crate::error::ReplayError;

/// Trait for reading orderbook events from storage.
pub trait EventReader: Send + Sync {
    fn read_events(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<Vec<OrderbookEvent>, ReplayError>> + Send;
}

/// Reads events from Parquet files in a time-partitioned directory layout.
///
/// Layout: `{base_path}/{YYYY}/{MM}/{DD}/{HH}/events_*.parquet`
pub struct ParquetReader {
    base_path: PathBuf,
}

impl ParquetReader {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Generate all hourly directory paths that cover [start_us, end_us].
    fn hour_paths(&self, start_us: u64, end_us: u64) -> Vec<PathBuf> {
        use chrono::{TimeZone, Timelike, Utc};

        let start_dt = Utc
            .timestamp_opt(start_us as i64 / 1_000_000, 0)
            .single()
            .unwrap_or_default();
        let end_dt = Utc
            .timestamp_opt(end_us as i64 / 1_000_000, 0)
            .single()
            .unwrap_or_default();

        let mut paths = Vec::new();
        let mut current = start_dt
            .date_naive()
            .and_hms_opt(start_dt.hour(), 0, 0)
            .unwrap();
        let end_naive = end_dt.naive_utc();

        while current <= end_naive {
            let path = self.base_path.join(format!(
                "{}/{:02}/{:02}/{:02}",
                current.format("%Y"),
                current.format("%m"),
                current.format("%d"),
                current.format("%H"),
            ));
            paths.push(path);
            current += chrono::Duration::hours(1);
        }
        paths
    }

    /// Read events from a single Parquet file.
    async fn read_parquet_file(
        &self,
        path: &std::path::Path,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<OrderbookEvent>, ReplayError> {
        let file = File::open(path).await?;
        let builder = ParquetRecordBatchStreamBuilder::new(file).await?;
        let mut stream = builder.build()?;

        let mut events = Vec::new();

        use futures_util::StreamExt;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            let batch_events = extract_events(&batch, asset_id, start_us, end_us)?;
            events.extend(batch_events);
        }

        Ok(events)
    }
}

fn extract_events(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<OrderbookEvent>, ReplayError> {
    let num_rows = batch.num_rows();
    let mut events = Vec::new();

    let recv_ts_col = batch
        .column_by_name("recv_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing recv_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let exchange_ts_col = batch
        .column_by_name("exchange_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing exchange_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let asset_id_col = batch
        .column_by_name("asset_id")
        .ok_or_else(|| ReplayError::Other("missing asset_id column".into()))?
        .as_string::<i32>();
    let event_type_col = batch
        .column_by_name("event_type")
        .ok_or_else(|| ReplayError::Other("missing event_type column".into()))?
        .as_primitive::<arrow::datatypes::UInt8Type>();
    let side_col = batch
        .column_by_name("side")
        .ok_or_else(|| ReplayError::Other("missing side column".into()))?
        .as_primitive::<arrow::datatypes::UInt8Type>();
    let price_col = batch
        .column_by_name("price")
        .ok_or_else(|| ReplayError::Other("missing price column".into()))?
        .as_primitive::<UInt32Type>();
    let size_col = batch
        .column_by_name("size")
        .ok_or_else(|| ReplayError::Other("missing size column".into()))?
        .as_primitive::<UInt64Type>();
    let sequence_col = batch
        .column_by_name("sequence")
        .ok_or_else(|| ReplayError::Other("missing sequence column".into()))?
        .as_primitive::<UInt64Type>();

    for i in 0..num_rows {
        let recv_ts = recv_ts_col.value(i);
        if recv_ts < start_us || recv_ts > end_us {
            continue;
        }

        let row_asset_id = asset_id_col.value(i);
        if row_asset_id != asset_id.as_str() {
            continue;
        }

        let event_type = match event_type_col.value(i) {
            1 => EventType::Snapshot,
            2 => EventType::Delta,
            3 => EventType::Trade,
            other => return Err(ReplayError::InvalidEventType(other.to_string())),
        };

        let side = if side_col.is_null(i) {
            None
        } else {
            match side_col.value(i) {
                1 => Some(Side::Bid),
                2 => Some(Side::Ask),
                other => return Err(ReplayError::InvalidSide(other.to_string())),
            }
        };

        let price = FixedPrice::new(price_col.value(i))?;

        events.push(OrderbookEvent {
            recv_timestamp_us: recv_ts,
            exchange_timestamp_us: exchange_ts_col.value(i),
            asset_id: AssetId::new(row_asset_id),
            event_type,
            side,
            price,
            size: FixedSize::new(size_col.value(i)),
            sequence: Sequence::new(sequence_col.value(i)),
        });
    }

    Ok(events)
}

impl EventReader for ParquetReader {
    async fn read_events(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<OrderbookEvent>, ReplayError> {
        let hour_dirs = self.hour_paths(start_us, end_us);
        let asset_file_prefix = format!("events_{}_", asset_id.as_str());
        let mut all_events = Vec::new();

        for dir in &hour_dirs {
            let mut entries = match tokio::fs::read_dir(dir).await {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    debug!(?dir, "hour directory not found, skipping");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with(&asset_file_prefix) && name.ends_with(".parquet") {
                        let file_events = self
                            .read_parquet_file(&path, asset_id, start_us, end_us)
                            .await?;
                        all_events.extend(file_events);
                    }
                }
            }
        }

        all_events.sort_by_key(|e| (e.recv_timestamp_us, e.sequence));
        Ok(all_events)
    }
}

/// Reads events from a ClickHouse database.
pub struct ClickHouseReader {
    client: clickhouse::Client,
    table: String,
}

/// Row type for ClickHouse deserialization.
#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct EventRow {
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: String,
    event_type: String,
    side: Option<String>,
    price: u32,
    size: u64,
    sequence: u64,
}

impl ClickHouseReader {
    pub fn new(url: &str, database: &str) -> Self {
        let client = clickhouse::Client::default()
            .with_url(url)
            .with_database(database);
        Self {
            client,
            table: "orderbook_events".to_string(),
        }
    }

    fn row_to_event(row: EventRow) -> Result<OrderbookEvent, ReplayError> {
        let event_type = match row.event_type.as_str() {
            "Snapshot" => EventType::Snapshot,
            "Delta" => EventType::Delta,
            "Trade" => EventType::Trade,
            other => return Err(ReplayError::InvalidEventType(other.to_string())),
        };

        let side = match row.side.as_deref() {
            Some("Bid") => Some(Side::Bid),
            Some("Ask") => Some(Side::Ask),
            None | Some("") => None,
            Some(other) => return Err(ReplayError::InvalidSide(other.to_string())),
        };

        let price = FixedPrice::new(row.price)?;

        Ok(OrderbookEvent {
            recv_timestamp_us: row.recv_timestamp_us,
            exchange_timestamp_us: row.exchange_timestamp_us,
            asset_id: AssetId::new(row.asset_id),
            event_type,
            side,
            price,
            size: FixedSize::new(row.size),
            sequence: Sequence::new(row.sequence),
        })
    }
}

impl EventReader for ClickHouseReader {
    async fn read_events(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<OrderbookEvent>, ReplayError> {
        let query = format!(
            "SELECT recv_timestamp_us, exchange_timestamp_us, asset_id, \
             event_type, side, price, size, sequence \
             FROM {} \
             WHERE asset_id = ? AND recv_timestamp_us >= ? AND recv_timestamp_us <= ? \
             ORDER BY recv_timestamp_us, sequence",
            self.table
        );

        let rows: Vec<EventRow> = self
            .client
            .query(&query)
            .bind(asset_id.as_str())
            .bind(start_us)
            .bind(end_us)
            .fetch_all()
            .await?;

        rows.into_iter().map(Self::row_to_event).collect()
    }
}
