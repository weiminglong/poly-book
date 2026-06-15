use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Datelike, Timelike};
use clickhouse::Client;
use object_store::path::Path as ObjectPath;
use object_store::ObjectStore;
use object_store::ObjectStoreExt;
use object_store::PutPayload;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding, ZstdLevel};
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
    sequence UInt64,
    source String,
    source_event_id Nullable(String),
    source_session_id Nullable(String),
    ingest_ordinal Nullable(UInt64),
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
ORDER BY (asset_id, recv_timestamp_us)
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
ORDER BY (recv_timestamp_us, event_kind)
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
    wal_offset Nullable(UInt64),
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
-- Lead with event_timestamp_us: the execution timeline always filters by a time
-- range (order_id is optional), so this matches the dominant query and avoids a
-- full scan on time-range lookups (audit finding A.38, clickhouse rule
-- schema-pk-prioritize-filters).
ORDER BY (event_timestamp_us, order_id)
"#;

#[derive(Clone)]
pub struct ParquetRecordWriter {
    store: Arc<dyn ObjectStore>,
    base_path: String,
}

/// Lower bound for a plausible event timestamp (~2001-09-09 in µs). Anything
/// below this is treated as corrupt/unstamped rather than a real 1970s event.
const MIN_PLAUSIBLE_PARTITION_US: u64 = 1_000_000_000_000_000;
/// Upper bound for a plausible event timestamp (~2286 in µs). Guards against an
/// absurd far-future value (e.g. a u64 that overflows i64) misfiling records.
const MAX_PLAUSIBLE_PARTITION_US: u64 = 10_000_000_000_000_000;

/// Build the `YYYY/MM/DD/HH` partition key for a record timestamp.
///
/// Timestamps outside a wide plausible band — or not representable as a datetime
/// — are routed to a dedicated `invalid_timestamp` partition with a warning,
/// instead of being silently misfiled into the 1970-01-01 partition by
/// `unwrap_or_default()` (audit finding A.123). This keeps corrupt/unstamped
/// records visible and quarantined rather than corrupting a real date partition.
pub(crate) fn partition_hour_key(partition_ts_us: u64) -> String {
    if !(MIN_PLAUSIBLE_PARTITION_US..=MAX_PLAUSIBLE_PARTITION_US).contains(&partition_ts_us) {
        tracing::warn!(
            partition_ts_us,
            "record timestamp outside plausible range; routing to invalid_timestamp partition"
        );
        return "invalid_timestamp".to_string();
    }
    match chrono::DateTime::from_timestamp_micros(partition_ts_us as i64) {
        Some(dt) => format!(
            "{:04}/{:02}/{:02}/{:02}",
            dt.date_naive().year(),
            dt.date_naive().month(),
            dt.date_naive().day(),
            dt.time().hour(),
        ),
        None => {
            tracing::warn!(
                partition_ts_us,
                "record timestamp not representable; routing to invalid_timestamp partition"
            );
            "invalid_timestamp".to_string()
        }
    }
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
            let hour_key = partition_hour_key(record.partition_timestamp_us());
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

            let batch = records_to_record_batch(records)?;
            let schema = Arc::new(schema_for_record(records[0]));
            let props = WriterProperties::builder()
                .set_compression(Compression::ZSTD(
                    ZstdLevel::try_new(3).expect("valid zstd level"),
                ))
                .set_max_row_group_row_count(Some(ROW_GROUP_SIZE))
                .set_column_encoding("recv_timestamp_us".into(), Encoding::DELTA_BINARY_PACKED)
                .set_column_encoding(
                    "exchange_timestamp_us".into(),
                    Encoding::DELTA_BINARY_PACKED,
                )
                .set_column_encoding(
                    "checkpoint_timestamp_us".into(),
                    Encoding::DELTA_BINARY_PACKED,
                )
                .set_column_encoding("event_timestamp_us".into(), Encoding::DELTA_BINARY_PACKED)
                .set_column_encoding("sequence".into(), Encoding::DELTA_BINARY_PACKED)
                .set_column_encoding("price".into(), Encoding::DELTA_BINARY_PACKED)
                .set_column_encoding("size".into(), Encoding::DELTA_BINARY_PACKED)
                .build();

            let mut buf = Vec::with_capacity(256 * 1024);
            let mut writer = ArrowWriter::try_new(&mut buf, schema, Some(props))?;
            writer.write(&batch)?;
            writer.close()?;

            // Append a content-derived suffix so two batches that land in the
            // same (asset, hour) bucket with the same first-record timestamp
            // (quiet books, checkpoints, execution-append re-runs) do not
            // silently overwrite each other (A.122). Identical content hashes to
            // the same name, making a true retry idempotent.
            let content_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                buf.hash(&mut hasher);
                hasher.finish()
            };
            let path = format!(
                "{}/{}/{}/{}_{}_{:016x}.parquet",
                self.base_path, dataset, hour_key, asset, first_ts_us, content_hash
            );

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
    // Enum8 columns are serialized as their i8 discriminant over RowBinary;
    // sending a Rust String here is rejected by ClickHouse (audit finding A.4).
    event_kind: i8,
    side: i8,
    price: u32,
    size: u64,
    // Non-nullable so it can stay in the sorting key without allow_nullable_key
    // (audit finding A.3). Book events always carry a sequence; 0 if absent.
    sequence: u64,
    source: String,
    source_event_id: Option<String>,
    source_session_id: Option<String>,
    // Monotonic ingest ordinal — replay's authoritative arrival-order tiebreaker
    // (A.116). Nullable for rows written before this column existed.
    ingest_ordinal: Option<u64>,
}

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct TradeEventRow {
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: String,
    price: u32,
    size: Option<u64>,
    side: Option<i8>,
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
    wal_offset: Option<u64>,
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
    side: Option<i8>,
    price: Option<u32>,
    size: Option<u64>,
    status: Option<String>,
    reason: Option<String>,
    latency_json: String,
}

