use std::sync::Arc;

use arrow::array::{ArrayRef, BooleanArray, StringArray, UInt32Array, UInt64Array, UInt8Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, ExecutionEvent, IngestEvent, PersistedRecord,
    ReplayValidation, Side, TradeEvent,
};

use crate::error::StoreError;

pub fn book_event_schema() -> Schema {
    Schema::new(vec![
        Field::new("recv_timestamp_us", DataType::UInt64, false),
        Field::new("exchange_timestamp_us", DataType::UInt64, false),
        Field::new("asset_id", DataType::Utf8, false),
        Field::new("event_kind", DataType::UInt8, false),
        Field::new("side", DataType::UInt8, false),
        Field::new("price", DataType::UInt32, false),
        Field::new("size", DataType::UInt64, false),
        Field::new("sequence", DataType::UInt64, true),
        Field::new("source", DataType::Utf8, false),
        Field::new("source_event_id", DataType::Utf8, true),
        Field::new("source_session_id", DataType::Utf8, true),
    ])
}

pub fn trade_event_schema() -> Schema {
    Schema::new(vec![
        Field::new("recv_timestamp_us", DataType::UInt64, false),
        Field::new("exchange_timestamp_us", DataType::UInt64, false),
        Field::new("asset_id", DataType::Utf8, false),
        Field::new("price", DataType::UInt32, false),
        Field::new("size", DataType::UInt64, true),
        Field::new("side", DataType::UInt8, true),
        Field::new("trade_id", DataType::Utf8, true),
        Field::new("fidelity", DataType::Utf8, false),
        Field::new("sequence", DataType::UInt64, true),
        Field::new("source", DataType::Utf8, false),
        Field::new("source_event_id", DataType::Utf8, true),
        Field::new("source_session_id", DataType::Utf8, true),
    ])
}

pub fn ingest_event_schema() -> Schema {
    Schema::new(vec![
        Field::new("recv_timestamp_us", DataType::UInt64, false),
        Field::new("exchange_timestamp_us", DataType::UInt64, false),
        Field::new("asset_id", DataType::Utf8, true),
        Field::new("event_kind", DataType::Utf8, false),
        Field::new("sequence", DataType::UInt64, true),
        Field::new("expected_sequence", DataType::UInt64, true),
        Field::new("observed_sequence", DataType::UInt64, true),
        Field::new("details", DataType::Utf8, true),
        Field::new("source", DataType::Utf8, false),
        Field::new("source_event_id", DataType::Utf8, true),
        Field::new("source_session_id", DataType::Utf8, true),
    ])
}

