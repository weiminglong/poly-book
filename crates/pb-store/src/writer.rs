use std::collections::HashMap;
use std::sync::Arc;

use clickhouse::Client;
use object_store::path::Path as ObjectPath;
use object_store::ObjectStore;
use object_store::PutPayload;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use serde::Serialize;

use pb_types::event::{BookEventKind, ExecutionEventKind, PersistedRecord, Side};

use crate::error::StoreError;
use crate::schema::{records_to_record_batch, schema_for_record};

const ROW_GROUP_SIZE: usize = 65_536;

const CREATE_BOOK_EVENTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS book_events (
    recv_timestamp_us UInt64,
    exchange_timestamp_us UInt64,
    asset_id String,
    event_kind Enum8('Snapshot' = 1, 'Delta' = 2),
    side Enum8('Bid' = 1, 'Ask' = 2),
    price UInt32,
    size UInt64,
    sequence Nullable(UInt64),
    source String,
    source_event_id Nullable(String),
    source_session_id Nullable(String),
    event_date Date MATERIALIZED toDate(fromUnixTimestamp64Micro(recv_timestamp_us))
) ENGINE = MergeTree()
PARTITION BY event_date
ORDER BY (asset_id, recv_timestamp_us, sequence, price)
"#;

const CREATE_TRADE_EVENTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS trade_events (
    recv_timestamp_us UInt64,
    exchange_timestamp_us UInt64,
    asset_id String,
    price UInt32,
    size Nullable(UInt64),
    side Nullable(Enum8('Bid' = 1, 'Ask' = 2)),
    trade_id Nullable(String),
    fidelity String,
    sequence Nullable(UInt64),
    source String,
    source_event_id Nullable(String),
    source_session_id Nullable(String),
    event_date Date MATERIALIZED toDate(fromUnixTimestamp64Micro(recv_timestamp_us))
) ENGINE = MergeTree()
PARTITION BY event_date
ORDER BY (asset_id, recv_timestamp_us, trade_id)
"#;

const CREATE_INGEST_EVENTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS ingest_events (
    recv_timestamp_us UInt64,
    exchange_timestamp_us UInt64,
    asset_id Nullable(String),
    event_kind String,
    sequence Nullable(UInt64),
    expected_sequence Nullable(UInt64),
    observed_sequence Nullable(UInt64),
    details Nullable(String),
    source String,
    source_event_id Nullable(String),
    source_session_id Nullable(String),
    event_date Date MATERIALIZED toDate(fromUnixTimestamp64Micro(recv_timestamp_us))
) ENGINE = MergeTree()
PARTITION BY event_date
ORDER BY (recv_timestamp_us, event_kind, source_session_id)
"#;

const CREATE_BOOK_CHECKPOINTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS book_checkpoints (
    checkpoint_timestamp_us UInt64,
    recv_timestamp_us UInt64,
    exchange_timestamp_us UInt64,
    asset_id String,
    source String,
    source_event_id Nullable(String),
    source_session_id Nullable(String),
    bids_json String,
    asks_json String,
    event_date Date MATERIALIZED toDate(fromUnixTimestamp64Micro(checkpoint_timestamp_us))
) ENGINE = MergeTree()
PARTITION BY event_date
ORDER BY (asset_id, checkpoint_timestamp_us)
"#;

const CREATE_REPLAY_VALIDATIONS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS replay_validations (
    asset_id String,
    mode String,
    replay_timestamp_us UInt64,
    reference_timestamp_us UInt64,
    matched UInt8,
    mismatch_summary Nullable(String),
    persisted_at_us UInt64,
    event_date Date MATERIALIZED toDate(fromUnixTimestamp64Micro(persisted_at_us))
) ENGINE = MergeTree()
PARTITION BY event_date
ORDER BY (asset_id, persisted_at_us, replay_timestamp_us)
"#;

