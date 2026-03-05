//! ClickHouse integration tests using testcontainers.
//! Run with: `cargo test -p pb-integration-tests --test clickhouse_roundtrip -- --ignored`
//! Requires Docker.

use std::time::Duration;

use pb_replay::engine::ReplayEngine;
use pb_replay::reader::EventReader;
use pb_store::ClickHouseSink;
use pb_types::event::{EventType, OrderbookEvent, Side};
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::clickhouse::ClickHouse;

/// Create test events for write/read roundtrip.
fn make_test_events(asset_id: &str, count: usize) -> Vec<OrderbookEvent> {
    let aid = AssetId::new(asset_id);
    (0..count)
        .map(|i| {
            let is_bid = i % 2 == 0;
            OrderbookEvent {
                recv_timestamp_us: 1_700_000_000_000_000 + (i as u64) * 1_000_000,
                exchange_timestamp_us: 1_700_000_000_000_000 + (i as u64) * 1_000_000,
                asset_id: aid.clone(),
                event_type: if i < 5 {
                    EventType::Snapshot
                } else {
                    EventType::Delta
                },
                side: Some(if is_bid { Side::Bid } else { Side::Ask }),
                price: FixedPrice::new(5000 + (i as u32) * 100).unwrap(),
                size: FixedSize::from_f64((i as f64 + 1.0) * 10.0).unwrap(),
                sequence: Sequence::new(i as u64),
            }
        })
        .collect()
}

/// Create replay events: 1 snapshot (3 bids + 2 asks) + 3 deltas.
fn make_replay_events(asset_id: &str, base_ts: u64) -> Vec<OrderbookEvent> {
    let aid = AssetId::new(asset_id);
    let mut events = Vec::new();
    let mut seq = 0u64;

    for (price, size) in [(5000u32, 100.0), (4900, 200.0), (4800, 300.0)] {
        events.push(OrderbookEvent {
            recv_timestamp_us: base_ts,
            exchange_timestamp_us: base_ts,
            asset_id: aid.clone(),
            event_type: EventType::Snapshot,
            side: Some(Side::Bid),
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::from_f64(size).unwrap(),
            sequence: Sequence::new(seq),
        });
        seq += 1;
    }
    for (price, size) in [(5500u32, 150.0), (5600, 250.0)] {
        events.push(OrderbookEvent {
            recv_timestamp_us: base_ts,
            exchange_timestamp_us: base_ts,
            asset_id: aid.clone(),
            event_type: EventType::Snapshot,
            side: Some(Side::Ask),
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::from_f64(size).unwrap(),
            sequence: Sequence::new(seq),
        });
        seq += 1;
    }

    // Delta 1: update best bid
    events.push(OrderbookEvent {
        recv_timestamp_us: base_ts + 1_000_000,
        exchange_timestamp_us: base_ts + 1_000_000,
        asset_id: aid.clone(),
        event_type: EventType::Delta,
        side: Some(Side::Bid),
        price: FixedPrice::new(5000).unwrap(),
        size: FixedSize::from_f64(500.0).unwrap(),
        sequence: Sequence::new(seq),
    });
    seq += 1;

    // Delta 2: add new ask level
    events.push(OrderbookEvent {
        recv_timestamp_us: base_ts + 2_000_000,
        exchange_timestamp_us: base_ts + 2_000_000,
        asset_id: aid.clone(),
        event_type: EventType::Delta,
        side: Some(Side::Ask),
        price: FixedPrice::new(5200).unwrap(),
        size: FixedSize::from_f64(75.0).unwrap(),
        sequence: Sequence::new(seq),
    });
    seq += 1;

    // Delta 3: remove a bid level
    events.push(OrderbookEvent {
        recv_timestamp_us: base_ts + 3_000_000,
        exchange_timestamp_us: base_ts + 3_000_000,
        asset_id: aid.clone(),
        event_type: EventType::Delta,
        side: Some(Side::Bid),
        price: FixedPrice::new(4800).unwrap(),
        size: FixedSize::ZERO,
        sequence: Sequence::new(seq),
    });

    events
}

async fn setup_clickhouse() -> (
    testcontainers::ContainerAsync<ClickHouse>,
    clickhouse::Client,
    String,
) {
    let container = ClickHouse::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(8123).await.unwrap();
    let url = format!("http://127.0.0.1:{}", port);

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

    let client = client.with_database(&db_name);

    (container, client, db_name)
}

async fn write_events_via_sink(client: clickhouse::Client, events: &[OrderbookEvent]) {
    let (tx, rx) = tokio::sync::mpsc::channel::<OrderbookEvent>(1000);
    let sink = ClickHouseSink::new(rx, client);
    sink.ensure_table().await.unwrap();

    let handle = tokio::spawn(async move { sink.run().await.unwrap() });

    for event in events {
        tx.send(event.clone()).await.unwrap();
    }

    // Wait for batch interval to flush
    tokio::time::sleep(Duration::from_secs(2)).await;
    drop(tx);
    handle.await.unwrap();
}

