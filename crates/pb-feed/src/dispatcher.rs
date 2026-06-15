use std::sync::Arc;

use rustc_hash::FxHashMap;

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

/// Parse a side string from the venue into the internal `Side` enum.
/// Returns `None` for unrecognized values.
fn parse_side(raw: &str) -> Option<Side> {
    match raw {
        "BUY" | "buy" | "Bid" | "bid" => Some(Side::Bid),
        "SELL" | "sell" | "Ask" | "ask" => Some(Side::Ask),
        _ => None,
    }
}

pub struct Dispatcher {
    rx: mpsc::Receiver<FeedMessage>,
    tx: mpsc::Sender<PersistedRecord>,
    /// Per-asset monotonic sequence counters. Snapshots reset the counter.
    asset_sequences: FxHashMap<Arc<str>, u64>,
    /// Per-asset last snapshot exchange timestamp for staleness detection.
    last_snapshot_ts: FxHashMap<Arc<str>, u64>,
    /// Per-asset last accepted snapshot venue hash, used to deduplicate exact
    /// retransmits of identical book state (e.g. an equal-timestamp duplicate).
    last_snapshot_hash: FxHashMap<Arc<str>, String>,
    /// Interned AssetIds to avoid heap allocation on every message.
    asset_id_cache: FxHashMap<Arc<str>, AssetId>,
    current_session_id: Option<String>,
}

