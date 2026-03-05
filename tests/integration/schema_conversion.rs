//! Test Arrow schema conversion roundtrip.

use pb_store::schema::{events_to_record_batch, orderbook_schema};
use pb_types::event::{EventType, OrderbookEvent, Side};
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

use arrow::array::{Array, AsArray};
use arrow::datatypes::{UInt32Type, UInt64Type, UInt8Type};

fn make_diverse_events() -> Vec<OrderbookEvent> {
    vec![
        OrderbookEvent {
            recv_timestamp_us: 1_000_000,
            exchange_timestamp_us: 999_000,
            asset_id: AssetId::new("token-1"),
            event_type: EventType::Snapshot,
            side: Some(Side::Bid),
            price: FixedPrice::from_f64(0.55).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            sequence: Sequence::new(0),
        },
        OrderbookEvent {
            recv_timestamp_us: 2_000_000,
            exchange_timestamp_us: 1_999_000,
            asset_id: AssetId::new("token-1"),
            event_type: EventType::Delta,
            side: Some(Side::Ask),
            price: FixedPrice::from_f64(0.60).unwrap(),
            size: FixedSize::from_f64(200.5).unwrap(),
            sequence: Sequence::new(1),
        },
        OrderbookEvent {
            recv_timestamp_us: 3_000_000,
            exchange_timestamp_us: 2_999_000,
            asset_id: AssetId::new("token-2"),
            event_type: EventType::Trade,
            side: None,
            price: FixedPrice::from_f64(0.42).unwrap(),
            size: FixedSize::ZERO,
            sequence: Sequence::new(2),
        },
    ]
}

#[test]
fn test_schema_has_expected_fields() {
    let schema = orderbook_schema();
    assert_eq!(schema.fields().len(), 8);
    assert!(schema.field_with_name("recv_timestamp_us").is_ok());
    assert!(schema.field_with_name("exchange_timestamp_us").is_ok());
    assert!(schema.field_with_name("asset_id").is_ok());
    assert!(schema.field_with_name("event_type").is_ok());
    assert!(schema.field_with_name("side").is_ok());
    assert!(schema.field_with_name("price").is_ok());
    assert!(schema.field_with_name("size").is_ok());
    assert!(schema.field_with_name("sequence").is_ok());
}

#[test]
fn test_events_to_record_batch() {
    let events = make_diverse_events();
    let batch = events_to_record_batch(&events).unwrap();

    assert_eq!(batch.num_rows(), 3);
    assert_eq!(batch.num_columns(), 8);

    // Verify timestamps
    let recv_ts = batch
        .column_by_name("recv_timestamp_us")
        .unwrap()
        .as_primitive::<UInt64Type>();
    assert_eq!(recv_ts.value(0), 1_000_000);
    assert_eq!(recv_ts.value(1), 2_000_000);
    assert_eq!(recv_ts.value(2), 3_000_000);

    // Verify asset IDs
    let asset_ids = batch.column_by_name("asset_id").unwrap().as_string::<i32>();
    assert_eq!(asset_ids.value(0), "token-1");
    assert_eq!(asset_ids.value(2), "token-2");

    // Verify event types (1=Snapshot, 2=Delta, 3=Trade)
    let event_types = batch
        .column_by_name("event_type")
        .unwrap()
        .as_primitive::<UInt8Type>();
    assert_eq!(event_types.value(0), 1);
    assert_eq!(event_types.value(1), 2);
    assert_eq!(event_types.value(2), 3);

    // Verify sides (1=Bid, 2=Ask, null for Trade)
    let sides = batch
        .column_by_name("side")
        .unwrap()
        .as_primitive::<UInt8Type>();
    assert_eq!(sides.value(0), 1);
    assert_eq!(sides.value(1), 2);
    assert!(sides.is_null(2));

    // Verify prices (raw u32)
    let prices = batch
        .column_by_name("price")
        .unwrap()
        .as_primitive::<UInt32Type>();
    assert_eq!(prices.value(0), 5500); // 0.55 * 10000
    assert_eq!(prices.value(1), 6000); // 0.60 * 10000
    assert_eq!(prices.value(2), 4200); // 0.42 * 10000

    // Verify sizes (raw u64)
    let sizes = batch
        .column_by_name("size")
        .unwrap()
        .as_primitive::<UInt64Type>();
    assert_eq!(sizes.value(0), 100_000_000); // 100.0 * 1_000_000
    assert_eq!(sizes.value(2), 0); // FixedSize::ZERO
}

#[test]
fn test_empty_events_batch() {
    let events: Vec<OrderbookEvent> = vec![];
    let batch = events_to_record_batch(&events).unwrap();
    assert_eq!(batch.num_rows(), 0);
    assert_eq!(batch.num_columns(), 8);
}