const CREATE_EXECUTION_EVENTS_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS execution_events (
    event_timestamp_us UInt64,
    asset_id Nullable(String),
    order_id String,
    client_order_id Nullable(String),
    venue_order_id Nullable(String),
    event_kind String,
    side Nullable(Enum8('Bid' = 1, 'Ask' = 2)),
    price Nullable(UInt32),
    size Nullable(UInt64),
    status Nullable(String),
    reason Nullable(String),
    latency_json String,
    event_date Date MATERIALIZED toDate(fromUnixTimestamp64Micro(event_timestamp_us))
) ENGINE = MergeTree()
PARTITION BY event_date
ORDER BY (order_id, event_timestamp_us)
"#;

#[derive(Clone)]
pub struct ParquetRecordWriter {
    store: Arc<dyn ObjectStore>,
    base_path: String,
}

impl ParquetRecordWriter {
    pub fn new(store: Arc<dyn ObjectStore>, base_path: impl Into<String>) -> Self {
        Self {
            store,
            base_path: base_path.into(),
        }
    }

    pub async fn write_record(&self, record: PersistedRecord) -> Result<(), StoreError> {
        self.write_batch(std::slice::from_ref(&record)).await
    }

    pub async fn write_batch(&self, records: &[PersistedRecord]) -> Result<(), StoreError> {
        if records.is_empty() {
            return Ok(());
        }

        let flush_start = std::time::Instant::now();
        let mut groups: HashMap<(String, String, String), Vec<&PersistedRecord>> = HashMap::new();
        for record in records {
            let dt =
                chrono::DateTime::from_timestamp_micros(record.partition_timestamp_us() as i64)
                    .unwrap_or_default();
            let hour_key = format!(
                "{}/{:02}/{:02}/{:02}",
                dt.format("%Y"),
                dt.format("%m"),
                dt.format("%d"),
                dt.format("%H"),
            );
            groups
                .entry((
                    record.dataset_name().to_string(),
                    record.asset_partition().to_string(),
                    hour_key,
                ))
                .or_default()
                .push(record);
        }

        for ((dataset, asset, hour_key), records) in &groups {
            let first_ts_us = records[0].partition_timestamp_us();
            let path = format!(
                "{}/{}/{}/{}_{}.parquet",
                self.base_path, dataset, hour_key, asset, first_ts_us
            );

            let batch = records_to_record_batch(records)?;
            let schema = Arc::new(schema_for_record(records[0]));
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
                dataset = %dataset,
                asset = %asset,
                rows = records.len(),
                path = %path,
                "flushed parquet file"
            );
        }