pub fn checkpoint_schema() -> Schema {
    Schema::new(vec![
        Field::new("checkpoint_timestamp_us", DataType::UInt64, false),
        Field::new("recv_timestamp_us", DataType::UInt64, false),
        Field::new("exchange_timestamp_us", DataType::UInt64, false),
        Field::new("asset_id", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("source_event_id", DataType::Utf8, true),
        Field::new("source_session_id", DataType::Utf8, true),
        Field::new("bids_json", DataType::Utf8, false),
        Field::new("asks_json", DataType::Utf8, false),
    ])
}

pub fn replay_validation_schema() -> Schema {
    Schema::new(vec![
        Field::new("asset_id", DataType::Utf8, false),
        Field::new("mode", DataType::Utf8, false),
        Field::new("replay_timestamp_us", DataType::UInt64, false),
        Field::new("reference_timestamp_us", DataType::UInt64, false),
        Field::new("matched", DataType::Boolean, false),
        Field::new("mismatch_summary", DataType::Utf8, true),
        Field::new("persisted_at_us", DataType::UInt64, false),
    ])
}

pub fn execution_event_schema() -> Schema {
    Schema::new(vec![
        Field::new("event_timestamp_us", DataType::UInt64, false),
        Field::new("asset_id", DataType::Utf8, true),
        Field::new("order_id", DataType::Utf8, false),
        Field::new("client_order_id", DataType::Utf8, true),
        Field::new("venue_order_id", DataType::Utf8, true),
        Field::new("event_kind", DataType::Utf8, false),
        Field::new("side", DataType::UInt8, true),
        Field::new("price", DataType::UInt32, true),
        Field::new("size", DataType::UInt64, true),
        Field::new("status", DataType::Utf8, true),
        Field::new("reason", DataType::Utf8, true),
        Field::new("latency_json", DataType::Utf8, false),
    ])
}

fn book_kind_to_u8(kind: BookEventKind) -> u8 {
    match kind {
        BookEventKind::Snapshot => 1,
        BookEventKind::Delta => 2,
    }
}

fn side_to_u8(side: Option<Side>) -> Option<u8> {
    side.map(|value| match value {
        Side::Bid => 1,
        Side::Ask => 2,
    })
}

pub fn records_to_record_batch(records: &[&PersistedRecord]) -> Result<RecordBatch, StoreError> {
    let Some(first) = records.first() else {
        return Err(StoreError::Other(
            "cannot build record batch from empty record list".into(),
        ));
    };
    match first {
        PersistedRecord::Book(_) => {
            let values = records
                .iter()
                .map(|record| match record {
                    PersistedRecord::Book(event) => Ok(event),
                    _ => Err(StoreError::Other("mixed dataset batch".into())),
                })
                .collect::<Result<Vec<_>, _>>()?;
            book_event_refs_to_record_batch(&values)
        }
        PersistedRecord::Trade(_) => {
            let values = records
                .iter()
                .map(|record| match record {
                    PersistedRecord::Trade(event) => Ok(event),
                    _ => Err(StoreError::Other("mixed dataset batch".into())),
                })
                .collect::<Result<Vec<_>, _>>()?;
            trade_event_refs_to_record_batch(&values)
        }
        PersistedRecord::Ingest(_) => {
            let values = records
                .iter()
                .map(|record| match record {
                    PersistedRecord::Ingest(event) => Ok(event),
                    _ => Err(StoreError::Other("mixed dataset batch".into())),
                })
                .collect::<Result<Vec<_>, _>>()?;
            ingest_event_refs_to_record_batch(&values)
        }
        PersistedRecord::Checkpoint(_) => {
            let values = records
                .iter()
                .map(|record| match record {
                    PersistedRecord::Checkpoint(event) => Ok(event),
                    _ => Err(StoreError::Other("mixed dataset batch".into())),
                })
                .collect::<Result<Vec<_>, _>>()?;
            checkpoint_refs_to_record_batch(&values)
        }
        PersistedRecord::Validation(_) => {
            let values = records
                .iter()
                .map(|record| match record {
                    PersistedRecord::Validation(event) => Ok(event),
                    _ => Err(StoreError::Other("mixed dataset batch".into())),
                })
                .collect::<Result<Vec<_>, _>>()?;
            replay_validation_refs_to_record_batch(&values)
        }
        PersistedRecord::Execution(_) => {
            let values = records
                .iter()
                .map(|record| match record {
                    PersistedRecord::Execution(event) => Ok(event),
                    _ => Err(StoreError::Other("mixed dataset batch".into())),
                })
                .collect::<Result<Vec<_>, _>>()?;
            execution_event_refs_to_record_batch(&values)
        }
    }
}

pub fn schema_for_record(record: &PersistedRecord) -> Schema {
    match record {
        PersistedRecord::Book(_) => book_event_schema(),
        PersistedRecord::Trade(_) => trade_event_schema(),
        PersistedRecord::Ingest(_) => ingest_event_schema(),
        PersistedRecord::Checkpoint(_) => checkpoint_schema(),
        PersistedRecord::Validation(_) => replay_validation_schema(),
        PersistedRecord::Execution(_) => execution_event_schema(),
    }
}

pub fn book_event_refs_to_record_batch(events: &[&BookEvent]) -> Result<RecordBatch, StoreError> {
    let schema = Arc::new(book_event_schema());
    let recv_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.provenance.recv_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let exchange_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.provenance.exchange_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let asset_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.asset_id.as_str())
            .collect::<Vec<_>>(),
    ));
    let kinds: ArrayRef = Arc::new(UInt8Array::from(
        events
            .iter()
            .map(|e| book_kind_to_u8(e.kind))
            .collect::<Vec<_>>(),
    ));
    let sides: ArrayRef = Arc::new(UInt8Array::from(
        events
            .iter()
            .map(|e| side_to_u8(Some(e.side)))
            .collect::<Vec<_>>(),
    ));
    let prices: ArrayRef = Arc::new(UInt32Array::from(
        events.iter().map(|e| e.price.raw()).collect::<Vec<_>>(),
    ));
    let sizes: ArrayRef = Arc::new(UInt64Array::from(
        events.iter().map(|e| e.size.raw()).collect::<Vec<_>>(),
    ));
    let sequences: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.provenance.sequence.map(|seq| seq.raw()))
            .collect::<Vec<_>>(),
    ));
    let sources: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.provenance.source.to_string())
            .collect::<Vec<_>>(),
    ));
    let source_event_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.provenance.source_event_id.as_deref())
            .collect::<Vec<_>>(),
    ));
    let source_session_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.provenance.source_session_id.as_deref())
            .collect::<Vec<_>>(),
    ));

    RecordBatch::try_new(
        schema,
        vec![
            recv_ts,
            exchange_ts,
            asset_ids,
            kinds,
            sides,
            prices,
            sizes,
            sequences,
            sources,
            source_event_ids,
            source_session_ids,
        ],
    )
    .map_err(StoreError::from)
}

