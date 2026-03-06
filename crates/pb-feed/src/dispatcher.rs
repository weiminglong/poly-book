use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use crate::error::FeedError;
use crate::ws::{FeedMessage, WsLifecycleEvent, WsLifecycleKind, WsRawMessage};
use pb_types::event::{
    BookEvent, BookEventKind, DataSource, EventProvenance, IngestEvent, IngestEventKind,
    PersistedRecord, Side, TradeEvent, TradeFidelity,
};
use pb_types::fixed::{FixedPrice, FixedSize};
use pb_types::newtype::{AssetId, Sequence};
use pb_types::wire::WsMessage;

fn record_label(record: &PersistedRecord) -> &'static str {
    match record {
        PersistedRecord::Book(event) => match event.kind {
            BookEventKind::Snapshot => "snapshot",
            BookEventKind::Delta => "delta",
        },
        PersistedRecord::Trade(_) => "trade",
        PersistedRecord::Ingest(_) => "ingest",
        PersistedRecord::Checkpoint(_) => "checkpoint",
        PersistedRecord::Validation(_) => "validation",
        PersistedRecord::Execution(_) => "execution",
    }
}

pub struct Dispatcher {
    rx: mpsc::Receiver<FeedMessage>,
    tx: mpsc::Sender<PersistedRecord>,
    /// Per-asset monotonic sequence counters. Snapshots reset the counter.
    asset_sequences: HashMap<Arc<str>, u64>,
    /// Per-asset last snapshot exchange timestamp for staleness detection.
    last_snapshot_ts: HashMap<Arc<str>, u64>,
    /// Interned AssetIds to avoid heap allocation on every message.
    asset_id_cache: HashMap<Arc<str>, AssetId>,
    current_session_id: Option<String>,
}

impl Dispatcher {
    pub fn new(rx: mpsc::Receiver<FeedMessage>, tx: mpsc::Sender<PersistedRecord>) -> Self {
        Self {
            rx,
            tx,
            asset_sequences: HashMap::new(),
            last_snapshot_ts: HashMap::new(),
            asset_id_cache: HashMap::new(),
            current_session_id: None,
        }
    }

    pub async fn run(&mut self) -> Result<(), FeedError> {
        self.run_with_token(CancellationToken::new()).await
    }

