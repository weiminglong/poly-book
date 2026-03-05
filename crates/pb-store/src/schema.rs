use std::sync::Arc;

use arrow::array::{ArrayRef, StringArray, UInt32Array, UInt64Array, UInt8Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;

use pb_types::event::EventType;
use pb_types::{OrderbookEvent, Side};

use crate::error::StoreError;

pub fn orderbook_schema() -> Schema {
    Schema::new(vec![
        Field::new("recv_timestamp_us", DataType::UInt64, false),
        Field::new("exchange_timestamp_us", DataType::UInt64, false),
        Field::new("asset_id", DataType::Utf8, false),
        Field::new("event_type", DataType::UInt8, false),
        Field::new("side", DataType::UInt8, true),
        Field::new("price", DataType::UInt32, false),
        Field::new("size", DataType::UInt64, false),
        Field::new("sequence", DataType::UInt64, false),
    ])
}

fn event_type_to_u8(et: &EventType) -> u8 {
    match et {
        EventType::Snapshot => 1,
        EventType::Delta => 2,
        EventType::Trade => 3,
    }
}

fn side_to_u8(side: &Option<Side>) -> Option<u8> {
    side.as_ref().map(|s| match s {
        Side::Bid => 1,
        Side::Ask => 2,
    })
}

pub fn events_to_record_batch(events: &[OrderbookEvent]) -> Result<RecordBatch, StoreError> {
    let schema = Arc::new(orderbook_schema());

    let recv_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.recv_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let exchange_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.exchange_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let asset_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.asset_id.as_str())
            .collect::<Vec<_>>(),
    ));
    let event_types: ArrayRef = Arc::new(UInt8Array::from(
        events
            .iter()
            .map(|e| event_type_to_u8(&e.event_type))
            .collect::<Vec<_>>(),
    ));
    let sides: ArrayRef = Arc::new(UInt8Array::from(
        events
            .iter()
            .map(|e| side_to_u8(&e.side))
            .collect::<Vec<_>>(),
    ));
    let prices: ArrayRef = Arc::new(UInt32Array::from(
        events.iter().map(|e| e.price.raw()).collect::<Vec<_>>(),
    ));
    let sizes: ArrayRef = Arc::new(UInt64Array::from(
        events.iter().map(|e| e.size.raw()).collect::<Vec<_>>(),
    ));
    let sequences: ArrayRef = Arc::new(UInt64Array::from(
        events.iter().map(|e| e.sequence.raw()).collect::<Vec<_>>(),
    ));

    let batch = RecordBatch::try_new(
        schema,
        vec![
            recv_ts,
            exchange_ts,
            asset_ids,
            event_types,
            sides,
            prices,
            sizes,
            sequences,
        ],
    )?;

    Ok(batch)
}

pub fn event_refs_to_record_batch(events: &[&OrderbookEvent]) -> Result<RecordBatch, StoreError> {
    let schema = Arc::new(orderbook_schema());

    let recv_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.recv_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let exchange_ts: ArrayRef = Arc::new(UInt64Array::from(
        events
            .iter()
            .map(|e| e.exchange_timestamp_us)
            .collect::<Vec<_>>(),
    ));
    let asset_ids: ArrayRef = Arc::new(StringArray::from(
        events
            .iter()
            .map(|e| e.asset_id.as_str())
            .collect::<Vec<_>>(),
    ));
    let event_types: ArrayRef = Arc::new(UInt8Array::from(
        events
            .iter()
            .map(|e| event_type_to_u8(&e.event_type))
            .collect::<Vec<_>>(),
    ));
    let sides: ArrayRef = Arc::new(UInt8Array::from(
        events
            .iter()
            .map(|e| side_to_u8(&e.side))
            .collect::<Vec<_>>(),
    ));
    let prices: ArrayRef = Arc::new(UInt32Array::from(
        events.iter().map(|e| e.price.raw()).collect::<Vec<_>>(),
    ));
    let sizes: ArrayRef = Arc::new(UInt64Array::from(
        events.iter().map(|e| e.size.raw()).collect::<Vec<_>>(),
    ));
    let sequences: ArrayRef = Arc::new(UInt64Array::from(
        events.iter().map(|e| e.sequence.raw()).collect::<Vec<_>>(),
    ));

    let batch = RecordBatch::try_new(
        schema,
        vec![
            recv_ts,
            exchange_ts,
            asset_ids,
            event_types,
            sides,
            prices,
            sizes,
            sequences,
        ],
    )?;

    Ok(batch)
}
