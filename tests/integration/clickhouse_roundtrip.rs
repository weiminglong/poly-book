//! ClickHouse integration tests using testcontainers.
//! Run with: `cargo test -p pb-integration-tests --test clickhouse_roundtrip -- --ignored`
//! Requires Docker.

use pb_replay::{ClickHouseReader, EventReader};
use pb_store::ClickHouseRecordWriter;
use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, DataSource, EventProvenance, ExecutionEvent,
    ExecutionEventKind, IngestEvent, IngestEventKind, LatencyTrace, PersistedRecord, ReplayMode,
    ReplayValidation, Side, TradeEvent, TradeFidelity,
};
use pb_types::{AssetId, FixedPrice, FixedSize, PriceLevel, Sequence};
use serde::Deserialize;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::clickhouse::ClickHouse;

fn provenance(
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    sequence: Option<u64>,
) -> EventProvenance {
    EventProvenance {
        recv_timestamp_us,
        exchange_timestamp_us,
        source: DataSource::WebSocket,
        source_event_id: None,
        source_session_id: Some("session-1".to_string()),
        sequence: sequence.map(Sequence::new),
    }
}

fn market_data_records(asset_id: &str, base_ts: u64) -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(asset_id);
    vec![
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::new(5000).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            provenance: provenance(base_ts, base_ts, Some(0)),
        }),
        PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Delta,
            side: Side::Ask,
            price: FixedPrice::new(5100).unwrap(),
            size: FixedSize::from_f64(25.0).unwrap(),
            provenance: provenance(base_ts + 1_000, base_ts + 1_000, Some(1)),
        }),
        PersistedRecord::Trade(TradeEvent {
            asset_id: asset_id.clone(),
            price: FixedPrice::new(5050).unwrap(),
            size: Some(FixedSize::from_f64(3.5).unwrap()),
            side: Some(Side::Bid),
            trade_id: Some("trade-1".to_string()),
            fidelity: TradeFidelity::Full,
            provenance: provenance(base_ts + 2_000, base_ts + 2_000, Some(2)),
        }),
        PersistedRecord::Ingest(IngestEvent {
            asset_id: Some(asset_id),
            kind: IngestEventKind::ReconnectSuccess,
            provenance: provenance(base_ts + 3_000, base_ts + 3_000, None),
            expected_sequence: None,
            observed_sequence: None,
            details: Some("reconnected".to_string()),
        }),
    ]
}

fn checkpoint_and_validation_records(asset_id: &str, base_ts: u64) -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(asset_id);
    vec![
        PersistedRecord::Checkpoint(BookCheckpoint {
            asset_id: asset_id.clone(),
            checkpoint_timestamp_us: base_ts + 10_000,
            recv_timestamp_us: base_ts + 10_500,
            exchange_timestamp_us: base_ts + 10_000,
            source: DataSource::RestSnapshot,
            source_event_id: Some("checkpoint-1".to_string()),
            source_session_id: None,
            bids: vec![PriceLevel {
                price: FixedPrice::new(5000).unwrap(),
                size: FixedSize::from_f64(100.0).unwrap(),
            }],
            asks: vec![PriceLevel {
                price: FixedPrice::new(5100).unwrap(),
                size: FixedSize::from_f64(25.0).unwrap(),
            }],
        }),
        PersistedRecord::Validation(ReplayValidation {
            asset_id,
            mode: ReplayMode::RecvTime,
            replay_timestamp_us: base_ts,
            reference_timestamp_us: base_ts + 10_000,
            matched: true,
            mismatch_summary: None,
            persisted_at_us: base_ts + 11_000,
        }),
    ]
}

fn execution_records(asset_id: &str, order_id: &str, base_ts: u64) -> Vec<PersistedRecord> {
    let asset_id = AssetId::new(asset_id);
    vec![
        PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: base_ts,
            asset_id: Some(asset_id.clone()),
            order_id: order_id.to_string(),
            client_order_id: Some("client-1".to_string()),
            venue_order_id: Some("venue-1".to_string()),
            kind: ExecutionEventKind::SubmitIntent,
            side: Some(Side::Bid),
            price: Some(FixedPrice::new(5050).unwrap()),
            size: Some(FixedSize::from_f64(7.25).unwrap()),
            status: Some("open".to_string()),
            reason: None,
            latency: LatencyTrace::from_optional_timestamps(
                Some(base_ts - 5),
                Some(base_ts - 2),
                Some(base_ts - 1),
                Some(base_ts),
                None,
                None,
            ),
        }),
        PersistedRecord::Execution(ExecutionEvent {
            event_timestamp_us: base_ts + 500,
            asset_id: Some(asset_id),
            order_id: order_id.to_string(),
            client_order_id: Some("client-1".to_string()),
            venue_order_id: Some("venue-1".to_string()),
            kind: ExecutionEventKind::Fill,
            side: Some(Side::Bid),
            price: Some(FixedPrice::new(5050).unwrap()),
            size: Some(FixedSize::from_f64(7.25).unwrap()),
            status: Some("filled".to_string()),
            reason: None,
            latency: LatencyTrace::from_optional_timestamps(
                Some(base_ts - 5),
                Some(base_ts - 2),
                Some(base_ts - 1),
                Some(base_ts),
                Some(base_ts + 200),
                Some(base_ts + 500),
            ),
        }),
    ]
}