impl Dispatcher {
    pub fn new(rx: mpsc::Receiver<FeedMessage>, tx: mpsc::Sender<PersistedRecord>) -> Self {
        Self {
            rx,
            tx,
            asset_sequences: FxHashMap::default(),
            last_snapshot_ts: FxHashMap::default(),
            last_snapshot_hash: FxHashMap::default(),
            asset_id_cache: FxHashMap::default(),
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
            self.reset_continuity_state();
        }

        let provenance = EventProvenance {
            recv_timestamp_us: event.recv_timestamp_us,
            exchange_timestamp_us: 0,
            source: DataSource::WebSocket,
            source_event_id: None,
            source_session_id: Some(event.session_id.clone()),
            sequence: None,
            ingest_ordinal: None,
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

    fn reset_continuity_state(&mut self) {
        self.asset_sequences.clear();
        self.last_snapshot_ts.clear();
        self.last_snapshot_hash.clear();
    }

    async fn dispatch_raw(&mut self, raw: &WsRawMessage) -> Result<(), FeedError> {
        let msg: WsMessage<'_> = match serde_json::from_str(&raw.text) {
            Ok(m) => m,
            Err(e) => {
                // A frame that matches no known message type is dropped. Meter it
                // so silent loss of new/unknown venue message types is visible
                // (audit finding A.110) instead of vanishing at debug level.
                pb_metrics::record_unknown_message_dropped();
                debug!("skipping non-event message: {e}");
                return Ok(());
            }
        };

        match msg {
            WsMessage::Book(book) => {
                let asset_id = self.intern_asset_id(book.asset_id);
                let exchange_ts = parse_timestamp_us(book.timestamp);
                let source_event_id = book.hash.map(str::to_string);

                // Skip only *strictly older* snapshots. Polymarket emits one
                // `book` event per trade at millisecond resolution, so two
                // trades in the same millisecond produce equal-timestamp
                // snapshots whose later one carries the newer state. Dropping
                // equal timestamps (the old `<=`) silently lost that state in
                // exactly the high-activity bursts that matter (A.21).
                if exchange_ts > 0 {
                    if let Some(&last_ts) = self.last_snapshot_ts.get(asset_id.as_str()) {
                        if exchange_ts < last_ts {
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
                                    "snapshot exchange timestamp {} < latest accepted {}",
                                    exchange_ts, last_ts
                                )),
                            }))
                            .await?;
                            return Ok(());
                        }
                    }
                }

                // Deduplicate exact retransmits of identical book state by venue
                // hash, so an equal-timestamp duplicate does not re-emit a
                // redundant snapshot.
                if let Some(ref hash) = source_event_id {
                    if self
                        .last_snapshot_hash
                        .get(asset_id.as_str())
                        .map(String::as_str)
                        == Some(hash.as_str())
                    {
                        pb_metrics::record_stale_snapshot_skipped();
                        return Ok(());
                    }
                }

                // Convert every level up-front. A mid-message conversion failure
                // must not leave a truncated snapshot downstream (which would be
                // indistinguishable from a complete one) and must not poison the
                // staleness tracker, so on error we emit a continuity-reset
                // marker and leave all tracker state untouched (A.108).
                let mut levels: Vec<(Side, FixedPrice, FixedSize)> =
                    Vec::with_capacity(book.bids.len() + book.asks.len());
                let mut conversion_error: Option<FeedError> = None;
                'convert: for (side, entries) in [(Side::Bid, &book.bids), (Side::Ask, &book.asks)]
                {
                    for entry in entries {
                        match (
                            FixedPrice::try_from(entry.price),
                            FixedSize::try_from(entry.size),
                        ) {
                            (Ok(price), Ok(size)) => levels.push((side, price, size)),
                            (Err(e), _) | (_, Err(e)) => {
                                conversion_error = Some(e.into());
                                break 'convert;
                            }
                        }
                    }
                }

                if let Some(e) = conversion_error {
                    warn!(
                        asset_id = %asset_id,
                        error = %e,
                        "book snapshot conversion failed; emitting continuity reset instead of a partial snapshot"
                    );
                    pb_metrics::record_stale_snapshot_skipped();
                    self.send(PersistedRecord::Ingest(IngestEvent {
                        asset_id: Some(asset_id),
                        kind: IngestEventKind::SourceReset,
                        provenance: self.make_provenance(
                            raw.recv_timestamp_us,
                            exchange_ts,
                            source_event_id,
                            None,
                        ),
                        expected_sequence: None,
                        observed_sequence: None,
                        details: Some(format!("book snapshot dropped (unparseable level): {e}")),
                    }))
                    .await?;
                    return Ok(());
                }

                // Full conversion succeeded — commit atomically: advance the
                // staleness tracker, record the hash, reset the sequence
                // counter, then emit the snapshot.
                if exchange_ts > 0 {
                    self.last_snapshot_ts
                        .insert(asset_id.0.clone(), exchange_ts);
                }
                if let Some(ref hash) = source_event_id {
                    self.last_snapshot_hash
                        .insert(asset_id.0.clone(), hash.clone());
                }
                pb_metrics::record_snapshot_reconciled();
                self.asset_sequences.insert(asset_id.0.clone(), 0);

                for (side, price, size) in levels {
                    let sequence = self.next_sequence_for(&asset_id);
                    let event = BookEvent {
                        asset_id: asset_id.clone(),
                        kind: BookEventKind::Snapshot,
                        side,
                        price,
                        size,
                        provenance: self.make_provenance(
                            raw.recv_timestamp_us,
                            exchange_ts,
                            source_event_id.clone(),
                            Some(sequence),
                        ),
                    };
                    self.send(PersistedRecord::Book(event)).await?;
                }
            }
            WsMessage::PriceChange(pc) => {
                let exchange_ts = parse_timestamp_us(pc.timestamp);
                for entry in &pc.price_changes {
                    let side = match parse_side(entry.side) {
                        Some(s) => s,
                        None => {
                            warn!(side = entry.side, "unknown side string, skipping delta");
                            continue;
                        }
                    };

                    let asset_id = self.intern_asset_id(entry.asset_id);
                    // Skip an unparseable price/size entry instead of aborting
                    // the whole price_change batch (which would also drop the
                    // valid entries after it). Deltas are independent level
                    // updates, so this matches the existing bad-side handling
                    // (A.108).
                    let (price, size) = match (
                        FixedPrice::try_from(entry.price),
                        FixedSize::try_from(entry.size),
                    ) {
                        (Ok(price), Ok(size)) => (price, size),
                        (Err(e), _) | (_, Err(e)) => {
                            warn!(
                                asset_id = %asset_id,
                                error = %e,
                                "price_change entry conversion failed, skipping delta"
                            );
                            pb_metrics::record_stale_snapshot_skipped();
                            continue;
                        }
                    };

                    let sequence = self.next_sequence_for(&asset_id);
                    let event = BookEvent {
                        asset_id,
                        kind: BookEventKind::Delta,
                        side,
                        price,
                        size,
                        provenance: self.make_provenance(
                            raw.recv_timestamp_us,
                            exchange_ts,
                            entry.hash.map(str::to_string),
                            Some(sequence),
                        ),
                    };
                    self.send(PersistedRecord::Book(event)).await?;
                }
            }
            WsMessage::LastTradePrice(lt) => {
                let asset_id = self.intern_asset_id(lt.asset_id);
                let size = lt.size.map(FixedSize::try_from).transpose()?;
                let side = lt.side.and_then(parse_side);
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
            WsMessage::TickSizeChange(t) => {
                // V2 event: market's minimum tick size changed (typically
                // when price crosses 0.04 / 0.96). Informational only — our
                // book engine stores prices at full FixedPrice precision and
                // does not enforce a min tick.
                pb_metrics::record_message_received("tick_size_change");
                debug!(
                    asset_id = t.asset_id,
                    old = ?t.old_tick_size,
                    new = ?t.new_tick_size,
                    "tick size changed"
                );
            }
        }

        Ok(())
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
            ingest_ordinal: None,
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
        let label = record_label(&record);
        pb_metrics::record_message_received(label);

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