pub fn trade_event_refs_to_record_batch(events: &[&TradeEvent]) -> Result<RecordBatch, StoreError> {
    let schema = Arc::new(trade_event_schema());
    let recv_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.provenance.recv_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let exchange_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.provenance.exchange_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let asset_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.asset_id.as_str())
            .collect::<Vec<_>>(),
    ));
    let prices: ArrayRef = Arc::new(UInt32Array::from(
        events.iter().map(|e| e.price.raw()).collect::<Vec<_>>(),
    ));
    let sizes: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.size.map(|size| size.raw()))
            .collect::<Vec<_>>(),
    ));
    let sides: ArrayRef = Arc::new(UInt8Array::from(
        events
            .iter()
            .map(|e| side_to_u8(e.side))
            .collect::<Vec<_>>(),
    ));
    let trade_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.trade_id.as_deref())
            .collect::<Vec<_>>(),
    ));
    let fidelity: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.fidelity.to_string())
            .collect::<Vec<_>>(),
    ));
    let sequences: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.provenance.sequence.map(|seq| seq.raw()))
            .collect::<Vec<_>>(),
    ));
    let sources: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.provenance.source.to_string())
            .collect::<Vec<_>>(),
    ));
    let source_event_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.provenance.source_event_id.as_deref())
            .collect::<Vec<_>>(),
    ));
    let source_session_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.provenance.source_session_id.as_deref())
            .collect::<Vec<_>>(),
    ));

    RecordBatch::try_new(
        schema,
        vec![
            recv_ts,
            exchange_ts,
            asset_ids,
            prices,
            sizes,
            sides,
            trade_ids,
            fidelity,
            sequences,
            sources,
            source_event_ids,
            source_session_ids,
        ],
    )
    .map_err(StoreError::from)
}

pub fn ingest_event_refs_to_record_batch(
    events: &[&IngestEvent],
) -> Result<RecordBatch, StoreError> {
    let schema = Arc::new(ingest_event_schema());
    let recv_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.provenance.recv_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let exchange_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.provenance.exchange_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let asset_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.asset_id.as_ref().map(|id| id.as_str()))
            .collect::<Vec<_>>(),
    ));
    let kinds: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.kind.to_string())
            .collect::<Vec<_>>(),
    ));
    let sequences: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.provenance.sequence.map(|seq| seq.raw()))
            .collect::<Vec<_>>(),
    ));
    let expected_sequences: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.expected_sequence)
            .collect::<Vec<_>>(),
    ));
    let observed_sequences: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.observed_sequence)
            .collect::<Vec<_>>(),
    ));
    let details: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.details.as_deref())
            .collect::<Vec<_>>(),
    ));
    let sources: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.provenance.source.to_string())
            .collect::<Vec<_>>(),
    ));
    let source_event_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.provenance.source_event_id.as_deref())
            .collect::<Vec<_>>(),
    ));
    let source_session_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.provenance.source_session_id.as_deref())
            .collect::<Vec<_>>(),
    ));

    RecordBatch::try_new(
        schema,
        vec![
            recv_ts,
            exchange_ts,
            asset_ids,
            kinds,
            sequences,
            expected_sequences,
            observed_sequences,
            details,
            sources,
            source_event_ids,
            source_session_ids,
        ],
    )
    .map_err(StoreError::from)
}