async fn setup_clickhouse() -> (
    testcontainers::ContainerAsync<ClickHouse>,
    clickhouse::Client,
    String,
    String,
) {
    let container = ClickHouse::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(8123).await.unwrap();
    let url = format!("http://127.0.0.1:{port}");

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let db_name = format!("test_{nanos}");

    let client = clickhouse::Client::default().with_url(&url);
    client
        .query(&format!("CREATE DATABASE {db_name}"))
        .execute()
        .await
        .unwrap();

    (container, client.with_database(&db_name), url, db_name)
}

async fn write_records(client: clickhouse::Client, records: &[PersistedRecord]) {
    let writer = ClickHouseRecordWriter::new(client);
    writer.ensure_tables().await.unwrap();
    writer.write_batch(records).await.unwrap();
}

#[derive(Debug, Deserialize, clickhouse::Row)]
struct CountRow {
    count: u64,
}

async fn count_rows(client: &clickhouse::Client, table: &str) -> u64 {
    let query = format!("SELECT count() AS count FROM {table}");
    let row: CountRow = client.query(&query).fetch_one().await.unwrap();
    row.count
}

#[tokio::test]
#[ignore]
async fn clickhouse_market_data_roundtrip_split_tables() {
    let (_container, client, url, db_name) = setup_clickhouse().await;
    let base_ts = 1_700_000_000_000_000;
    let asset_id = AssetId::new("clickhouse-market-data");
    let records = market_data_records(asset_id.as_str(), base_ts);

    write_records(client.clone(), &records).await;

    assert_eq!(count_rows(&client, "book_events").await, 2);
    assert_eq!(count_rows(&client, "trade_events").await, 1);
    assert_eq!(count_rows(&client, "ingest_events").await, 1);

    let reader = ClickHouseReader::new(&url, &db_name);
    let window = reader
        .read_market_data(&asset_id, base_ts, base_ts + 20_000)
        .await
        .unwrap();
    assert_eq!(window.book_events.len(), 2);
    assert_eq!(window.trade_events.len(), 1);
    assert_eq!(window.ingest_events.len(), 1);
}

#[tokio::test]
#[ignore]
async fn clickhouse_checkpoint_and_validation_roundtrip() {
    let (_container, client, url, db_name) = setup_clickhouse().await;
    let base_ts = 1_700_000_100_000_000;
    let asset_id = AssetId::new("clickhouse-checkpoint");
    let records = checkpoint_and_validation_records(asset_id.as_str(), base_ts);

    write_records(client.clone(), &records).await;

    assert_eq!(count_rows(&client, "book_checkpoints").await, 1);
    assert_eq!(count_rows(&client, "replay_validations").await, 1);

    let reader = ClickHouseReader::new(&url, &db_name);
    let checkpoints = reader
        .read_checkpoints(&asset_id, base_ts, base_ts + 20_000)
        .await
        .unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].asset_id, asset_id);

    let validations = reader
        .read_validations(&asset_id, base_ts, base_ts + 20_000)
        .await
        .unwrap();
    assert_eq!(validations.len(), 1);
    assert!(validations[0].matched);
}

#[tokio::test]
#[ignore]
async fn clickhouse_execution_event_roundtrip() {
    let (_container, client, url, db_name) = setup_clickhouse().await;
    let base_ts = 1_700_000_200_000_000;
    let order_id = "order-clickhouse-1";
    let records = execution_records("clickhouse-execution", order_id, base_ts);

    write_records(client.clone(), &records).await;

    assert_eq!(count_rows(&client, "execution_events").await, 2);

    let reader = ClickHouseReader::new(&url, &db_name);
    let events = reader
        .read_execution_events(Some(order_id), base_ts, base_ts + 1_000)
        .await
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, ExecutionEventKind::SubmitIntent);
    assert_eq!(events[1].kind, ExecutionEventKind::Fill);
    assert_eq!(events[0].latency.market_data_recv_us, Some(base_ts - 5));
    assert_eq!(events[0].latency.exchange_fill_us, None);
    assert_eq!(events[1].latency.exchange_fill_us, Some(base_ts + 500));
}
