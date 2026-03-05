use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::error::FeedError;
use crate::ws::WsRawMessage;
use pb_types::event::{EventType, OrderbookEvent, Side};
use pb_types::fixed::{FixedPrice, FixedSize};
use pb_types::newtype::{AssetId, Sequence};
use pb_types::wire::WsMessage;

pub struct Dispatcher {
    rx: mpsc::Receiver<WsRawMessage>,
    tx: mpsc::Sender<OrderbookEvent>,
    sequence: u64,
}

impl Dispatcher {
    pub fn new(rx: mpsc::Receiver<WsRawMessage>, tx: mpsc::Sender<OrderbookEvent>) -> Self {
        Self {
            rx,
            tx,
            sequence: 0,
        }
    }

    pub async fn run(&mut self) -> Result<(), FeedError> {
        while let Some(raw) = self.rx.recv().await {
            if let Err(e) = self.dispatch(&raw).await {
                warn!("dispatch error: {e}");
            }
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
                    _ => None,
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
        let seq = Sequence::new(self.sequence);
        self.sequence += 1;

        Ok(OrderbookEvent {
            recv_timestamp_us,
            exchange_timestamp_us,
            asset_id,
            event_type,
            side,
            price,
            size,
            sequence: seq,
        })
    }

    async fn send(&self, event: OrderbookEvent) -> Result<(), FeedError> {
        self.tx.send(event).await.map_err(|_| {
            error!("output channel closed");
            FeedError::ChannelSend
        })
    }
}

fn parse_timestamp(ts: Option<&str>) -> u64 {
    ts.and_then(|s| s.parse::<u64>().ok()).unwrap_or(0)
}
