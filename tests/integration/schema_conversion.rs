//! Test Arrow schema conversion for split datasets.

use pb_store::schema::{
    book_event_schema, execution_event_schema, records_to_record_batch, trade_event_schema,
};
use pb_types::event::{
    BookEvent, BookEventKind, DataSource, EventProvenance, ExecutionEvent, ExecutionEventKind,
    LatencyTrace, PersistedRecord, Side, TradeEvent, TradeFidelity,
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
            },
        }),
        PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: 3_000_000,
            asset_id: Some(AssetId::new("token-1")),
            order_id: "order-1".to_string(),
            client_order_id: Some("client-1".to_string()),
            venue_order_id: None,
            kind: ExecutionEventKind::SubmitIntent,
            side: Some(Side::Bid),
            price: Some(FixedPrice::from_f64(0.40).unwrap()),
            size: Some(FixedSize::from_f64(12.0).unwrap()),
            status: Some("open".to_string()),
            reason: None,
            latency: LatencyTrace {
                market_data_recv_us: Some(10),
                normalization_done_us: Some(12),
                strategy_decision_us: Some(15),
                order_submit_us: Some(20),
                exchange_ack_us: None,
                exchange_fill_us: None,
            },
        }),
    ]
}

#[test]
fn split_schemas_have_expected_fields() {
    let book_schema = book_event_schema();
    assert_eq!(book_schema.fields().len(), 11);
    assert!(book_schema.field_with_name("event_kind").is_ok());
    assert!(book_schema.field_with_name("source").is_ok());

    let trade_schema = trade_event_schema();
    assert_eq!(trade_schema.fields().len(), 12);
    assert!(trade_schema.field_with_name("fidelity").is_ok());

    let execution_schema = execution_event_schema();
    assert_eq!(execution_schema.fields().len(), 12);
    assert!(execution_schema.field_with_name("latency_json").is_ok());
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

    let execution_batch = records_to_record_batch(&[&records[2]]).unwrap();
    assert_eq!(execution_batch.num_rows(), 1);
    let execution_sizes = execution_batch
        .column_by_name("size")
        .unwrap()
        .as_primitive::<UInt64Type>();
    assert_eq!(execution_sizes.value(0), 12_000_000);
}