        pb_metrics::record_storage_flush("parquet");
        pb_metrics::record_flush_duration_ms(flush_start.elapsed().as_millis() as f64);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct BookEventRow {
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: String,
    event_kind: String,
    side: String,
    price: u32,
    size: u64,
    sequence: Option<u64>,
    source: String,
    source_event_id: Option<String>,
    source_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct TradeEventRow {
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: String,
    price: u32,
    size: Option<u64>,
    side: Option<String>,
    trade_id: Option<String>,
    fidelity: String,
    sequence: Option<u64>,
    source: String,
    source_event_id: Option<String>,
    source_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct IngestEventRow {
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: Option<String>,
    event_kind: String,
    sequence: Option<u64>,
    expected_sequence: Option<u64>,
    observed_sequence: Option<u64>,
    details: Option<String>,
    source: String,
    source_event_id: Option<String>,
    source_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct CheckpointRow {
    checkpoint_timestamp_us: u64,
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: String,
    source: String,
    source_event_id: Option<String>,
    source_session_id: Option<String>,
    bids_json: String,
    asks_json: String,
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct ReplayValidationRow {
    asset_id: String,
    mode: String,
    replay_timestamp_us: u64,
    reference_timestamp_us: u64,
    matched: u8,
    mismatch_summary: Option<String>,
    persisted_at_us: u64,
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct ExecutionEventRow {
    event_timestamp_us: u64,
    asset_id: Option<String>,
    order_id: String,
    client_order_id: Option<String>,
    venue_order_id: Option<String>,
    event_kind: String,
    side: Option<String>,
    price: Option<u32>,
    size: Option<u64>,
    status: Option<String>,
    reason: Option<String>,
    latency_json: String,
}

fn side_to_string(side: Option<Side>) -> Option<String> {
    side.map(|value| match value {
        Side::Bid => "Bid".to_string(),
        Side::Ask => "Ask".to_string(),
    })
}

#[derive(Clone)]
pub struct ClickHouseRecordWriter {
    client: Client,
}

impl ClickHouseRecordWriter {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn ensure_tables(&self) -> Result<(), StoreError> {
        self.client.query(CREATE_BOOK_EVENTS_DDL).execute().await?;
        self.client.query(CREATE_TRADE_EVENTS_DDL).execute().await?;
        self.client
            .query(CREATE_INGEST_EVENTS_DDL)
            .execute()
            .await?;
        self.client
            .query(CREATE_BOOK_CHECKPOINTS_DDL)
            .execute()
            .await?;
        self.client
            .query(CREATE_REPLAY_VALIDATIONS_DDL)
            .execute()
            .await?;
        self.client
            .query(CREATE_EXECUTION_EVENTS_DDL)
            .execute()
            .await?;
        tracing::info!("ensured ClickHouse tables exist");
        Ok(())
    }

    pub async fn write_record(&self, record: PersistedRecord) -> Result<(), StoreError> {
        self.write_batch(std::slice::from_ref(&record)).await
    }

    pub async fn write_batch(&self, records: &[PersistedRecord]) -> Result<(), StoreError> {
        if records.is_empty() {
            return Ok(());
        }

        let flush_start = std::time::Instant::now();
        let mut book_insert = self.client.insert("book_events")?;
        let mut trade_insert = self.client.insert("trade_events")?;
        let mut ingest_insert = self.client.insert("ingest_events")?;
        let mut checkpoint_insert = self.client.insert("book_checkpoints")?;
        let mut validation_insert = self.client.insert("replay_validations")?;
        let mut execution_insert = self.client.insert("execution_events")?;

        for record in records {
            match record {
                PersistedRecord::Book(event) => {
                    let row = BookEventRow {
                        recv_timestamp_us: event.provenance.recv_timestamp_us,
                        exchange_timestamp_us: event.provenance.exchange_timestamp_us,
                        asset_id: event.asset_id.as_str().to_string(),
                        event_kind: match event.kind {
                            BookEventKind::Snapshot => "Snapshot".to_string(),
                            BookEventKind::Delta => "Delta".to_string(),
                        },
                        side: match event.side {
                            Side::Bid => "Bid".to_string(),
                            Side::Ask => "Ask".to_string(),
                        },
                        price: event.price.raw(),
                        size: event.size.raw(),
                        sequence: event.provenance.sequence.map(|seq| seq.raw()),
                        source: event.provenance.source.to_string(),
                        source_event_id: event.provenance.source_event_id.clone(),
                        source_session_id: event.provenance.source_session_id.clone(),
                    };
                    book_insert.write(&row).await?;
                }
                PersistedRecord::Trade(event) => {
                    let row = TradeEventRow {
                        recv_timestamp_us: event.provenance.recv_timestamp_us,
                        exchange_timestamp_us: event.provenance.exchange_timestamp_us,
                        asset_id: event.asset_id.as_str().to_string(),
                        price: event.price.raw(),
                        size: event.size.map(|size| size.raw()),
                        side: side_to_string(event.side),
                        trade_id: event.trade_id.clone(),
                        fidelity: event.fidelity.to_string(),
                        sequence: event.provenance.sequence.map(|seq| seq.raw()),
                        source: event.provenance.source.to_string(),
                        source_event_id: event.provenance.source_event_id.clone(),
                        source_session_id: event.provenance.source_session_id.clone(),
                    };
                    trade_insert.write(&row).await?;
                }
                PersistedRecord::Ingest(event) => {
                    let row = IngestEventRow {
                        recv_timestamp_us: event.provenance.recv_timestamp_us,
                        exchange_timestamp_us: event.provenance.exchange_timestamp_us,
                        asset_id: event.asset_id.as_ref().map(|id| id.as_str().to_string()),
                        event_kind: event.kind.to_string(),
                        sequence: event.provenance.sequence.map(|seq| seq.raw()),
                        expected_sequence: event.expected_sequence,
                        observed_sequence: event.observed_sequence,
                        details: event.details.clone(),
                        source: event.provenance.source.to_string(),
                        source_event_id: event.provenance.source_event_id.clone(),
                        source_session_id: event.provenance.source_session_id.clone(),
                    };
                    ingest_insert.write(&row).await?;
                }
                PersistedRecord::Checkpoint(event) => {
                    let row = CheckpointRow {
                        checkpoint_timestamp_us: event.checkpoint_timestamp_us,
                        recv_timestamp_us: event.provenance.recv_timestamp_us,
                        exchange_timestamp_us: event.provenance.exchange_timestamp_us,
                        asset_id: event.asset_id.as_str().to_string(),
                        source: event.provenance.source.to_string(),
                        source_event_id: event.provenance.source_event_id.clone(),
                        source_session_id: event.provenance.source_session_id.clone(),
                        bids_json: serde_json::to_string(&event.bids)?,
                        asks_json: serde_json::to_string(&event.asks)?,
                    };
                    checkpoint_insert.write(&row).await?;
                }
                PersistedRecord::Validation(event) => {
                    let row = ReplayValidationRow {
                        asset_id: event.asset_id.as_str().to_string(),
                        mode: event.mode.to_string(),
                        replay_timestamp_us: event.replay_timestamp_us,
                        reference_timestamp_us: event.reference_timestamp_us,
                        matched: if event.matched { 1 } else { 0 },
                        mismatch_summary: event.mismatch_summary.clone(),
                        persisted_at_us: event.persisted_at_us,
                    };
                    validation_insert.write(&row).await?;
                }
                PersistedRecord::Execution(event) => {
                    let row = ExecutionEventRow {
                        event_timestamp_us: event.event_timestamp_us,
                        asset_id: event.asset_id.as_ref().map(|id| id.as_str().to_string()),
                        order_id: event.order_id.clone(),
                        client_order_id: event.client_order_id.clone(),
                        venue_order_id: event.venue_order_id.clone(),
                        event_kind: match event.kind {
                            ExecutionEventKind::SubmitIntent => "submit_intent".to_string(),
                            ExecutionEventKind::ExchangeAck => "exchange_ack".to_string(),
                            ExecutionEventKind::CancelRequest => "cancel_request".to_string(),
                            ExecutionEventKind::CancelAck => "cancel_ack".to_string(),
                            ExecutionEventKind::Reject => "reject".to_string(),
                            ExecutionEventKind::PartialFill => "partial_fill".to_string(),
                            ExecutionEventKind::Fill => "fill".to_string(),
                            ExecutionEventKind::Terminal => "terminal".to_string(),
                        },
                        side: side_to_string(event.side),
                        price: event.price.map(|price| price.raw()),
                        size: event.size.map(|size| size.raw()),
                        status: event.status.clone(),
                        reason: event.reason.clone(),
                        latency_json: serde_json::to_string(&event.latency)?,
                    };
                    execution_insert.write(&row).await?;
                }
            }
        }

        book_insert.end().await?;
        trade_insert.end().await?;
        ingest_insert.end().await?;
        checkpoint_insert.end().await?;
        validation_insert.end().await?;
        execution_insert.end().await?;

        pb_metrics::record_storage_flush("clickhouse");
        pb_metrics::record_flush_duration_ms(flush_start.elapsed().as_millis() as f64);
        tracing::debug!(rows = records.len(), "flushed batch to ClickHouse");
        Ok(())
    }
}
