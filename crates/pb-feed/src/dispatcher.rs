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
                if let Some(seq) = self.asset_sequences.get_mut(asset_id.as_str()) {
                    *seq = 0;
                } else {
                    self.asset_sequences.insert(asset_id.as_str().to_owned(), 0);
                }

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

        let sequence = self.next_sequence_for(&asset_id);

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

    fn next_sequence_for(&mut self, asset_id: &AssetId) -> Sequence {
        if let Some(seq) = self.asset_sequences.get_mut(asset_id.as_str()) {
            let current = *seq;
            *seq += 1;
            Sequence::new(current)
        } else {
            // New asset starts at sequence 0, then advances to 1.
            self.asset_sequences.insert(asset_id.as_str().to_owned(), 1);
            Sequence::new(0)
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_message(text: String) -> WsRawMessage {
        WsRawMessage {
            text,
            recv_timestamp_us: 1_700_000_000_000_000,
        }
    }

    #[tokio::test]
    async fn snapshot_resets_existing_asset_sequence_counter() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher.asset_sequences.insert("tok1".to_string(), 99);

        let msg = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "1700000000000000",
            "bids": [{"price": "0.50", "size": "10"}],
            "asks": [{"price": "0.60", "size": "20"}]
        });

        dispatcher
            .dispatch(&raw_message(msg.to_string()))
            .await
            .unwrap();

        let first = event_rx.recv().await.unwrap();
        let second = event_rx.recv().await.unwrap();

        assert_eq!(first.sequence.raw(), 0);
        assert_eq!(second.sequence.raw(), 1);
        assert_eq!(dispatcher.asset_sequences.get("tok1"), Some(&2));
    }

    #[tokio::test]
    async fn snapshot_registers_new_asset_with_zero_sequence_even_if_empty() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok-new",
            "timestamp": "1700000000000000",
            "bids": [],
            "asks": []
        });

        dispatcher
            .dispatch(&raw_message(msg.to_string()))
            .await
            .unwrap();

        assert_eq!(dispatcher.asset_sequences.get("tok-new"), Some(&0));
        assert!(event_rx.try_recv().is_err());
    }
}