pub fn checkpoint_refs_to_record_batch(
    checkpoints: &[&BookCheckpoint],
) -> Result<RecordBatch, StoreError> {
    let schema = Arc::new(checkpoint_schema());
    let checkpoint_ts: ArrayRef = Arc::new(UInt64Array::from(
        checkpoints
            .iter()
            .map(|e| e.checkpoint_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let recv_ts: ArrayRef = Arc::new(UInt64Array::from(
        checkpoints
            .iter()
            .map(|e| e.provenance.recv_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let exchange_ts: ArrayRef = Arc::new(UInt64Array::from(
        checkpoints
            .iter()
            .map(|e| e.provenance.exchange_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let asset_ids: ArrayRef = Arc::new(StringArray::from(
        checkpoints
            .iter()
            .map(|e| e.asset_id.as_str())
            .collect::<Vec<_>>(),
    ));
    let sources: ArrayRef = Arc::new(StringArray::from(
        checkpoints
            .iter()
            .map(|e| e.provenance.source.to_string())
            .collect::<Vec<_>>(),
    ));
    let source_event_ids: ArrayRef = Arc::new(StringArray::from(
        checkpoints
            .iter()
            .map(|e| e.provenance.source_event_id.as_deref())
            .collect::<Vec<_>>(),
    ));
    let source_session_ids: ArrayRef = Arc::new(StringArray::from(
        checkpoints
            .iter()
            .map(|e| e.provenance.source_session_id.as_deref())
            .collect::<Vec<_>>(),
    ));
    let bids_json: ArrayRef = Arc::new(StringArray::from(
        checkpoints
            .iter()
            .map(|e| serde_json::to_string(&e.bids))
            .collect::<Result<Vec<_>, _>>()?,
    ));
    let asks_json: ArrayRef = Arc::new(StringArray::from(
        checkpoints
            .iter()
            .map(|e| serde_json::to_string(&e.asks))
            .collect::<Result<Vec<_>, _>>()?,
    ));

    RecordBatch::try_new(
        schema,
        vec![
            checkpoint_ts,
            recv_ts,
            exchange_ts,
            asset_ids,
            sources,
            source_event_ids,
            source_session_ids,
            bids_json,
            asks_json,
        ],
    )
    .map_err(StoreError::from)
}

pub fn replay_validation_refs_to_record_batch(
    validations: &[&ReplayValidation],
) -> Result<RecordBatch, StoreError> {
    let schema = Arc::new(replay_validation_schema());
    let asset_ids: ArrayRef = Arc::new(StringArray::from(
        validations
            .iter()
            .map(|e| e.asset_id.as_str())
            .collect::<Vec<_>>(),
    ));
    let modes: ArrayRef = Arc::new(StringArray::from(
        validations
            .iter()
            .map(|e| e.mode.to_string())
            .collect::<Vec<_>>(),
    ));
    let replay_ts: ArrayRef = Arc::new(UInt64Array::from(
        validations
            .iter()
            .map(|e| e.replay_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let reference_ts: ArrayRef = Arc::new(UInt64Array::from(
        validations
            .iter()
            .map(|e| e.reference_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let matched: ArrayRef = Arc::new(BooleanArray::from(
        validations.iter().map(|e| e.matched).collect::<Vec<_>>(),
    ));
    let mismatch_summary: ArrayRef = Arc::new(StringArray::from(
        validations
            .iter()
            .map(|e| e.mismatch_summary.as_deref())
            .collect::<Vec<_>>(),
    ));
    let persisted_at: ArrayRef = Arc::new(UInt64Array::from(
        validations
            .iter()
            .map(|e| e.persisted_at_us)
            .collect::<Vec<_>>(),
    ));

    RecordBatch::try_new(
        schema,
        vec![
            asset_ids,
            modes,
            replay_ts,
            reference_ts,
            matched,
            mismatch_summary,
            persisted_at,
        ],
    )
    .map_err(StoreError::from)
}

pub fn execution_event_refs_to_record_batch(
    events: &[&ExecutionEvent],
) -> Result<RecordBatch, StoreError> {
    let schema = Arc::new(execution_event_schema());
    let event_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.event_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let asset_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.asset_id.as_ref().map(|id| id.as_str()))
            .collect::<Vec<_>>(),
    ));
    let order_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.order_id.as_str())
            .collect::<Vec<_>>(),
    ));
    let client_order_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.client_order_id.as_deref())
            .collect::<Vec<_>>(),
    ));
    let venue_order_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.venue_order_id.as_deref())
            .collect::<Vec<_>>(),
    ));
    let kinds: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.kind.to_string())
            .collect::<Vec<_>>(),
    ));
    let sides: ArrayRef = Arc::new(UInt8Array::from(
        events
            .iter()
            .map(|e| side_to_u8(e.side))
            .collect::<Vec<_>>(),
    ));
    let prices: ArrayRef = Arc::new(UInt32Array::from(
        events
            .iter()
            .map(|e| e.price.map(|p| p.raw()))
            .collect::<Vec<_>>(),
    ));
    let sizes: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.size.map(|s| s.raw()))
            .collect::<Vec<_>>(),
    ));
    let status: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.status.as_deref())
            .collect::<Vec<_>>(),
    ));
    let reason: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.reason.as_deref())
            .collect::<Vec<_>>(),
    ));
    let latency: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| serde_json::to_string(&e.latency))
            .collect::<Result<Vec<_>, _>>()?,
    ));

    RecordBatch::try_new(
        schema,
        vec![
            event_ts,
            asset_ids,
            order_ids,
            client_order_ids,
            venue_order_ids,
            kinds,
            sides,
            prices,
            sizes,
            status,
            reason,
            latency,
        ],
    )
    .map_err(StoreError::from)
}