    pub async fn run_with_token(&mut self, token: CancellationToken) -> Result<(), FeedError> {
        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    debug!("dispatcher shutdown requested");
                    return Ok(());
                }
                raw = self.rx.recv() => {
                    match raw {
                        Some(message) => {
                            let start = std::time::Instant::now();
                            if let Err(e) = self.dispatch(message).await {
                                match &e {
                                    FeedError::ChannelSend => return Err(e),
                                    _ => warn!("dispatch error: {e}"),
                                }
                            }
                            pb_metrics::record_processing_duration_us(start.elapsed().as_micros() as f64);
                        }
                        None => {
                            debug!("dispatcher input channel closed");
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    async fn dispatch(&mut self, message: FeedMessage) -> Result<(), FeedError> {
        match message {
            FeedMessage::Raw(raw) => self.dispatch_raw(&raw).await,
            FeedMessage::Lifecycle(event) => self.dispatch_lifecycle(event).await,
        }
    }

    async fn dispatch_lifecycle(&mut self, event: WsLifecycleEvent) -> Result<(), FeedError> {
        let kind = match event.kind {
            WsLifecycleKind::ReconnectStart => IngestEventKind::ReconnectStart,
            WsLifecycleKind::ReconnectSuccess => IngestEventKind::ReconnectSuccess,
        };
        if event.kind == WsLifecycleKind::ReconnectSuccess {
            self.current_session_id = Some(event.session_id.clone());
        }

        let provenance = EventProvenance {
            recv_timestamp_us: event.recv_timestamp_us,
            exchange_timestamp_us: 0,
            source: DataSource::WebSocket,
            source_event_id: None,
            source_session_id: Some(event.session_id.clone()),
            sequence: None,
        };
        self.send(PersistedRecord::Ingest(IngestEvent {
            asset_id: None,
            kind,
            provenance: provenance.clone(),
            expected_sequence: None,
            observed_sequence: None,
            details: event.details,
        }))
        .await?;

        if kind == IngestEventKind::ReconnectSuccess {
            self.send(PersistedRecord::Ingest(IngestEvent {
                asset_id: None,
                kind: IngestEventKind::SourceReset,
                provenance,
                expected_sequence: None,
                observed_sequence: None,
                details: Some("connection re-established; downstream readers should treat continuity as reset".to_string()),
            }))
            .await?;
        }

        Ok(())
    }

    async fn dispatch_raw(&mut self, raw: &WsRawMessage) -> Result<(), FeedError> {
        let msg: WsMessage<'_> = match serde_json::from_str(&raw.text) {
            Ok(m) => m,
            Err(e) => {
                debug!("skipping non-event message: {e}");
                return Ok(());
            }
        };

        match msg {
            WsMessage::Book(book) => {
                let asset_id = self.intern_asset_id(book.asset_id);
                let exchange_ts = parse_timestamp_us(book.timestamp);
                let source_event_id = book.hash.map(str::to_string);

                if exchange_ts > 0 {
                    if let Some(&last_ts) = self.last_snapshot_ts.get(asset_id.as_str()) {
                        if exchange_ts <= last_ts {
                            warn!(
                                asset_id = %asset_id,
                                exchange_ts,
                                last_ts,
                                "stale snapshot detected, skipping"
                            );
                            pb_metrics::record_stale_snapshot_skipped();
                            self.send(PersistedRecord::Ingest(IngestEvent {
                                asset_id: Some(asset_id),
                                kind: IngestEventKind::StaleSnapshotSkip,
                                provenance: self.make_provenance(
                                    raw.recv_timestamp_us,
                                    exchange_ts,
                                    source_event_id,
                                    None,
                                ),
                                expected_sequence: None,
                                observed_sequence: None,
                                details: Some(format!(
                                    "snapshot exchange timestamp {} <= latest accepted {}",
                                    exchange_ts, last_ts
                                )),
                            }))
                            .await?;
                            return Ok(());
                        }
                    }
                    self.last_snapshot_ts
                        .insert(asset_id.0.clone(), exchange_ts);
                }
                pb_metrics::record_snapshot_reconciled();

                self.asset_sequences.insert(asset_id.0.clone(), 0);

                for entry in &book.bids {
                    let event = self.make_book_event(
                        raw.recv_timestamp_us,
                        exchange_ts,
                        asset_id.clone(),
                        BookEventKind::Snapshot,
                        Side::Bid,
                        entry.price,
                        entry.size,
                        source_event_id.clone(),
                    )?;
                    self.send(PersistedRecord::Book(event)).await?;
                }

                for entry in &book.asks {
                    let event = self.make_book_event(
                        raw.recv_timestamp_us,
                        exchange_ts,
                        asset_id.clone(),
                        BookEventKind::Snapshot,
                        Side::Ask,
                        entry.price,
                        entry.size,
                        source_event_id.clone(),
                    )?;
                    self.send(PersistedRecord::Book(event)).await?;
                }
            }
            WsMessage::PriceChange(pc) => {
                let exchange_ts = parse_timestamp_us(pc.timestamp);
                for entry in &pc.price_changes {
                    let side = match entry.side {
                        "BUY" | "buy" | "Bid" | "bid" => Side::Bid,
                        "SELL" | "sell" | "Ask" | "ask" => Side::Ask,
                        other => {
                            warn!(side = other, "unknown side string, skipping delta");
                            continue;
                        }
                    };

                    let asset_id = self.intern_asset_id(entry.asset_id);
                    let event = self.make_book_event(
                        raw.recv_timestamp_us,
                        exchange_ts,
                        asset_id,
                        BookEventKind::Delta,
                        side,
                        entry.price,
                        entry.size,
                        entry.hash.map(str::to_string),
                    )?;
                    self.send(PersistedRecord::Book(event)).await?;
                }
            }
            WsMessage::LastTradePrice(lt) => {
                let asset_id = self.intern_asset_id(lt.asset_id);
                let size = lt.size.map(FixedSize::try_from).transpose()?;
                let side = lt.side.and_then(parse_trade_side);
                let fidelity = if size.is_some() && side.is_some() {
                    TradeFidelity::Full
                } else {
                    TradeFidelity::Partial
                };

                let event = TradeEvent {
                    asset_id,
                    price: FixedPrice::try_from(lt.price)?,
                    size,
                    side,
                    trade_id: lt.transaction_hash.map(str::to_string),
                    fidelity,
                    provenance: self.make_provenance(
                        raw.recv_timestamp_us,
                        parse_timestamp_us(lt.timestamp),
                        lt.transaction_hash.map(str::to_string),
                        None,
                    ),
                };
                self.send(PersistedRecord::Trade(event)).await?;
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::result_large_err)]
    fn make_book_event(
        &mut self,
        recv_timestamp_us: u64,
        exchange_timestamp_us: u64,
        asset_id: AssetId,
        kind: BookEventKind,
        side: Side,
        price_str: &str,
        size_str: &str,
        source_event_id: Option<String>,
    ) -> Result<BookEvent, FeedError> {
        let price = FixedPrice::try_from(price_str)?;
        let size = FixedSize::try_from(size_str)?;
        let sequence = self.next_sequence_for(&asset_id);

        Ok(BookEvent {
            asset_id,
            kind,
            side,
            price,
            size,
            provenance: self.make_provenance(
                recv_timestamp_us,
                exchange_timestamp_us,
                source_event_id,
                Some(sequence),
            ),
        })
    }

    fn make_provenance(
        &self,
        recv_timestamp_us: u64,
        exchange_timestamp_us: u64,
        source_event_id: Option<String>,
        sequence: Option<Sequence>,
    ) -> EventProvenance {
        EventProvenance {
            recv_timestamp_us,
            exchange_timestamp_us,
            source: DataSource::WebSocket,
            source_event_id,
            source_session_id: self.current_session_id.clone(),
            sequence,
        }
    }

    fn intern_asset_id(&mut self, raw: &str) -> AssetId {
        if let Some(cached) = self.asset_id_cache.get(raw) {
            cached.clone()
        } else {
            let id = AssetId::new(raw);
            self.asset_id_cache.insert(id.0.clone(), id.clone());
            id
        }
    }

    fn next_sequence_for(&mut self, asset_id: &AssetId) -> Sequence {
        if let Some(seq) = self.asset_sequences.get_mut(asset_id.as_str()) {
            let current = *seq;
            *seq += 1;
            Sequence::new(current)
        } else {
            self.asset_sequences.insert(asset_id.0.clone(), 1);
            Sequence::new(0)
        }
    }

    async fn send(&self, record: PersistedRecord) -> Result<(), FeedError> {
        pb_metrics::record_message_received(record_label(&record));

        match &record {
            PersistedRecord::Book(event) => match event.kind {
                BookEventKind::Snapshot => pb_metrics::record_snapshot_applied(),
                BookEventKind::Delta => pb_metrics::record_delta_applied(),
            },
            PersistedRecord::Trade(_) => pb_metrics::record_trade_received(),
            PersistedRecord::Ingest(event) => {
                if event.kind == IngestEventKind::SequenceGap {
                    pb_metrics::record_gap_detected();
                }
            }
            PersistedRecord::Checkpoint(_)
            | PersistedRecord::Validation(_)
            | PersistedRecord::Execution(_) => {}
        }

        let latency_pair = match &record {
            PersistedRecord::Book(event) => Some((
                event.provenance.recv_timestamp_us,
                event.provenance.exchange_timestamp_us,
            )),
            PersistedRecord::Trade(event) => Some((
                event.provenance.recv_timestamp_us,
                event.provenance.exchange_timestamp_us,
            )),
            PersistedRecord::Ingest(event) => Some((
                event.provenance.recv_timestamp_us,
                event.provenance.exchange_timestamp_us,
            )),
            PersistedRecord::Checkpoint(_)
            | PersistedRecord::Validation(_)
            | PersistedRecord::Execution(_) => None,
        };
        if let Some((recv, exchange)) = latency_pair {
            if exchange > 0 && recv > exchange {
                pb_metrics::record_ws_latency_us((recv - exchange) as f64);
            }
        }

        self.tx.send(record).await.map_err(|_| {
            error!("output channel closed");
            FeedError::ChannelSend
        })
    }
}

fn parse_timestamp_us(ts: Option<&str>) -> u64 {
    let raw = ts.and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    if raw > 0 && raw < 10_000_000_000_000 {
        raw.saturating_mul(1000)
    } else {
        raw
    }
}

fn parse_trade_side(raw: &str) -> Option<Side> {
    match raw {
        "BUY" | "buy" | "Bid" | "bid" => Some(Side::Bid),
        "SELL" | "sell" | "Ask" | "ask" => Some(Side::Ask),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_message(text: String) -> FeedMessage {
        FeedMessage::Raw(WsRawMessage {
            text,
            recv_timestamp_us: 1_700_000_000_000_000,
        })
    }

    #[tokio::test]
    async fn snapshot_resets_existing_asset_sequence_counter() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher.asset_sequences.insert(Arc::from("tok1"), 99);

        let msg = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "1700000000000000",
            "bids": [{"price": "0.50", "size": "10"}],
            "asks": [{"price": "0.60", "size": "20"}]
        });

        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        let first = event_rx.recv().await.unwrap();
        let second = event_rx.recv().await.unwrap();

        let first_seq = match first {
            PersistedRecord::Book(event) => event.provenance.sequence.unwrap().raw(),
            other => panic!("unexpected record: {other:?}"),
        };
        let second_seq = match second {
            PersistedRecord::Book(event) => event.provenance.sequence.unwrap().raw(),
            other => panic!("unexpected record: {other:?}"),
        };

        assert_eq!(first_seq, 0);
        assert_eq!(second_seq, 1);
        assert_eq!(dispatcher.asset_sequences.get("tok1"), Some(&2));
    }

    #[tokio::test]
    async fn stale_snapshot_is_persisted_as_ingest_event() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg1 = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "100",
            "bids": [{"price": "0.50", "size": "10"}],
            "asks": []
        });
        dispatcher
            .dispatch(raw_message(msg1.to_string()))
            .await
            .unwrap();
        let _ = event_rx.recv().await.unwrap();

        let msg2 = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "50",
            "bids": [{"price": "0.60", "size": "20"}],
            "asks": []
        });
        dispatcher
            .dispatch(raw_message(msg2.to_string()))
            .await
            .unwrap();

        match event_rx.recv().await.unwrap() {
            PersistedRecord::Ingest(event) => {
                assert_eq!(event.kind, IngestEventKind::StaleSnapshotSkip);
            }
            other => panic!("expected ingest event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn trade_event_marks_partial_fidelity_when_side_or_size_missing() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "last_trade_price",
            "asset_id": "tok1",
            "price": "0.60"
        });

        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        match event_rx.recv().await.unwrap() {
            PersistedRecord::Trade(event) => {
                assert_eq!(event.fidelity, TradeFidelity::Partial);
                assert!(event.size.is_none());
                assert!(event.side.is_none());
            }
            other => panic!("expected trade event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn lifecycle_events_are_forwarded_as_ingest_records() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher
            .dispatch(FeedMessage::Lifecycle(WsLifecycleEvent {
                kind: WsLifecycleKind::ReconnectSuccess,
                recv_timestamp_us: 10,
                session_id: "session-1".to_string(),
                details: None,
            }))
            .await
            .unwrap();

        match event_rx.recv().await.unwrap() {
            PersistedRecord::Ingest(event) => {
                assert_eq!(event.kind, IngestEventKind::ReconnectSuccess)
            }
            other => panic!("expected reconnect success, got {other:?}"),
        }
        match event_rx.recv().await.unwrap() {
            PersistedRecord::Ingest(event) => assert_eq!(event.kind, IngestEventKind::SourceReset),
            other => panic!("expected source reset, got {other:?}"),
        }
    }
}