#[tokio::test]
#[ignore]
async fn test_clickhouse_sink_write_and_read() {
    let (_container, client, _db) = setup_clickhouse().await;

    let events = make_test_events("ch-test-1", 20);
    write_events_via_sink(client.clone(), &events).await;

    // Query count
    let count: u64 = client
        .query("SELECT count() FROM orderbook_events")
        .fetch_one()
        .await
        .unwrap();
    assert_eq!(count, 20);

    // Verify field roundtrip for first event
    #[derive(Debug, clickhouse::Row, serde::Deserialize)]
    #[allow(dead_code)]
    struct TestRow {
        recv_timestamp_us: u64,
        asset_id: String,
        event_type: String,
        price: u32,
        size: u64,
    }

    let row: TestRow = client
        .query(
            "SELECT recv_timestamp_us, asset_id, event_type, price, size \
             FROM orderbook_events ORDER BY recv_timestamp_us LIMIT 1",
        )
        .fetch_one()
        .await
        .unwrap();

    assert_eq!(row.recv_timestamp_us, 1_700_000_000_000_000);
    assert_eq!(row.asset_id, "ch-test-1");
    assert_eq!(row.event_type, "Snapshot");
    assert_eq!(row.price, events[0].price.raw());
    assert_eq!(row.size, events[0].size.raw());
}

#[tokio::test]
#[ignore]
async fn test_clickhouse_reader_roundtrip() {
    let (_container, client, _db) = setup_clickhouse().await;

    let events = make_test_events("ch-test-2", 20);
    write_events_via_sink(client.clone(), &events).await;

    let reader = ClickHouseReaderFromClient::new(client.clone());

    let asset_id = AssetId::new("ch-test-2");
    let read_events = reader
        .read_events(
            &asset_id,
            1_700_000_000_000_000,
            1_700_000_000_000_000 + 20_000_000,
        )
        .await
        .unwrap();

    assert_eq!(read_events.len(), 20);

    // Verify ordering by (recv_timestamp_us, sequence)
    for window in read_events.windows(2) {
        assert!(
            (window[0].recv_timestamp_us, window[0].sequence.raw())
                <= (window[1].recv_timestamp_us, window[1].sequence.raw())
        );
    }

    // Verify first event fields
    assert_eq!(read_events[0].asset_id.as_str(), "ch-test-2");
    assert_eq!(read_events[0].event_type, EventType::Snapshot);
    assert_eq!(read_events[0].price.raw(), events[0].price.raw());
}

#[tokio::test]
#[ignore]
async fn test_clickhouse_replay_engine() {
    let (_container, client, _db) = setup_clickhouse().await;

    let base_ts: u64 = 1_700_000_000_000_000;
    let events = make_replay_events("ch-replay", base_ts);
    write_events_via_sink(client.clone(), &events).await;

    let reader = ClickHouseReaderFromClient::new(client);
    let engine = ReplayEngine::new(reader);

    // Reconstruct at snapshot time
    let book = engine
        .reconstruct_at(&AssetId::new("ch-replay"), base_ts)
        .await
        .unwrap();

    assert_eq!(book.bid_depth(), 3);
    assert_eq!(book.ask_depth(), 2);
    assert_eq!(book.best_bid().unwrap().0.raw(), 5000);
    assert_eq!(book.best_ask().unwrap().0.raw(), 5500);

    // Reconstruct after all deltas
    let book = engine
        .reconstruct_at(&AssetId::new("ch-replay"), base_ts + 3_000_000)
        .await
        .unwrap();

    // Delta 1: bid at 5000 updated to 500 size
    assert_eq!(book.best_bid().unwrap().1.raw(), 500_000_000);
    // Delta 2: new ask at 5200 (becomes best ask)
    assert_eq!(book.best_ask().unwrap().0.raw(), 5200);
    // Delta 3: bid at 4800 removed -> 2 bids remain
    assert_eq!(book.bid_depth(), 2);
    // 3 asks total: 5200, 5500, 5600
    assert_eq!(book.ask_depth(), 3);
}

/// A ClickHouseReader that wraps an existing client (for testing).
/// This avoids needing to expose the container's mapped port through the URL.
struct ClickHouseReaderFromClient {
    client: clickhouse::Client,
}

impl ClickHouseReaderFromClient {
    fn new(client: clickhouse::Client) -> Self {
        Self { client }
    }
}

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

impl pb_replay::reader::EventReader for ClickHouseReaderFromClient {
    async fn read_events(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<OrderbookEvent>, pb_replay::ReplayError> {
        let rows: Vec<EventRow> = self
            .client
            .query(
                "SELECT recv_timestamp_us, exchange_timestamp_us, asset_id, \
                 event_type, side, price, size, sequence \
                 FROM orderbook_events \
                 WHERE asset_id = ? AND recv_timestamp_us >= ? AND recv_timestamp_us <= ? \
                 ORDER BY recv_timestamp_us, sequence",
            )
            .bind(asset_id.as_str())
            .bind(start_us)
            .bind(end_us)
            .fetch_all()
            .await?;

        rows.into_iter()
            .map(|row| {
                let event_type = match row.event_type.as_str() {
                    "Snapshot" => EventType::Snapshot,
                    "Delta" => EventType::Delta,
                    "Trade" => EventType::Trade,
                    other => {
                        return Err(pb_replay::ReplayError::InvalidEventType(other.to_string()))
                    }
                };
                let side = match row.side.as_deref() {
                    Some("Bid") => Some(Side::Bid),
                    Some("Ask") => Some(Side::Ask),
                    None | Some("") => None,
                    Some(other) => {
                        return Err(pb_replay::ReplayError::InvalidSide(other.to_string()))
                    }
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
            })
            .collect()
    }
}
