use std::time::Duration;

use clickhouse::Client;
use serde::Serialize;
use tokio::sync::mpsc;

use pb_types::event::EventType;
use pb_types::{OrderbookEvent, Side};

use crate::error::StoreError;

const DEFAULT_BATCH_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_BATCH_SIZE: usize = 10_000;

const CREATE_TABLE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS orderbook_events (
    recv_timestamp_us UInt64,
    exchange_timestamp_us UInt64,
    asset_id String,
    event_type Enum8('Snapshot' = 1, 'Delta' = 2, 'Trade' = 3),
    side Nullable(Enum8('Bid' = 1, 'Ask' = 2)),
    price UInt32,
    size UInt64,
    sequence UInt64,
    event_date Date MATERIALIZED toDate(fromUnixTimestamp64Micro(exchange_timestamp_us))
) ENGINE = ReplacingMergeTree()
PARTITION BY event_date
ORDER BY (asset_id, exchange_timestamp_us, sequence)
TTL event_date + INTERVAL 90 DAY
"#;

#[derive(Debug, Clone, Serialize, clickhouse::Row)]
struct ClickHouseRow {
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    asset_id: String,
    event_type: String,
    #[serde(serialize_with = "serialize_optional_side")]
    side: Option<String>,
    price: u32,
    size: u64,
    sequence: u64,
}

fn serialize_optional_side<S: serde::Serializer>(
    val: &Option<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match val {
        Some(v) => serializer.serialize_some(v),
        None => serializer.serialize_none(),
    }
}

impl From<&OrderbookEvent> for ClickHouseRow {
    fn from(e: &OrderbookEvent) -> Self {
        Self {
            recv_timestamp_us: e.recv_timestamp_us,
            exchange_timestamp_us: e.exchange_timestamp_us,
            asset_id: e.asset_id.as_str().to_string(),
            event_type: match e.event_type {
                EventType::Snapshot => "Snapshot".to_string(),
                EventType::Delta => "Delta".to_string(),
                EventType::Trade => "Trade".to_string(),
            },
            side: e.side.as_ref().map(|s| match s {
                Side::Bid => "Bid".to_string(),
                Side::Ask => "Ask".to_string(),
            }),
            price: e.price.raw(),
            size: e.size.raw(),
            sequence: e.sequence.raw(),
        }
    }
}

pub struct ClickHouseSink {
    rx: mpsc::Receiver<OrderbookEvent>,
    client: Client,
    batch_size: usize,
    batch_interval: Duration,
}

impl ClickHouseSink {
    pub fn new(rx: mpsc::Receiver<OrderbookEvent>, client: Client) -> Self {
        Self {
            rx,
            client,
            batch_size: DEFAULT_BATCH_SIZE,
            batch_interval: DEFAULT_BATCH_INTERVAL,
        }
    }

    pub async fn ensure_table(&self) -> Result<(), StoreError> {
        self.client.query(CREATE_TABLE_DDL).execute().await?;
        tracing::info!("Ensured orderbook_events table exists");
        Ok(())
    }

    pub async fn run(mut self) -> Result<(), StoreError> {
        let mut buffer: Vec<OrderbookEvent> = Vec::with_capacity(self.batch_size);
        let mut interval = tokio::time::interval(self.batch_interval);
        interval.tick().await; // consume immediate first tick

        loop {
            tokio::select! {
                event = self.rx.recv() => {
                    match event {
                        Some(e) => {
                            buffer.push(e);
                            if buffer.len() >= self.batch_size {
                                self.flush(&mut buffer).await?;
                            }
                        }
                        None => {
                            if !buffer.is_empty() {
                                self.flush(&mut buffer).await?;
                            }
                            tracing::info!("ClickHouseSink channel closed, shutting down");
                            return Ok(());
                        }
                    }
                }
                _ = interval.tick() => {
                    if !buffer.is_empty() {
                        self.flush(&mut buffer).await?;
                    }
                }
            }
        }
    }

    async fn flush(&self, buffer: &mut Vec<OrderbookEvent>) -> Result<(), StoreError> {
        let rows: Vec<ClickHouseRow> = buffer.drain(..).map(|e| ClickHouseRow::from(&e)).collect();
        let row_count = rows.len();

        let mut insert = self.client.insert("orderbook_events")?;
        for row in rows {
            insert.write(&row).await?;
        }
        insert.end().await?;

        tracing::debug!(rows = row_count, "Flushed batch to ClickHouse");
        Ok(())
    }
}
