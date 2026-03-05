use std::collections::HashMap;

use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::error::FeedError;
use crate::ws::WsRawMessage;
use pb_types::event::{EventType, OrderbookEvent, Side};
use pb_types::fixed::{FixedPrice, FixedSize};
use pb_types::newtype::{AssetId, Sequence};
use pb_types::wire::WsMessage;

fn event_type_label(et: &EventType) -> &'static str {
    match et {
        EventType::Snapshot => "snapshot",
        EventType::Delta => "delta",
        EventType::Trade => "trade",
    }
}

pub struct Dispatcher {
    rx: mpsc::Receiver<WsRawMessage>,
    tx: mpsc::Sender<OrderbookEvent>,
    /// Per-asset monotonic sequence counters.
    /// Snapshots reset the counter; deltas increment it.
    /// This makes `L2Book::check_sequence()` meaningful during replay.
    asset_sequences: HashMap<String, u64>,
}

impl Dispatcher {
    pub fn new(rx: mpsc::Receiver<WsRawMessage>, tx: mpsc::Sender<OrderbookEvent>) -> Self {
        Self {
            rx,
            tx,
            asset_sequences: HashMap::new(),
        }
    }

    pub async fn run(&mut self) -> Result<(), FeedError> {
        while let Some(raw) = self.rx.recv().await {
            let start = std::time::Instant::now();
            if let Err(e) = self.dispatch(&raw).await {
                match &e {
                    FeedError::ChannelSend => return Err(e),
                    _ => warn!("dispatch error: {e}"),
                }
            }
            pb_metrics::record_processing_duration_us(start.elapsed().as_micros() as f64);
        }
        debug!("dispatcher input channel closed");
        Ok(())
    }

    async fn dispatch(&mut self, raw: &WsRawMessage) -> Result<(), FeedError> {
        let msg: WsMessage<'_> = serde_json::from_str(&raw.text)?;

        match msg {
            WsMessage::Book(book) => {
                let asset_id = AssetId::new(book.asset_id);
                let exchange_ts = parse_timestamp(book.timestamp);

                // Snapshot resets the per-asset sequence to 0
                let asset_key = asset_id.as_str().to_string();
                self.asset_sequences.insert(asset_key, 0);

                for entry in &book.bids {
                    let event = self.make_event(
                        raw.recv_timestamp_us,
                        exchange_ts,
                        asset_id.clone(),
                        EventType::Snapshot,
                        Some(Side::Bid),
                        entry.price,
                        entry.size,
                    )?;
                    self.send(event).await?;
                }

                for entry in &book.asks {
                    let event = self.make_event(
                        raw.recv_timestamp_us,
                        exchange_ts,
                        asset_id.clone(),
                        EventType::Snapshot,
                        Some(Side::Ask),
                        entry.price,
                        entry.size,
                    )?;
                    self.send(event).await?;
                }
            }
            WsMessage::PriceChange(pc) => {
                let side = match pc.side {
                    "BUY" | "buy" | "Bid" | "bid" => Some(Side::Bid),
                    "SELL" | "sell" | "Ask" | "ask" => Some(Side::Ask),
                    other => {
                        warn!(side = other, "unknown side string, skipping delta");
                        return Ok(());
                    }
                };

                let event = self.make_event(
                    raw.recv_timestamp_us,
                    parse_timestamp(pc.timestamp),
                    AssetId::new(pc.asset_id),
                    EventType::Delta,
                    side,
                    pc.price,
                    pc.size,
                )?;
                self.send(event).await?;
            }
            WsMessage::LastTradePrice(lt) => {
                let event = self.make_event(
                    raw.recv_timestamp_us,
                    parse_timestamp(lt.timestamp),
                    AssetId::new(lt.asset_id),
                    EventType::Trade,
                    None,
                    lt.price,
                    "0",
                )?;
                self.send(event).await?;
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    fn make_event(
        &mut self,
        recv_timestamp_us: u64,
        exchange_timestamp_us: u64,
        asset_id: AssetId,
        event_type: EventType,
        side: Option<Side>,
        price_str: &str,
        size_str: &str,
    ) -> Result<OrderbookEvent, FeedError> {
        let price = FixedPrice::try_from(price_str)?;
        let size = FixedSize::try_from(size_str)?;

        let asset_key = asset_id.as_str().to_string();
        let seq = self.asset_sequences.entry(asset_key).or_insert(0);
        let sequence = Sequence::new(*seq);
        *seq += 1;

        Ok(OrderbookEvent {
            recv_timestamp_us,
            exchange_timestamp_us,
            asset_id,
            event_type,
            side,
            price,
            size,
            sequence,
        })
    }

    async fn send(&self, event: OrderbookEvent) -> Result<(), FeedError> {
        pb_metrics::record_message_received(event_type_label(&event.event_type));

        // Record per-type metrics
        match event.event_type {
            EventType::Snapshot => pb_metrics::record_snapshot_applied(),
            EventType::Delta => pb_metrics::record_delta_applied(),
            EventType::Trade => pb_metrics::record_trade_received(),
        }

        // Record WS latency when exchange timestamp is available
        if event.exchange_timestamp_us > 0 && event.recv_timestamp_us > event.exchange_timestamp_us
        {
            let latency_us = (event.recv_timestamp_us - event.exchange_timestamp_us) as f64;
            pb_metrics::record_ws_latency_us(latency_us);
        }

        self.tx.send(event).await.map_err(|_| {
            error!("output channel closed");
            FeedError::ChannelSend
        })
    }
}

fn parse_timestamp(ts: Option<&str>) -> u64 {
    ts.and_then(|s| s.parse::<u64>().ok()).unwrap_or(0)
}
