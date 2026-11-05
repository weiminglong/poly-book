//! Test Arrow schema conversion for split datasets.

use pb_store::schema::{book_event_schema, records_to_record_batch, trade_event_schema};
use pb_types::event::{
    BookEvent, BookEventKind, DataSource, EventProvenance, PersistedRecord, Side, TradeEvent,
    TradeFidelity,
};
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

use arrow::array::AsArray;
use arrow::datatypes::{UInt32Type, UInt64Type, UInt8Type};

fn sample_records() -> Vec<PersistedRecord> {
    vec![
        PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new("token-1"),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::from_f64(0.55).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: 1_000_000,
                exchange_timestamp_us: 999_000,
                source: DataSource::WebSocket,
                source_event_id: Some("book-1".to_string()),
                source_session_id: Some("session-1".to_string()),
                sequence: Some(Sequence::new(0)),
                ingest_ordinal: None,
            },
        }),
        PersistedRecord::Trade(TradeEvent {
            asset_id: AssetId::new("token-1"),
            price: FixedPrice::from_f64(0.42).unwrap(),
            size: None,
            side: None,
            trade_id: Some("trade-1".to_string()),
            fidelity: TradeFidelity::Partial,
            provenance: EventProvenance {
                recv_timestamp_us: 2_000_000,
                exchange_timestamp_us: 1_999_000,
                source: DataSource::WebSocket,
                source_event_id: Some("trade-1".to_string()),
                source_session_id: Some("session-1".to_string()),
                sequence: Some(Sequence::new(1)),
                ingest_ordinal: None,
            },
        }),
    ]
}

#[test]
fn split_schemas_have_expected_fields() {
    let book_schema = book_event_schema();
    // 12 fields: the 11 original columns plus `ingest_ordinal`, the monotonic
    // arrival-order tiebreaker added for replay determinism.
    assert_eq!(book_schema.fields().len(), 12);
    assert!(book_schema.field_with_name("event_kind").is_ok());
    assert!(book_schema.field_with_name("source").is_ok());
    assert!(book_schema.field_with_name("ingest_ordinal").is_ok());

    let trade_schema = trade_event_schema();
    assert_eq!(trade_schema.fields().len(), 12);
    assert!(trade_schema.field_with_name("fidelity").is_ok());
}

#[test]
fn records_to_record_batch_serializes_each_dataset_shape() {
    let records = sample_records();

    let book_batch = records_to_record_batch(&[&records[0]]).unwrap();
    assert_eq!(book_batch.num_rows(), 1);
    let prices = book_batch
        .column_by_name("price")
        .unwrap()
        .as_primitive::<UInt32Type>();
    assert_eq!(prices.value(0), 5500);
    let sequence = book_batch
        .column_by_name("sequence")
        .unwrap()
        .as_primitive::<UInt64Type>();
    assert_eq!(sequence.value(0), 0);
    let kind = book_batch
        .column_by_name("event_kind")
        .unwrap()
        .as_primitive::<UInt8Type>();
    assert_eq!(kind.value(0), 1);

    let trade_batch = records_to_record_batch(&[&records[1]]).unwrap();
    assert_eq!(trade_batch.num_rows(), 1);
    let trade_prices = trade_batch
        .column_by_name("price")
        .unwrap()
        .as_primitive::<UInt32Type>();
    assert_eq!(trade_prices.value(0), 4200);
}