/// Parse a venue timestamp into microseconds. Delegates to the single shared
/// converter so the dispatcher and the REST backfill agree on every resolution
/// (seconds/ms/µs/ns) and on the zero case (audit findings A.119/A.147). Absent
/// or non-numeric input becomes `0` (the "unknown timestamp" sentinel).
fn parse_timestamp_us(ts: Option<&str>) -> u64 {
    pb_types::time::parse_to_micros(ts).unwrap_or(0)
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
    async fn same_millisecond_snapshot_is_accepted_not_dropped() {
        // Two snapshots with equal exchange timestamps (two trades within the
        // same millisecond): the second must be applied, not dropped (A.21).
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let first = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "100",
            "hash": "h1",
            "bids": [{"price": "0.50", "size": "10"}],
            "asks": []
        });
        dispatcher
            .dispatch(raw_message(first.to_string()))
            .await
            .unwrap();
        match event_rx.recv().await.unwrap() {
            PersistedRecord::Book(e) => assert_eq!(e.kind, BookEventKind::Snapshot),
            other => panic!("expected snapshot, got {other:?}"),
        }

        // Same timestamp, different state (different hash) — must be accepted.
        let second = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "100",
            "hash": "h2",
            "bids": [{"price": "0.51", "size": "11"}],
            "asks": []
        });
        dispatcher
            .dispatch(raw_message(second.to_string()))
            .await
            .unwrap();
        match event_rx.recv().await.unwrap() {
            PersistedRecord::Book(e) => {
                assert_eq!(e.kind, BookEventKind::Snapshot);
                assert_eq!(e.price, FixedPrice::try_from("0.51").unwrap());
            }
            other => panic!("expected accepted same-ms snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn identical_snapshot_retransmit_is_deduped_by_hash() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let snap = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "100",
            "hash": "same-hash",
            "bids": [{"price": "0.50", "size": "10"}],
            "asks": []
        });
        dispatcher
            .dispatch(raw_message(snap.to_string()))
            .await
            .unwrap();
        assert!(matches!(
            event_rx.recv().await.unwrap(),
            PersistedRecord::Book(_)
        ));

        // Exact retransmit (same hash) — deduped, no new record emitted.
        dispatcher
            .dispatch(raw_message(snap.to_string()))
            .await
            .unwrap();
        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn snapshot_with_unparseable_level_emits_reset_not_partial() {
        // A snapshot whose second level is unparseable must produce ZERO book
        // events (no partial snapshot) and a single continuity-reset marker,
        // and must not advance the staleness tracker (A.108).
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "100",
            "bids": [{"price": "0.50", "size": "10"}, {"price": "not-a-number", "size": "5"}],
            "asks": []
        });
        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        match event_rx.recv().await.unwrap() {
            PersistedRecord::Ingest(e) => assert_eq!(e.kind, IngestEventKind::SourceReset),
            other => panic!("expected source-reset marker, got {other:?}"),
        }
        // No book events at all (not even the first, valid bid).
        assert!(event_rx.try_recv().is_err());
        // Tracker untouched, so a subsequent valid snapshot at the same ts is
        // still accepted.
        assert!(!dispatcher.last_snapshot_ts.contains_key("tok1"));
    }

    #[tokio::test]
    async fn price_change_skips_bad_entry_keeps_valid_ones() {
        // A price_change batch with one unparseable entry must still emit the
        // valid deltas around it (A.108), not abort the whole batch.
        let (_raw_tx, raw_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "price_change",
            "timestamp": "1700000000000000",
            "price_changes": [
                {"asset_id": "tok1", "price": "0.50", "size": "10", "side": "BUY"},
                {"asset_id": "tok1", "price": "bad", "size": "1", "side": "BUY"},
                {"asset_id": "tok1", "price": "0.55", "size": "20", "side": "SELL"}
            ]
        });
        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        // First valid delta.
        match event_rx.recv().await.unwrap() {
            PersistedRecord::Book(e) => assert_eq!(e.price, FixedPrice::try_from("0.50").unwrap()),
            other => panic!("expected first delta, got {other:?}"),
        }
        // Third valid delta (second was skipped).
        match event_rx.recv().await.unwrap() {
            PersistedRecord::Book(e) => assert_eq!(e.price, FixedPrice::try_from("0.55").unwrap()),
            other => panic!("expected third delta, got {other:?}"),
        }
        assert!(event_rx.try_recv().is_err());
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

    // ---- parse_side tests ----

    #[test]
    fn parse_side_buy_variants() {
        assert_eq!(parse_side("BUY"), Some(Side::Bid));
        assert_eq!(parse_side("buy"), Some(Side::Bid));
        assert_eq!(parse_side("Bid"), Some(Side::Bid));
        assert_eq!(parse_side("bid"), Some(Side::Bid));
    }

    #[test]
    fn parse_side_sell_variants() {
        assert_eq!(parse_side("SELL"), Some(Side::Ask));
        assert_eq!(parse_side("sell"), Some(Side::Ask));
        assert_eq!(parse_side("Ask"), Some(Side::Ask));
        assert_eq!(parse_side("ask"), Some(Side::Ask));
    }

    #[test]
    fn parse_side_invalid_returns_none() {
        assert_eq!(parse_side(""), None);
        assert_eq!(parse_side("Buy"), None);
        assert_eq!(parse_side("Sell"), None);
        assert_eq!(parse_side("BUYING"), None);
        assert_eq!(parse_side("bids"), None);
        assert_eq!(parse_side("unknown"), None);
        assert_eq!(parse_side("0"), None);
        assert_eq!(parse_side(" BUY"), None);
        assert_eq!(parse_side("BUY "), None);
    }

    // ---- parse_timestamp_us tests ----

    #[test]
    fn parse_timestamp_us_none_returns_zero() {
        assert_eq!(parse_timestamp_us(None), 0);
    }

    #[test]
    fn parse_timestamp_us_non_numeric_returns_zero() {
        assert_eq!(parse_timestamp_us(Some("not-a-number")), 0);
        assert_eq!(parse_timestamp_us(Some("")), 0);
    }

    #[test]
    fn parse_timestamp_us_millis_converted_to_micros() {
        // A 13-digit millisecond timestamp should be multiplied by 1000.
        let ts = parse_timestamp_us(Some("1700000000000"));
        assert_eq!(ts, 1_700_000_000_000_000);
    }

    #[test]
    fn parse_timestamp_us_micros_left_unchanged() {
        // A 16-digit microsecond timestamp should pass through unchanged.
        let ts = parse_timestamp_us(Some("1700000000000000"));
        assert_eq!(ts, 1_700_000_000_000_000);
    }

    #[test]
    fn parse_timestamp_us_zero_returns_zero() {
        assert_eq!(parse_timestamp_us(Some("0")), 0);
    }

    // ---- Dispatcher: malformed JSON handling ----

    #[tokio::test]
    async fn dispatch_ignores_empty_string() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher
            .dispatch(raw_message(String::new()))
            .await
            .unwrap();

        // No events should have been produced.
        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_ignores_plain_text() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher
            .dispatch(raw_message("hello world".to_string()))
            .await
            .unwrap();

        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_ignores_empty_json_object() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher
            .dispatch(raw_message("{}".to_string()))
            .await
            .unwrap();

        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_ignores_json_null() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher
            .dispatch(raw_message("null".to_string()))
            .await
            .unwrap();

        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_ignores_unknown_event_type() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "unknown_event",
            "data": "something"
        });
        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_handles_v2_tick_size_change() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "tick_size_change",
            "asset_id": "tok1",
            "market": "0xabc",
            "old_tick_size": "0.01",
            "new_tick_size": "0.001",
            "timestamp": "1700000000000"
        });
        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        // V2 tick size events are observational; no PersistedRecord is emitted.
        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_ignores_missing_event_type() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "asset_id": "tok1",
            "bids": [],
            "asks": []
        });
        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_ignores_json_array() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher
            .dispatch(raw_message("[1,2,3]".to_string()))
            .await
            .unwrap();

        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dispatch_ignores_nested_garbage() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "bids": "not_an_array",
            "asks": 42
        });
        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        assert!(event_rx.try_recv().is_err());
    }

    // ---- Dispatcher: price_change with invalid side ----

    #[tokio::test]
    async fn delta_with_unknown_side_is_skipped() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "price_change",
            "timestamp": "1700000000000000",
            "price_changes": [{
                "asset_id": "tok1",
                "price": "0.50",
                "size": "10",
                "side": "INVALID_SIDE"
            }]
        });

        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        // The delta should be silently skipped.
        assert!(event_rx.try_recv().is_err());
    }

    // ---- Dispatcher: trade with full fidelity ----

    #[tokio::test]
    async fn trade_event_marks_full_fidelity_when_side_and_size_present() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "last_trade_price",
            "asset_id": "tok1",
            "price": "0.60",
            "size": "100",
            "side": "BUY"
        });

        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        match event_rx.recv().await.unwrap() {
            PersistedRecord::Trade(event) => {
                assert_eq!(event.fidelity, TradeFidelity::Full);
                assert!(event.size.is_some());
                assert_eq!(event.side, Some(Side::Bid));
            }
            other => panic!("expected trade event, got {other:?}"),
        }
    }

    // ---- Dispatcher: asset_id interning ----

    #[tokio::test]
    async fn asset_id_is_interned_across_messages() {
        let (_raw_tx, raw_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        for _ in 0..3 {
            let msg = serde_json::json!({
                "event_type": "last_trade_price",
                "asset_id": "tok1",
                "price": "0.60"
            });
            dispatcher
                .dispatch(raw_message(msg.to_string()))
                .await
                .unwrap();
        }

        // Should have exactly 1 entry in the cache despite 3 messages.
        assert_eq!(dispatcher.asset_id_cache.len(), 1);
        assert!(dispatcher.asset_id_cache.contains_key("tok1"));

        // All 3 events should have been produced.
        for _ in 0..3 {
            assert!(event_rx.try_recv().is_ok());
        }
    }

    // ---- Dispatcher: empty dispatcher lookups ----

    #[tokio::test]
    async fn empty_dispatcher_handles_unknown_asset() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        // Sequences for a brand new asset start at 0.
        let msg = serde_json::json!({
            "event_type": "price_change",
            "timestamp": "1700000000000000",
            "price_changes": [{
                "asset_id": "never-seen",
                "price": "0.50",
                "size": "10",
                "side": "BUY"
            }]
        });

        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        match event_rx.recv().await.unwrap() {
            PersistedRecord::Book(event) => {
                assert_eq!(event.provenance.sequence.unwrap().raw(), 0);
                assert_eq!(event.asset_id.as_str(), "never-seen");
            }
            other => panic!("expected book event, got {other:?}"),
        }
    }

    // ---- Dispatcher: sequence numbering across assets ----

    #[tokio::test]
    async fn sequences_are_independent_per_asset() {
        let (_raw_tx, raw_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        for asset in &["tok1", "tok2"] {
            for _ in 0..2 {
                let msg = serde_json::json!({
                    "event_type": "price_change",
                    "timestamp": "1700000000000000",
                    "price_changes": [{
                        "asset_id": asset,
                        "price": "0.50",
                        "size": "10",
                        "side": "BUY"
                    }]
                });
                dispatcher
                    .dispatch(raw_message(msg.to_string()))
                    .await
                    .unwrap();
            }
        }

        // tok1: seq 0, 1
        for expected_seq in [0, 1] {
            match event_rx.recv().await.unwrap() {
                PersistedRecord::Book(event) => {
                    assert_eq!(event.asset_id.as_str(), "tok1");
                    assert_eq!(event.provenance.sequence.unwrap().raw(), expected_seq);
                }
                other => panic!("expected book event, got {other:?}"),
            }
        }
        // tok2: seq 0, 1
        for expected_seq in [0, 1] {
            match event_rx.recv().await.unwrap() {
                PersistedRecord::Book(event) => {
                    assert_eq!(event.asset_id.as_str(), "tok2");
                    assert_eq!(event.provenance.sequence.unwrap().raw(), expected_seq);
                }
                other => panic!("expected book event, got {other:?}"),
            }
        }
    }

    // ---- Dispatcher: lifecycle ReconnectStart ----

    #[tokio::test]
    async fn reconnect_start_emits_single_ingest_event() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher
            .dispatch(FeedMessage::Lifecycle(WsLifecycleEvent {
                kind: WsLifecycleKind::ReconnectStart,
                recv_timestamp_us: 10,
                session_id: "session-1".to_string(),
                details: Some("attempt=0".to_string()),
            }))
            .await
            .unwrap();

        match event_rx.recv().await.unwrap() {
            PersistedRecord::Ingest(event) => {
                assert_eq!(event.kind, IngestEventKind::ReconnectStart);
                assert_eq!(event.details, Some("attempt=0".to_string()));
            }
            other => panic!("expected reconnect start, got {other:?}"),
        }

        // ReconnectStart should NOT emit SourceReset.
        assert!(event_rx.try_recv().is_err());
    }

    // ---- Dispatcher: session tracking ----

    #[tokio::test]
    async fn reconnect_success_updates_session_id() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        assert!(dispatcher.current_session_id.is_none());

        dispatcher
            .dispatch(FeedMessage::Lifecycle(WsLifecycleEvent {
                kind: WsLifecycleKind::ReconnectSuccess,
                recv_timestamp_us: 10,
                session_id: "new-session".to_string(),
                details: None,
            }))
            .await
            .unwrap();

        assert_eq!(
            dispatcher.current_session_id.as_deref(),
            Some("new-session")
        );
    }

    #[tokio::test]
    async fn reconnect_success_clears_sequence_and_snapshot_state() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);
        dispatcher.asset_sequences.insert(Arc::from("tok1"), 42);
        dispatcher
            .last_snapshot_ts
            .insert(Arc::from("tok1"), 123_456);

        dispatcher
            .dispatch(FeedMessage::Lifecycle(WsLifecycleEvent {
                kind: WsLifecycleKind::ReconnectSuccess,
                recv_timestamp_us: 10,
                session_id: "new-session".to_string(),
                details: None,
            }))
            .await
            .unwrap();

        assert!(dispatcher.asset_sequences.is_empty());
        assert!(dispatcher.last_snapshot_ts.is_empty());
    }

    #[tokio::test]
    async fn snapshot_after_reconnect_is_not_treated_as_stale() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let initial = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "100",
            "bids": [{"price": "0.50", "size": "10"}],
            "asks": []
        });
        dispatcher
            .dispatch(raw_message(initial.to_string()))
            .await
            .unwrap();
        let _ = event_rx.recv().await.unwrap();

        dispatcher
            .dispatch(FeedMessage::Lifecycle(WsLifecycleEvent {
                kind: WsLifecycleKind::ReconnectSuccess,
                recv_timestamp_us: 20,
                session_id: "session-2".to_string(),
                details: None,
            }))
            .await
            .unwrap();
        let _ = event_rx.recv().await.unwrap();
        let _ = event_rx.recv().await.unwrap();

        let after_reconnect = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "50",
            "bids": [{"price": "0.60", "size": "20"}],
            "asks": []
        });
        dispatcher
            .dispatch(raw_message(after_reconnect.to_string()))
            .await
            .unwrap();

        match event_rx.recv().await.unwrap() {
            PersistedRecord::Book(event) => {
                assert_eq!(event.kind, BookEventKind::Snapshot);
                assert_eq!(event.provenance.sequence.unwrap().raw(), 0);
            }
            other => panic!("expected fresh snapshot after reset, got {other:?}"),
        }
    }

    // ---- Dispatcher: book with empty bids and asks ----

    #[tokio::test]
    async fn snapshot_with_empty_bids_and_asks_resets_sequence() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        dispatcher.asset_sequences.insert(Arc::from("tok1"), 50);

        let msg = serde_json::json!({
            "event_type": "book",
            "asset_id": "tok1",
            "timestamp": "1700000000000000",
            "bids": [],
            "asks": []
        });

        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        // No book events emitted (empty bids/asks), but sequence should still reset.
        assert!(event_rx.try_recv().is_err());
        assert_eq!(dispatcher.asset_sequences.get("tok1"), Some(&0));
    }

    // ---- Dispatcher: output channel closed ----

    #[tokio::test]
    async fn dispatch_returns_channel_send_when_output_closed() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        // Drop the receiver to close the channel.
        drop(event_rx);

        let msg = serde_json::json!({
            "event_type": "last_trade_price",
            "asset_id": "tok1",
            "price": "0.60"
        });

        let result = dispatcher.dispatch(raw_message(msg.to_string())).await;
        assert!(matches!(result, Err(FeedError::ChannelSend)));
    }

    // ---- Dispatcher: cancellation token shuts down run loop ----

    #[tokio::test]
    async fn run_with_token_shuts_down_on_cancellation() {
        let (_raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let token = CancellationToken::new();
        let token_clone = token.clone();

        // Cancel immediately.
        token_clone.cancel();

        let result = dispatcher.run_with_token(token).await;
        assert!(result.is_ok());
    }

    // ---- Dispatcher: run exits when input channel closes ----

    #[tokio::test]
    async fn run_exits_when_input_channel_closes() {
        let (raw_tx, raw_rx) = mpsc::channel(8);
        let (event_tx, _event_rx) = mpsc::channel(8);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        // Drop the sender to close the input channel.
        drop(raw_tx);

        let result = dispatcher.run().await;
        assert!(result.is_ok());
    }

    // ---- Dispatcher: price_change with multiple entries ----

    #[tokio::test]
    async fn price_change_with_multiple_entries_emits_multiple_book_events() {
        let (_raw_tx, raw_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let mut dispatcher = Dispatcher::new(raw_rx, event_tx);

        let msg = serde_json::json!({
            "event_type": "price_change",
            "timestamp": "1700000000000000",
            "price_changes": [
                {"asset_id": "tok1", "price": "0.50", "size": "10", "side": "BUY"},
                {"asset_id": "tok1", "price": "0.55", "size": "20", "side": "SELL"},
                {"asset_id": "tok2", "price": "0.30", "size": "5", "side": "buy"}
            ]
        });

        dispatcher
            .dispatch(raw_message(msg.to_string()))
            .await
            .unwrap();

        // Should emit 3 book events.
        let e1 = event_rx.recv().await.unwrap();
        let e2 = event_rx.recv().await.unwrap();
        let e3 = event_rx.recv().await.unwrap();

        match e1 {
            PersistedRecord::Book(event) => {
                assert_eq!(event.side, Side::Bid);
                assert_eq!(event.kind, BookEventKind::Delta);
            }
            other => panic!("expected book event, got {other:?}"),
        }
        match e2 {
            PersistedRecord::Book(event) => {
                assert_eq!(event.side, Side::Ask);
            }
            other => panic!("expected book event, got {other:?}"),
        }
        match e3 {
            PersistedRecord::Book(event) => {
                assert_eq!(event.asset_id.as_str(), "tok2");
                assert_eq!(event.side, Side::Bid);
            }
            other => panic!("expected book event, got {other:?}"),
        }
    }

    // ---- record_label coverage ----

    #[test]
    fn record_label_covers_all_variants() {
        use pb_types::event::{
            BookCheckpoint, ExecutionEvent, ExecutionEventKind, LatencyTrace, ReplayMode,
            ReplayValidation,
        };

        let prov = EventProvenance {
            recv_timestamp_us: 0,
            exchange_timestamp_us: 0,
            source: DataSource::WebSocket,
            source_event_id: None,
            source_session_id: None,
            sequence: None,
            ingest_ordinal: None,
        };

        assert_eq!(
            record_label(&PersistedRecord::Book(BookEvent {
                asset_id: AssetId::new("t"),
                kind: BookEventKind::Snapshot,
                side: Side::Bid,
                price: FixedPrice::new(1).unwrap(),
                size: FixedSize::from_f64(1.0).unwrap(),
                provenance: prov.clone(),
            })),
            "snapshot"
        );
        assert_eq!(
            record_label(&PersistedRecord::Book(BookEvent {
                asset_id: AssetId::new("t"),
                kind: BookEventKind::Delta,
                side: Side::Ask,
                price: FixedPrice::new(1).unwrap(),
                size: FixedSize::from_f64(1.0).unwrap(),
                provenance: prov.clone(),
            })),
            "delta"
        );
        assert_eq!(
            record_label(&PersistedRecord::Trade(TradeEvent {
                asset_id: AssetId::new("t"),
                price: FixedPrice::new(1).unwrap(),
                size: None,
                side: None,
                trade_id: None,
                fidelity: TradeFidelity::Partial,
                provenance: prov.clone(),
            })),
            "trade"
        );
        assert_eq!(
            record_label(&PersistedRecord::Ingest(IngestEvent {
                asset_id: None,
                kind: IngestEventKind::ReconnectStart,
                provenance: prov.clone(),
                expected_sequence: None,
                observed_sequence: None,
                details: None,
            })),
            "ingest"
        );
        assert_eq!(
            record_label(&PersistedRecord::Checkpoint(BookCheckpoint {
                asset_id: AssetId::new("t"),
                checkpoint_timestamp_us: 0,
                provenance: prov.clone(),
                bids: vec![],
                asks: vec![],
                wal_offset: None,
            })),
            "checkpoint"
        );
        assert_eq!(
            record_label(&PersistedRecord::Validation(ReplayValidation {
                asset_id: AssetId::new("t"),
                mode: ReplayMode::RecvTime,
                replay_timestamp_us: 0,
                reference_timestamp_us: 0,
                matched: true,
                mismatch_summary: None,
                persisted_at_us: 0,
            })),
            "validation"
        );
        assert_eq!(
            record_label(&PersistedRecord::Execution(ExecutionEvent {
                event_timestamp_us: 0,
                asset_id: None,
                order_id: "o1".to_string(),
                client_order_id: None,
                venue_order_id: None,
                kind: ExecutionEventKind::SubmitIntent,
                side: None,
                price: None,
                size: None,
                status: None,
                reason: None,
                latency: LatencyTrace::default(),
            })),
            "execution"
        );
    }
}