/// Map a side to its `Enum8('Bid' = 1, 'Ask' = 2)` discriminant for RowBinary.
fn side_to_i8(side: Side) -> i8 {
    match side {
        Side::Bid => 1,
        Side::Ask => 2,
    }
}

fn opt_side_to_i8(side: Option<Side>) -> Option<i8> {
    side.map(side_to_i8)
}

/// Map a book event kind to its `Enum8('Snapshot' = 1, 'Delta' = 2)` discriminant.
fn book_kind_to_i8(kind: BookEventKind) -> i8 {
    match kind {
        BookEventKind::Snapshot => 1,
        BookEventKind::Delta => 2,
    }
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

        // Check which event types are present to avoid opening unused insert handles.
        let mut has_book = false;
        let mut has_trade = false;
        let mut has_ingest = false;
        let mut has_checkpoint = false;
        let mut has_validation = false;
        let mut has_execution = false;
        for record in records {
            match record {
                PersistedRecord::Book(_) => has_book = true,
                PersistedRecord::Trade(_) => has_trade = true,
                PersistedRecord::Ingest(_) => has_ingest = true,
                PersistedRecord::Checkpoint(_) => has_checkpoint = true,
                PersistedRecord::Validation(_) => has_validation = true,
                PersistedRecord::Execution(_) => has_execution = true,
            }
        }

        let mut book_insert: Option<clickhouse::insert::Insert<BookEventRow>> = if has_book {
            Some(self.client.insert("book_events").await?)
        } else {
            None
        };
        let mut trade_insert: Option<clickhouse::insert::Insert<TradeEventRow>> = if has_trade {
            Some(self.client.insert("trade_events").await?)
        } else {
            None
        };
        let mut ingest_insert: Option<clickhouse::insert::Insert<IngestEventRow>> = if has_ingest {
            Some(self.client.insert("ingest_events").await?)
        } else {
            None
        };
        let mut checkpoint_insert: Option<clickhouse::insert::Insert<CheckpointRow>> =
            if has_checkpoint {
                Some(self.client.insert("book_checkpoints").await?)
            } else {
                None
            };
        let mut validation_insert: Option<clickhouse::insert::Insert<ReplayValidationRow>> =
            if has_validation {
                Some(self.client.insert("replay_validations").await?)
            } else {
                None
            };
        let mut execution_insert: Option<clickhouse::insert::Insert<ExecutionEventRow>> =
            if has_execution {
                Some(self.client.insert("execution_events").await?)
            } else {
                None
            };

        for record in records {
            match record {
                PersistedRecord::Book(event) => {
                    let row = BookEventRow {
                        recv_timestamp_us: event.provenance.recv_timestamp_us,
                        exchange_timestamp_us: event.provenance.exchange_timestamp_us,
                        asset_id: event.asset_id.as_str().to_string(),
                        event_kind: book_kind_to_i8(event.kind),
                        side: side_to_i8(event.side),
                        price: event.price.raw(),
                        size: event.size.raw(),
                        sequence: event.provenance.sequence.map_or(0, |seq| seq.raw()),
                        source: event.provenance.source.to_string(),
                        source_event_id: event.provenance.source_event_id.clone(),
                        source_session_id: event.provenance.source_session_id.clone(),
                        ingest_ordinal: event.provenance.ingest_ordinal,
                    };
                    book_insert.as_mut().unwrap().write(&row).await?;
                }
                PersistedRecord::Trade(event) => {
                    let row = TradeEventRow {
                        recv_timestamp_us: event.provenance.recv_timestamp_us,
                        exchange_timestamp_us: event.provenance.exchange_timestamp_us,
                        asset_id: event.asset_id.as_str().to_string(),
                        price: event.price.raw(),
                        size: event.size.map(|size| size.raw()),
                        side: opt_side_to_i8(event.side),
                        trade_id: event.trade_id.clone(),
                        fidelity: event.fidelity.to_string(),
                        sequence: event.provenance.sequence.map(|seq| seq.raw()),
                        source: event.provenance.source.to_string(),
                        source_event_id: event.provenance.source_event_id.clone(),
                        source_session_id: event.provenance.source_session_id.clone(),
                    };
                    trade_insert.as_mut().unwrap().write(&row).await?;
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
                    ingest_insert.as_mut().unwrap().write(&row).await?;
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
                        wal_offset: event.wal_offset,
                    };
                    checkpoint_insert.as_mut().unwrap().write(&row).await?;
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
                    validation_insert.as_mut().unwrap().write(&row).await?;
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
                        side: opt_side_to_i8(event.side),
                        price: event.price.map(|price| price.raw()),
                        size: event.size.map(|size| size.raw()),
                        status: event.status.clone(),
                        reason: event.reason.clone(),
                        latency_json: serde_json::to_string(&event.latency)?,
                    };
                    execution_insert.as_mut().unwrap().write(&row).await?;
                }
            }
        }

        if let Some(insert) = book_insert {
            insert.end().await?;
        }
        if let Some(insert) = trade_insert {
            insert.end().await?;
        }
        if let Some(insert) = ingest_insert {
            insert.end().await?;
        }
        if let Some(insert) = checkpoint_insert {
            insert.end().await?;
        }
        if let Some(insert) = validation_insert {
            insert.end().await?;
        }
        if let Some(insert) = execution_insert {
            insert.end().await?;
        }

        pb_metrics::record_storage_flush("clickhouse");
        pb_metrics::record_flush_duration_ms(flush_start.elapsed().as_millis() as f64);
        tracing::debug!(rows = records.len(), "flushed batch to ClickHouse");
        Ok(())
    }
}
