use std::path::{Path, PathBuf};

use arrow::array::{Array, AsArray};
use arrow::datatypes::{UInt32Type, UInt64Type};
use parquet::arrow::async_reader::ParquetRecordBatchStreamBuilder;
use tokio::fs::File;
use tracing::debug;

use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, DataSource, EventProvenance, ExecutionEvent,
    ExecutionEventKind, IngestEvent, IngestEventKind, LatencyTrace, MarketDataWindow, ReplayMode,
    ReplayValidation, Side, TradeEvent,
};
use pb_types::{AssetId, FixedPrice, FixedSize, PriceLevel, Sequence, TradeFidelity};

use crate::error::ReplayError;

pub trait EventReader: Send + Sync {
    fn read_market_data(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<MarketDataWindow, ReplayError>> + Send;

    fn read_checkpoints(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<Vec<BookCheckpoint>, ReplayError>> + Send;

    fn read_latest_checkpoint(
        &self,
        asset_id: &AssetId,
        at_us: u64,
    ) -> impl std::future::Future<Output = Result<Option<BookCheckpoint>, ReplayError>> + Send;

    fn read_validations(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<Vec<ReplayValidation>, ReplayError>> + Send;

    fn read_execution_events(
        &self,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
    ) -> impl std::future::Future<Output = Result<Vec<ExecutionEvent>, ReplayError>> + Send;
}

pub struct ParquetReader {
    base_path: PathBuf,
}

impl ParquetReader {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    fn hour_paths(&self, dataset: &str, start_us: u64, end_us: u64) -> Vec<PathBuf> {
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
            paths.push(self.base_path.join(dataset).join(format!(
                "{}/{:02}/{:02}/{:02}",
                current.format("%Y"),
                current.format("%m"),
                current.format("%d"),
                current.format("%H"),
            )));
            current += chrono::Duration::hours(1);
        }
        paths
    }

    async fn dataset_files(
        &self,
        dataset: &str,
        asset_prefix: Option<&str>,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<PathBuf>, ReplayError> {
        let mut files = Vec::new();
        for dir in self.hour_paths(dataset, start_us, end_us) {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    debug!(?dir, "hour directory not found, skipping");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if !path
                    .extension()
                    .map(|ext| ext == "parquet")
                    .unwrap_or(false)
                {
                    continue;
                }
                if let Some(prefix) = asset_prefix {
                    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    let expected = format!("{prefix}_");
                    if !name.starts_with(&expected) {
                        continue;
                    }
                }
                files.push(path);
            }
        }
        Ok(files)
    }

    async fn read_parquet_file<T, F>(
        &self,
        path: &Path,
        extractor: F,
    ) -> Result<Vec<T>, ReplayError>
    where
        F: Fn(&arrow::record_batch::RecordBatch) -> Result<Vec<T>, ReplayError>,
    {
        let file = File::open(path).await?;
        let builder = ParquetRecordBatchStreamBuilder::new(file).await?;
        let mut stream = builder.build()?;
        let mut rows = Vec::new();

        use futures_util::StreamExt;
        while let Some(batch_result) = stream.next().await {
            let batch = batch_result?;
            rows.extend(extractor(&batch)?);
        }

        Ok(rows)
    }
}

fn parse_source(value: &str) -> Result<DataSource, ReplayError> {
    match value {
        "websocket" => Ok(DataSource::WebSocket),
        "rest_snapshot" => Ok(DataSource::RestSnapshot),
        "replay_validator" => Ok(DataSource::ReplayValidator),
        "strategy" => Ok(DataSource::Strategy),
        "exchange" => Ok(DataSource::Exchange),
        "system" => Ok(DataSource::System),
        other => Err(ReplayError::Other(format!("invalid source: {other}"))),
    }
}

fn parse_book_kind(value: u8) -> Result<BookEventKind, ReplayError> {
    match value {
        1 => Ok(BookEventKind::Snapshot),
        2 => Ok(BookEventKind::Delta),
        other => Err(ReplayError::InvalidEventType {
            raw: other.to_string(),
        }),
    }
}

fn parse_side_value(value: u8) -> Result<Side, ReplayError> {
    match value {
        1 => Ok(Side::Bid),
        2 => Ok(Side::Ask),
        other => Err(ReplayError::InvalidSide {
            raw: other.to_string(),
        }),
    }
}

fn parse_optional_side(value: Option<&str>) -> Result<Option<Side>, ReplayError> {
    match value {
        Some("Bid") => Ok(Some(Side::Bid)),
        Some("Ask") => Ok(Some(Side::Ask)),
        Some("bid") => Ok(Some(Side::Bid)),
        Some("ask") => Ok(Some(Side::Ask)),
        Some(other) => Err(ReplayError::InvalidSide {
            raw: other.to_string(),
        }),
        None => Ok(None),
    }
}

fn parse_trade_fidelity(value: &str) -> Result<TradeFidelity, ReplayError> {
    match value {
        "partial" => Ok(TradeFidelity::Partial),
        "full" => Ok(TradeFidelity::Full),
        other => Err(ReplayError::Other(format!(
            "invalid trade fidelity: {other}"
        ))),
    }
}

fn parse_ingest_kind(value: &str) -> Result<IngestEventKind, ReplayError> {
    match value {
        "reconnect_start" => Ok(IngestEventKind::ReconnectStart),
        "reconnect_success" => Ok(IngestEventKind::ReconnectSuccess),
        "sequence_gap" => Ok(IngestEventKind::SequenceGap),
        "stale_snapshot_skip" => Ok(IngestEventKind::StaleSnapshotSkip),
        "source_reset" => Ok(IngestEventKind::SourceReset),
        other => Err(ReplayError::Other(format!(
            "invalid ingest event kind: {other}"
        ))),
    }
}

fn parse_replay_mode(value: &str) -> Result<ReplayMode, ReplayError> {
    match value {
        "recv_time" => Ok(ReplayMode::RecvTime),
        "exchange_time" => Ok(ReplayMode::ExchangeTime),
        other => Err(ReplayError::Other(format!("invalid replay mode: {other}"))),
    }
}

fn parse_execution_kind(value: &str) -> Result<ExecutionEventKind, ReplayError> {
    match value {
        "submit_intent" => Ok(ExecutionEventKind::SubmitIntent),
        "exchange_ack" => Ok(ExecutionEventKind::ExchangeAck),
        "cancel_request" => Ok(ExecutionEventKind::CancelRequest),
        "cancel_ack" => Ok(ExecutionEventKind::CancelAck),
        "reject" => Ok(ExecutionEventKind::Reject),
        "partial_fill" => Ok(ExecutionEventKind::PartialFill),
        "fill" => Ok(ExecutionEventKind::Fill),
        "terminal" => Ok(ExecutionEventKind::Terminal),
        other => Err(ReplayError::Other(format!(
            "invalid execution event kind: {other}"
        ))),
    }
}

fn extract_book_events(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<BookEvent>, ReplayError> {
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
    let kind_col = batch
        .column_by_name("event_kind")
        .ok_or_else(|| ReplayError::Other("missing event_kind column".into()))?
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
    let source_col = batch
        .column_by_name("source")
        .ok_or_else(|| ReplayError::Other("missing source column".into()))?
        .as_string::<i32>();
    let source_event_id_col = batch
        .column_by_name("source_event_id")
        .ok_or_else(|| ReplayError::Other("missing source_event_id column".into()))?
        .as_string::<i32>();
    let source_session_id_col = batch
        .column_by_name("source_session_id")
        .ok_or_else(|| ReplayError::Other("missing source_session_id column".into()))?
        .as_string::<i32>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let recv_ts = recv_ts_col.value(i);
        if recv_ts < start_us || recv_ts > end_us {
            continue;
        }
        if asset_id_col.value(i) != asset_id.as_str() {
            continue;
        }
        rows.push(BookEvent {
            asset_id: AssetId::new(asset_id_col.value(i)),
            kind: parse_book_kind(kind_col.value(i))?,
            side: parse_side_value(side_col.value(i))?,
            price: FixedPrice::new(price_col.value(i))?,
            size: FixedSize::new(size_col.value(i)),
            provenance: EventProvenance {
                recv_timestamp_us: recv_ts,
                exchange_timestamp_us: exchange_ts_col.value(i),
                source: parse_source(source_col.value(i))?,
                source_event_id: if source_event_id_col.is_null(i) {
                    None
                } else {
                    Some(source_event_id_col.value(i).to_string())
                },
                source_session_id: if source_session_id_col.is_null(i) {
                    None
                } else {
                    Some(source_session_id_col.value(i).to_string())
                },
                sequence: if sequence_col.is_null(i) {
                    None
                } else {
                    Some(Sequence::new(sequence_col.value(i)))
                },
            },
        });
    }
    Ok(rows)
}

fn extract_trade_events(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<TradeEvent>, ReplayError> {
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
    let price_col = batch
        .column_by_name("price")
        .ok_or_else(|| ReplayError::Other("missing price column".into()))?
        .as_primitive::<UInt32Type>();
    let size_col = batch
        .column_by_name("size")
        .ok_or_else(|| ReplayError::Other("missing size column".into()))?
        .as_primitive::<UInt64Type>();
    let side_col = batch
        .column_by_name("side")
        .ok_or_else(|| ReplayError::Other("missing side column".into()))?
        .as_primitive::<arrow::datatypes::UInt8Type>();
    let trade_id_col = batch
        .column_by_name("trade_id")
        .ok_or_else(|| ReplayError::Other("missing trade_id column".into()))?
        .as_string::<i32>();
    let fidelity_col = batch
        .column_by_name("fidelity")
        .ok_or_else(|| ReplayError::Other("missing fidelity column".into()))?
        .as_string::<i32>();
    let sequence_col = batch
        .column_by_name("sequence")
        .ok_or_else(|| ReplayError::Other("missing sequence column".into()))?
        .as_primitive::<UInt64Type>();
    let source_col = batch
        .column_by_name("source")
        .ok_or_else(|| ReplayError::Other("missing source column".into()))?
        .as_string::<i32>();
    let source_event_id_col = batch
        .column_by_name("source_event_id")
        .ok_or_else(|| ReplayError::Other("missing source_event_id column".into()))?
        .as_string::<i32>();
    let source_session_id_col = batch
        .column_by_name("source_session_id")
        .ok_or_else(|| ReplayError::Other("missing source_session_id column".into()))?
        .as_string::<i32>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let recv_ts = recv_ts_col.value(i);
        if recv_ts < start_us || recv_ts > end_us {
            continue;
        }
        if asset_id_col.value(i) != asset_id.as_str() {
            continue;
        }
        let side = if side_col.is_null(i) {
            None
        } else {
            Some(parse_side_value(side_col.value(i))?)
        };
        rows.push(TradeEvent {
            asset_id: AssetId::new(asset_id_col.value(i)),
            price: FixedPrice::new(price_col.value(i))?,
            size: if size_col.is_null(i) {
                None
            } else {
                Some(FixedSize::new(size_col.value(i)))
            },
            side,
            trade_id: if trade_id_col.is_null(i) {
                None
            } else {
                Some(trade_id_col.value(i).to_string())
            },
            fidelity: parse_trade_fidelity(fidelity_col.value(i))?,
            provenance: EventProvenance {
                recv_timestamp_us: recv_ts,
                exchange_timestamp_us: exchange_ts_col.value(i),
                source: parse_source(source_col.value(i))?,
                source_event_id: if source_event_id_col.is_null(i) {
                    None
                } else {
                    Some(source_event_id_col.value(i).to_string())
                },
                source_session_id: if source_session_id_col.is_null(i) {
                    None
                } else {
                    Some(source_session_id_col.value(i).to_string())
                },
                sequence: if sequence_col.is_null(i) {
                    None
                } else {
                    Some(Sequence::new(sequence_col.value(i)))
                },
            },
        });
    }
    Ok(rows)
}

fn extract_ingest_events(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<IngestEvent>, ReplayError> {
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
    let kind_col = batch
        .column_by_name("event_kind")
        .ok_or_else(|| ReplayError::Other("missing event_kind column".into()))?
        .as_string::<i32>();
    let sequence_col = batch
        .column_by_name("sequence")
        .ok_or_else(|| ReplayError::Other("missing sequence column".into()))?
        .as_primitive::<UInt64Type>();
    let expected_col = batch
        .column_by_name("expected_sequence")
        .ok_or_else(|| ReplayError::Other("missing expected_sequence column".into()))?
        .as_primitive::<UInt64Type>();
    let observed_col = batch
        .column_by_name("observed_sequence")
        .ok_or_else(|| ReplayError::Other("missing observed_sequence column".into()))?
        .as_primitive::<UInt64Type>();
    let details_col = batch
        .column_by_name("details")
        .ok_or_else(|| ReplayError::Other("missing details column".into()))?
        .as_string::<i32>();
    let source_col = batch
        .column_by_name("source")
        .ok_or_else(|| ReplayError::Other("missing source column".into()))?
        .as_string::<i32>();
    let source_event_id_col = batch
        .column_by_name("source_event_id")
        .ok_or_else(|| ReplayError::Other("missing source_event_id column".into()))?
        .as_string::<i32>();
    let source_session_id_col = batch
        .column_by_name("source_session_id")
        .ok_or_else(|| ReplayError::Other("missing source_session_id column".into()))?
        .as_string::<i32>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let recv_ts = recv_ts_col.value(i);
        if recv_ts < start_us || recv_ts > end_us {
            continue;
        }
        let row_asset = if asset_id_col.is_null(i) {
            None
        } else {
            Some(AssetId::new(asset_id_col.value(i)))
        };
        if let Some(row_asset_id) = row_asset.as_ref() {
            if row_asset_id.as_str() != asset_id.as_str() {
                continue;
            }
        }
        rows.push(IngestEvent {
            asset_id: row_asset,
            kind: parse_ingest_kind(kind_col.value(i))?,
            provenance: EventProvenance {
                recv_timestamp_us: recv_ts,
                exchange_timestamp_us: exchange_ts_col.value(i),
                source: parse_source(source_col.value(i))?,
                source_event_id: if source_event_id_col.is_null(i) {
                    None
                } else {
                    Some(source_event_id_col.value(i).to_string())
                },
                source_session_id: if source_session_id_col.is_null(i) {
                    None
                } else {
                    Some(source_session_id_col.value(i).to_string())
                },
                sequence: if sequence_col.is_null(i) {
                    None
                } else {
                    Some(Sequence::new(sequence_col.value(i)))
                },
            },
            expected_sequence: if expected_col.is_null(i) {
                None
            } else {
                Some(expected_col.value(i))
            },
            observed_sequence: if observed_col.is_null(i) {
                None
            } else {
                Some(observed_col.value(i))
            },
            details: if details_col.is_null(i) {
                None
            } else {
                Some(details_col.value(i).to_string())
            },
        });
    }
    Ok(rows)
}

fn extract_checkpoints(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<BookCheckpoint>, ReplayError> {
    let checkpoint_ts_col = batch
        .column_by_name("checkpoint_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing checkpoint_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
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
    let source_col = batch
        .column_by_name("source")
        .ok_or_else(|| ReplayError::Other("missing source column".into()))?
        .as_string::<i32>();
    let source_event_id_col = batch
        .column_by_name("source_event_id")
        .ok_or_else(|| ReplayError::Other("missing source_event_id column".into()))?
        .as_string::<i32>();
    let source_session_id_col = batch
        .column_by_name("source_session_id")
        .ok_or_else(|| ReplayError::Other("missing source_session_id column".into()))?
        .as_string::<i32>();
    let bids_col = batch
        .column_by_name("bids_json")
        .ok_or_else(|| ReplayError::Other("missing bids_json column".into()))?
        .as_string::<i32>();
    let asks_col = batch
        .column_by_name("asks_json")
        .ok_or_else(|| ReplayError::Other("missing asks_json column".into()))?
        .as_string::<i32>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let checkpoint_ts = checkpoint_ts_col.value(i);
        if checkpoint_ts < start_us || checkpoint_ts > end_us {
            continue;
        }
        if asset_id_col.value(i) != asset_id.as_str() {
            continue;
        }
        rows.push(BookCheckpoint {
            asset_id: AssetId::new(asset_id_col.value(i)),
            checkpoint_timestamp_us: checkpoint_ts,
            provenance: EventProvenance {
                recv_timestamp_us: recv_ts_col.value(i),
                exchange_timestamp_us: exchange_ts_col.value(i),
                source: parse_source(source_col.value(i))?,
                source_event_id: if source_event_id_col.is_null(i) {
                    None
                } else {
                    Some(source_event_id_col.value(i).to_string())
                },
                source_session_id: if source_session_id_col.is_null(i) {
                    None
                } else {
                    Some(source_session_id_col.value(i).to_string())
                },
                sequence: None,
            },
            bids: serde_json::from_str::<Vec<PriceLevel>>(bids_col.value(i))?,
            asks: serde_json::from_str::<Vec<PriceLevel>>(asks_col.value(i))?,
        });
    }
    Ok(rows)
}

fn extract_validations(
    batch: &arrow::record_batch::RecordBatch,
    asset_id: &AssetId,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<ReplayValidation>, ReplayError> {
    let asset_id_col = batch
        .column_by_name("asset_id")
        .ok_or_else(|| ReplayError::Other("missing asset_id column".into()))?
        .as_string::<i32>();
    let mode_col = batch
        .column_by_name("mode")
        .ok_or_else(|| ReplayError::Other("missing mode column".into()))?
        .as_string::<i32>();
    let replay_ts_col = batch
        .column_by_name("replay_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing replay_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let reference_ts_col = batch
        .column_by_name("reference_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing reference_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let matched_col = batch
        .column_by_name("matched")
        .ok_or_else(|| ReplayError::Other("missing matched column".into()))?
        .as_boolean();
    let mismatch_col = batch
        .column_by_name("mismatch_summary")
        .ok_or_else(|| ReplayError::Other("missing mismatch_summary column".into()))?
        .as_string::<i32>();
    let persisted_col = batch
        .column_by_name("persisted_at_us")
        .ok_or_else(|| ReplayError::Other("missing persisted_at_us column".into()))?
        .as_primitive::<UInt64Type>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let persisted_at = persisted_col.value(i);
        if persisted_at < start_us || persisted_at > end_us {
            continue;
        }
        if asset_id_col.value(i) != asset_id.as_str() {
            continue;
        }
        rows.push(ReplayValidation {
            asset_id: AssetId::new(asset_id_col.value(i)),
            mode: parse_replay_mode(mode_col.value(i))?,
            replay_timestamp_us: replay_ts_col.value(i),
            reference_timestamp_us: reference_ts_col.value(i),
            matched: matched_col.value(i),
            mismatch_summary: if mismatch_col.is_null(i) {
                None
            } else {
                Some(mismatch_col.value(i).to_string())
            },
            persisted_at_us: persisted_at,
        });
    }
    Ok(rows)
}

fn extract_execution_events(
    batch: &arrow::record_batch::RecordBatch,
    order_id: Option<&str>,
    start_us: u64,
    end_us: u64,
) -> Result<Vec<ExecutionEvent>, ReplayError> {
    let event_ts_col = batch
        .column_by_name("event_timestamp_us")
        .ok_or_else(|| ReplayError::Other("missing event_timestamp_us column".into()))?
        .as_primitive::<UInt64Type>();
    let asset_id_col = batch
        .column_by_name("asset_id")
        .ok_or_else(|| ReplayError::Other("missing asset_id column".into()))?
        .as_string::<i32>();
    let order_id_col = batch
        .column_by_name("order_id")
        .ok_or_else(|| ReplayError::Other("missing order_id column".into()))?
        .as_string::<i32>();
    let client_order_id_col = batch
        .column_by_name("client_order_id")
        .ok_or_else(|| ReplayError::Other("missing client_order_id column".into()))?
        .as_string::<i32>();
    let venue_order_id_col = batch
        .column_by_name("venue_order_id")
        .ok_or_else(|| ReplayError::Other("missing venue_order_id column".into()))?
        .as_string::<i32>();
    let kind_col = batch
        .column_by_name("event_kind")
        .ok_or_else(|| ReplayError::Other("missing event_kind column".into()))?
        .as_string::<i32>();
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
    let status_col = batch
        .column_by_name("status")
        .ok_or_else(|| ReplayError::Other("missing status column".into()))?
        .as_string::<i32>();
    let reason_col = batch
        .column_by_name("reason")
        .ok_or_else(|| ReplayError::Other("missing reason column".into()))?
        .as_string::<i32>();
    let latency_col = batch
        .column_by_name("latency_json")
        .ok_or_else(|| ReplayError::Other("missing latency_json column".into()))?
        .as_string::<i32>();

    let mut rows = Vec::new();
    for i in 0..batch.num_rows() {
        let event_ts = event_ts_col.value(i);
        if event_ts < start_us || event_ts > end_us {
            continue;
        }
        if let Some(filter_order_id) = order_id {
            if order_id_col.value(i) != filter_order_id {
                continue;
            }
        }
        rows.push(ExecutionEvent {
            event_timestamp_us: event_ts,
            asset_id: if asset_id_col.is_null(i) {
                None
            } else {
                Some(AssetId::new(asset_id_col.value(i)))
            },
            order_id: order_id_col.value(i).to_string(),
            client_order_id: if client_order_id_col.is_null(i) {
                None
            } else {
                Some(client_order_id_col.value(i).to_string())
            },
            venue_order_id: if venue_order_id_col.is_null(i) {
                None
            } else {
                Some(venue_order_id_col.value(i).to_string())
            },
            kind: parse_execution_kind(kind_col.value(i))?,
            side: if side_col.is_null(i) {
                None
            } else {
                Some(parse_side_value(side_col.value(i))?)
            },
            price: if price_col.is_null(i) {
                None
            } else {
                Some(FixedPrice::new(price_col.value(i))?)
            },
            size: if size_col.is_null(i) {
                None
            } else {
                Some(FixedSize::new(size_col.value(i)))
            },
            status: if status_col.is_null(i) {
                None
            } else {
                Some(status_col.value(i).to_string())
            },
            reason: if reason_col.is_null(i) {
                None
            } else {
                Some(reason_col.value(i).to_string())
            },
            latency: serde_json::from_str::<LatencyTrace>(latency_col.value(i))?,
        });
    }
    Ok(rows)
}

impl EventReader for ParquetReader {
    async fn read_market_data(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<MarketDataWindow, ReplayError> {
        let book_files = self
            .dataset_files("book_events", Some(asset_id.as_str()), start_us, end_us)
            .await?;
        let trade_files = self
            .dataset_files("trade_events", Some(asset_id.as_str()), start_us, end_us)
            .await?;
        let ingest_files = self
            .dataset_files("ingest_events", None, start_us, end_us)
            .await?;

        let mut window = MarketDataWindow::default();
        for path in book_files {
            window.book_events.extend(
                self.read_parquet_file(&path, |batch| {
                    extract_book_events(batch, asset_id, start_us, end_us)
                })
                .await?,
            );
        }
        for path in trade_files {
            window.trade_events.extend(
                self.read_parquet_file(&path, |batch| {
                    extract_trade_events(batch, asset_id, start_us, end_us)
                })
                .await?,
            );
        }
        for path in ingest_files {
            window.ingest_events.extend(
                self.read_parquet_file(&path, |batch| {
                    extract_ingest_events(batch, asset_id, start_us, end_us)
                })
                .await?,
            );
        }
        Ok(window)
    }

    async fn read_checkpoints(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<BookCheckpoint>, ReplayError> {
        let files = self
            .dataset_files(
                "book_checkpoints",
                Some(asset_id.as_str()),
                start_us,
                end_us,
            )
            .await?;
        let mut checkpoints = Vec::new();
        for path in files {
            checkpoints.extend(
                self.read_parquet_file(&path, |batch| {
                    extract_checkpoints(batch, asset_id, start_us, end_us)
                })
                .await?,
            );
        }
        checkpoints.sort_by_key(|checkpoint| checkpoint.checkpoint_timestamp_us);
        Ok(checkpoints)
    }

    async fn read_latest_checkpoint(
        &self,
        asset_id: &AssetId,
        at_us: u64,
    ) -> Result<Option<BookCheckpoint>, ReplayError> {
        let mut checkpoints = self.read_checkpoints(asset_id, 0, at_us).await?;
        Ok(checkpoints.pop())
    }

    async fn read_validations(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<ReplayValidation>, ReplayError> {
        let files = self
            .dataset_files(
                "replay_validations",
                Some(asset_id.as_str()),
                start_us,
                end_us,
            )
            .await?;
        let mut validations = Vec::new();
        for path in files {
            validations.extend(
                self.read_parquet_file(&path, |batch| {
                    extract_validations(batch, asset_id, start_us, end_us)
                })
                .await?,
            );
        }
        validations.sort_by_key(|validation| validation.persisted_at_us);
        Ok(validations)
    }

    async fn read_execution_events(
        &self,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<ExecutionEvent>, ReplayError> {
        let files = self
            .dataset_files("execution_events", None, start_us, end_us)
            .await?;
        let mut events = Vec::new();
        for path in files {
            events.extend(
                self.read_parquet_file(&path, |batch| {
                    extract_execution_events(batch, order_id, start_us, end_us)
                })
                .await?,
            );
        }
        events.sort_by_key(|event| event.event_timestamp_us);
        Ok(events)
    }
}

pub struct ClickHouseReader {
    client: clickhouse::Client,
}

impl ClickHouseReader {
    pub fn new(url: &str, database: &str) -> Self {
        let client = clickhouse::Client::default()
            .with_url(url)
            .with_database(database);
        Self { client }
    }
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
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

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
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

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
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

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
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

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
struct ReplayValidationRow {
    asset_id: String,
    mode: String,
    replay_timestamp_us: u64,
    reference_timestamp_us: u64,
    matched: u8,
    mismatch_summary: Option<String>,
    persisted_at_us: u64,
}

#[derive(Debug, clickhouse::Row, serde::Deserialize)]
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

impl EventReader for ClickHouseReader {
    async fn read_market_data(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<MarketDataWindow, ReplayError> {
        let book_query = "SELECT recv_timestamp_us, exchange_timestamp_us, asset_id, event_kind, side, price, size, sequence, source, source_event_id, source_session_id FROM book_events WHERE asset_id = ? AND recv_timestamp_us >= ? AND recv_timestamp_us <= ?";
        let trade_query = "SELECT recv_timestamp_us, exchange_timestamp_us, asset_id, price, size, side, trade_id, fidelity, sequence, source, source_event_id, source_session_id FROM trade_events WHERE asset_id = ? AND recv_timestamp_us >= ? AND recv_timestamp_us <= ?";
        let ingest_query = "SELECT recv_timestamp_us, exchange_timestamp_us, asset_id, event_kind, sequence, expected_sequence, observed_sequence, details, source, source_event_id, source_session_id FROM ingest_events WHERE recv_timestamp_us >= ? AND recv_timestamp_us <= ?";

        let book_rows: Vec<BookEventRow> = self
            .client
            .query(book_query)
            .bind(asset_id.as_str())
            .bind(start_us)
            .bind(end_us)
            .fetch_all()
            .await?;
        let trade_rows: Vec<TradeEventRow> = self
            .client
            .query(trade_query)
            .bind(asset_id.as_str())
            .bind(start_us)
            .bind(end_us)
            .fetch_all()
            .await?;
        let ingest_rows: Vec<IngestEventRow> = self
            .client
            .query(ingest_query)
            .bind(start_us)
            .bind(end_us)
            .fetch_all()
            .await?;

        let book_events = book_rows
            .into_iter()
            .map(|row| {
                Ok(BookEvent {
                    asset_id: AssetId::new(row.asset_id),
                    kind: match row.event_kind.as_str() {
                        "Snapshot" => BookEventKind::Snapshot,
                        "Delta" => BookEventKind::Delta,
                        other => {
                            return Err(ReplayError::InvalidEventType {
                                raw: other.to_string(),
                            })
                        }
                    },
                    side: parse_optional_side(Some(row.side.as_str()))?.unwrap(),
                    price: FixedPrice::new(row.price)?,
                    size: FixedSize::new(row.size),
                    provenance: EventProvenance {
                        recv_timestamp_us: row.recv_timestamp_us,
                        exchange_timestamp_us: row.exchange_timestamp_us,
                        source: parse_source(&row.source)?,
                        source_event_id: row.source_event_id,
                        source_session_id: row.source_session_id,
                        sequence: row.sequence.map(Sequence::new),
                    },
                })
            })
            .collect::<Result<Vec<_>, ReplayError>>()?;
        let trade_events = trade_rows
            .into_iter()
            .map(|row| {
                Ok(TradeEvent {
                    asset_id: AssetId::new(row.asset_id),
                    price: FixedPrice::new(row.price)?,
                    size: row.size.map(FixedSize::new),
                    side: parse_optional_side(row.side.as_deref())?,
                    trade_id: row.trade_id,
                    fidelity: parse_trade_fidelity(&row.fidelity)?,
                    provenance: EventProvenance {
                        recv_timestamp_us: row.recv_timestamp_us,
                        exchange_timestamp_us: row.exchange_timestamp_us,
                        source: parse_source(&row.source)?,
                        source_event_id: row.source_event_id,
                        source_session_id: row.source_session_id,
                        sequence: row.sequence.map(Sequence::new),
                    },
                })
            })
            .collect::<Result<Vec<_>, ReplayError>>()?;
        let ingest_events = ingest_rows
            .into_iter()
            .filter(|row| {
                row.asset_id.as_deref().is_none()
                    || row.asset_id.as_deref() == Some(asset_id.as_str())
            })
            .map(|row| {
                Ok(IngestEvent {
                    asset_id: row.asset_id.map(AssetId::new),
                    kind: parse_ingest_kind(&row.event_kind)?,
                    provenance: EventProvenance {
                        recv_timestamp_us: row.recv_timestamp_us,
                        exchange_timestamp_us: row.exchange_timestamp_us,
                        source: parse_source(&row.source)?,
                        source_event_id: row.source_event_id,
                        source_session_id: row.source_session_id,
                        sequence: row.sequence.map(Sequence::new),
                    },
                    expected_sequence: row.expected_sequence,
                    observed_sequence: row.observed_sequence,
                    details: row.details,
                })
            })
            .collect::<Result<Vec<_>, ReplayError>>()?;

        Ok(MarketDataWindow {
            book_events,
            trade_events,
            ingest_events,
        })
    }

    async fn read_checkpoints(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<BookCheckpoint>, ReplayError> {
        let query = "SELECT checkpoint_timestamp_us, recv_timestamp_us, exchange_timestamp_us, asset_id, source, source_event_id, source_session_id, bids_json, asks_json FROM book_checkpoints WHERE asset_id = ? AND checkpoint_timestamp_us >= ? AND checkpoint_timestamp_us <= ? ORDER BY checkpoint_timestamp_us";
        let rows: Vec<CheckpointRow> = self
            .client
            .query(query)
            .bind(asset_id.as_str())
            .bind(start_us)
            .bind(end_us)
            .fetch_all()
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(BookCheckpoint {
                    asset_id: AssetId::new(row.asset_id),
                    checkpoint_timestamp_us: row.checkpoint_timestamp_us,
                    provenance: EventProvenance {
                        recv_timestamp_us: row.recv_timestamp_us,
                        exchange_timestamp_us: row.exchange_timestamp_us,
                        source: parse_source(&row.source)?,
                        source_event_id: row.source_event_id,
                        source_session_id: row.source_session_id,
                        sequence: None,
                    },
                    bids: serde_json::from_str(&row.bids_json)?,
                    asks: serde_json::from_str(&row.asks_json)?,
                })
            })
            .collect()
    }

    async fn read_latest_checkpoint(
        &self,
        asset_id: &AssetId,
        at_us: u64,
    ) -> Result<Option<BookCheckpoint>, ReplayError> {
        let query = "SELECT checkpoint_timestamp_us, recv_timestamp_us, exchange_timestamp_us, asset_id, source, source_event_id, source_session_id, bids_json, asks_json FROM book_checkpoints WHERE asset_id = ? AND checkpoint_timestamp_us <= ? ORDER BY checkpoint_timestamp_us DESC LIMIT 1";
        let row: Option<CheckpointRow> = self
            .client
            .query(query)
            .bind(asset_id.as_str())
            .bind(at_us)
            .fetch_optional()
            .await?;
        row.map(|row| {
            Ok(BookCheckpoint {
                asset_id: AssetId::new(row.asset_id),
                checkpoint_timestamp_us: row.checkpoint_timestamp_us,
                provenance: EventProvenance {
                    recv_timestamp_us: row.recv_timestamp_us,
                    exchange_timestamp_us: row.exchange_timestamp_us,
                    source: parse_source(&row.source)?,
                    source_event_id: row.source_event_id,
                    source_session_id: row.source_session_id,
                    sequence: None,
                },
                bids: serde_json::from_str(&row.bids_json)?,
                asks: serde_json::from_str(&row.asks_json)?,
            })
        })
        .transpose()
    }

    async fn read_validations(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<ReplayValidation>, ReplayError> {
        let query = "SELECT asset_id, mode, replay_timestamp_us, reference_timestamp_us, matched, mismatch_summary, persisted_at_us FROM replay_validations WHERE asset_id = ? AND persisted_at_us >= ? AND persisted_at_us <= ? ORDER BY persisted_at_us";
        let rows: Vec<ReplayValidationRow> = self
            .client
            .query(query)
            .bind(asset_id.as_str())
            .bind(start_us)
            .bind(end_us)
            .fetch_all()
            .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ReplayValidation {
                    asset_id: AssetId::new(row.asset_id),
                    mode: parse_replay_mode(&row.mode)?,
                    replay_timestamp_us: row.replay_timestamp_us,
                    reference_timestamp_us: row.reference_timestamp_us,
                    matched: row.matched > 0,
                    mismatch_summary: row.mismatch_summary,
                    persisted_at_us: row.persisted_at_us,
                })
            })
            .collect()
    }

    async fn read_execution_events(
        &self,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<ExecutionEvent>, ReplayError> {
        let base_query = "SELECT event_timestamp_us, asset_id, order_id, client_order_id, venue_order_id, event_kind, side, price, size, status, reason, latency_json FROM execution_events WHERE event_timestamp_us >= ? AND event_timestamp_us <= ?";
        let query = if order_id.is_some() {
            format!("{base_query} AND order_id = ? ORDER BY event_timestamp_us")
        } else {
            format!("{base_query} ORDER BY event_timestamp_us")
        };

        let mut request = self.client.query(&query).bind(start_us).bind(end_us);
        if let Some(order_id) = order_id {
            request = request.bind(order_id);
        }
        let rows: Vec<ExecutionEventRow> = request.fetch_all().await?;
        rows.into_iter()
            .map(|row| {
                Ok(ExecutionEvent {
                    event_timestamp_us: row.event_timestamp_us,
                    asset_id: row.asset_id.map(AssetId::new),
                    order_id: row.order_id,
                    client_order_id: row.client_order_id,
                    venue_order_id: row.venue_order_id,
                    kind: parse_execution_kind(&row.event_kind)?,
                    side: parse_optional_side(row.side.as_deref())?,
                    price: row.price.map(FixedPrice::new).transpose()?,
                    size: row.size.map(FixedSize::new),
                    status: row.status,
                    reason: row.reason,
                    latency: serde_json::from_str(&row.latency_json)?,
                })
            })
            .collect()
    }
}
