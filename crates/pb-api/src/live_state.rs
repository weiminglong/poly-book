use std::collections::HashMap;
use std::sync::Arc;

use pb_book::L2Book;
use pb_types::event::{BookEvent, BookEventKind, IngestEvent, PersistedRecord};
use pb_types::{AssetId, FixedPrice, FixedSize};
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::dto::{
    ActiveAssetSummary, ContinuityWarning, FeedMode, FeedStatusResponse, LiveOrderBookSnapshot,
    PriceLevelView, SessionStatus,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotGroupKey {
    asset_id: String,
    recv_timestamp_us: u64,
    exchange_timestamp_us: u64,
    source_event_id: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingSnapshot {
    key: SnapshotGroupKey,
    bids: Vec<(FixedPrice, FixedSize)>,
    asks: Vec<(FixedPrice, FixedSize)>,
    sequence: u64,
    last_recv_timestamp_us: u64,
}

#[derive(Debug, Clone)]
struct AssetState {
    book: L2Book,
    initialized_from_snapshot: bool,
    last_recv_timestamp_us: Option<u64>,
    last_exchange_timestamp_us: Option<u64>,
    latest_warning: Option<ContinuityWarning>,
}

impl AssetState {
    fn new(asset_id: &str) -> Self {
        Self {
            book: L2Book::new(AssetId::new(asset_id)),
            initialized_from_snapshot: false,
            last_recv_timestamp_us: None,
            last_exchange_timestamp_us: None,
            latest_warning: None,
        }
    }
}

#[derive(Debug)]
struct LiveState {
    mode: FeedMode,
    session_status: SessionStatus,
    current_session_id: Option<String>,
    active_assets: Vec<String>,
    assets: HashMap<String, AssetState>,
    pending_snapshots: HashMap<String, PendingSnapshot>,
    last_rotation_us: Option<u64>,
    latest_global_warning: Option<ContinuityWarning>,
}

impl LiveState {
    fn new(mode: FeedMode) -> Self {
        Self {
            mode,
            session_status: SessionStatus::Starting,
            current_session_id: None,
            active_assets: Vec::new(),
            assets: HashMap::new(),
            pending_snapshots: HashMap::new(),
            last_rotation_us: None,
            latest_global_warning: None,
        }
    }

    fn ensure_asset(&mut self, asset_id: &str) -> &mut AssetState {
        self.assets
            .entry(asset_id.to_string())
            .or_insert_with(|| AssetState::new(asset_id))
    }

    fn set_active_assets(&mut self, assets: Vec<String>) {
        self.active_assets = assets;
        let mut retained = HashMap::new();
        for asset_id in &self.active_assets {
            let state = self
                .assets
                .remove(asset_id)
                .unwrap_or_else(|| AssetState::new(asset_id));
            retained.insert(asset_id.clone(), state);
        }
        self.assets = retained;
        self.pending_snapshots.retain(|asset_id, _| {
            self.active_assets
                .iter()
                .any(|candidate| candidate == asset_id)
        });
    }

    fn materialize_all_pending(&mut self) {
        let keys: Vec<String> = self.pending_snapshots.keys().cloned().collect();
        for asset_id in keys {
            self.materialize_pending_for_asset(&asset_id);
        }
    }

    fn materialize_pending_before_record(&mut self, record: &PersistedRecord) {
        match record {
            PersistedRecord::Book(event) if event.kind == BookEventKind::Snapshot => {
                let key = SnapshotGroupKey {
                    asset_id: event.asset_id.to_string(),
                    recv_timestamp_us: event.provenance.recv_timestamp_us,
                    exchange_timestamp_us: event.provenance.exchange_timestamp_us,
                    source_event_id: event.provenance.source_event_id.clone(),
                };
                let stale_keys: Vec<String> = self
                    .pending_snapshots
                    .iter()
                    .filter(|(_, pending)| pending.key != key)
                    .map(|(asset_id, _)| asset_id.clone())
                    .collect();
                for asset_id in stale_keys {
                    self.materialize_pending_for_asset(&asset_id);
                }
            }
            _ => self.materialize_all_pending(),
        }
    }

    fn materialize_pending_for_asset(&mut self, asset_id: &str) {
        let Some(pending) = self.pending_snapshots.remove(asset_id) else {
            return;
        };
        let state = self.ensure_asset(asset_id);
        state.book.apply_snapshot(
            &pending.bids,
            &pending.asks,
            pb_types::Sequence::new(pending.sequence),
            pending.last_recv_timestamp_us,
        );
        state.initialized_from_snapshot = true;
        state.last_recv_timestamp_us = Some(pending.last_recv_timestamp_us);
        state.last_exchange_timestamp_us = Some(pending.key.exchange_timestamp_us);
    }

    fn record_snapshot_event(&mut self, event: BookEvent) {
        let asset_id = event.asset_id.to_string();
        let key = SnapshotGroupKey {
            asset_id: asset_id.clone(),
            recv_timestamp_us: event.provenance.recv_timestamp_us,
            exchange_timestamp_us: event.provenance.exchange_timestamp_us,
            source_event_id: event.provenance.source_event_id.clone(),
        };
        let pending = self
            .pending_snapshots
            .entry(asset_id.clone())
            .or_insert_with(|| PendingSnapshot {
                key: key.clone(),
                bids: Vec::new(),
                asks: Vec::new(),
                sequence: 0,
                last_recv_timestamp_us: event.provenance.recv_timestamp_us,
            });
        if pending.key != key {
            self.materialize_pending_for_asset(&asset_id);
            self.record_snapshot_event(event);
            return;
        }

        match event.side {
            pb_types::Side::Bid => pending.bids.push((event.price, event.size)),
            pb_types::Side::Ask => pending.asks.push((event.price, event.size)),
        }
        pending.sequence = event.provenance.sequence.unwrap_or_default().raw();
        pending.last_recv_timestamp_us = event.provenance.recv_timestamp_us;

        let state = self.ensure_asset(&asset_id);
        state.last_recv_timestamp_us = Some(event.provenance.recv_timestamp_us);
        state.last_exchange_timestamp_us = Some(event.provenance.exchange_timestamp_us);
    }

    fn record_delta_event(&mut self, event: BookEvent) {
        let asset_id = event.asset_id.to_string();
        let state = self.ensure_asset(&asset_id);
        state.book.apply_delta(
            event.side,
            event.price,
            event.size,
            event.provenance.sequence.unwrap_or_default(),
            event.provenance.recv_timestamp_us,
        );
        state.last_recv_timestamp_us = Some(event.provenance.recv_timestamp_us);
        state.last_exchange_timestamp_us = Some(event.provenance.exchange_timestamp_us);
    }

    fn record_ingest_event(&mut self, event: IngestEvent) {
        let warning = ContinuityWarning {
            kind: event.kind.to_string(),
            recv_timestamp_us: event.provenance.recv_timestamp_us,
            exchange_timestamp_us: event.provenance.exchange_timestamp_us,
            details: event.details.clone(),
        };
        match event.kind {
            pb_types::IngestEventKind::ReconnectStart => {
                self.session_status = SessionStatus::Reconnecting;
            }
            pb_types::IngestEventKind::ReconnectSuccess => {
                self.session_status = SessionStatus::Connected;
                self.current_session_id = event.provenance.source_session_id.clone();
            }
            pb_types::IngestEventKind::SequenceGap
            | pb_types::IngestEventKind::StaleSnapshotSkip
            | pb_types::IngestEventKind::SourceReset => {}
        }
        if let Some(asset_id) = event.asset_id.as_ref() {
            let state = self.ensure_asset(asset_id.as_str());
            state.latest_warning = Some(warning);
        } else {
            self.latest_global_warning = Some(warning);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotLookupError {
    AssetNotActive,
    SnapshotNotReady,
}

#[derive(Clone)]
pub struct LiveReadModel {
    inner: Arc<RwLock<LiveState>>,
}

impl LiveReadModel {
    pub fn new(mode: FeedMode) -> Self {
        Self {
            inner: Arc::new(RwLock::new(LiveState::new(mode))),
        }
    }

    pub fn spawn_consumer(
        &self,
        mut rx: mpsc::Receiver<PersistedRecord>,
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let model = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        break;
                    }
                    record = rx.recv() => {
                        match record {
                            Some(record) => model.apply_record(record).await,
                            None => break,
                        }
                    }
                }
            }
        })
    }

    pub fn spawn_consumer_with_broadcast(
        &self,
        mut rx: mpsc::Receiver<PersistedRecord>,
        broadcast: crate::streaming::BookBroadcast,
        default_depth: usize,
        stale_after_secs: u64,
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let model = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        break;
                    }
                    record = rx.recv() => {
                        match record {
                            Some(record) => {
                                let is_book = matches!(&record, PersistedRecord::Book(_));
                                let asset_id = match &record {
                                    PersistedRecord::Book(e) => Some(e.asset_id.to_string()),
                                    _ => None,
                                };
                                model.apply_record(record).await;
                                if is_book && broadcast.has_subscribers() {
                                    if let Some(asset_id) = asset_id {
                                        if let Ok(snap) = model.snapshot(&asset_id, default_depth, stale_after_secs).await {
                                            broadcast.send(crate::dto::BookUpdateMessage {
                                                asset_id: snap.asset_id,
                                                sequence: snap.sequence,
                                                last_update_us: snap.last_update_us,
                                                bids: snap.bids,
                                                asks: snap.asks,
                                                mid_price: snap.mid_price,
                                                spread: snap.spread,
                                            });
                                        }
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        })
    }

    pub async fn apply_record(&self, record: PersistedRecord) {
        let mut state = self.inner.write().await;
        state.materialize_pending_before_record(&record);
        match record {
            PersistedRecord::Book(event) => match event.kind {
                BookEventKind::Snapshot => state.record_snapshot_event(event),
                BookEventKind::Delta => state.record_delta_event(event),
            },
            PersistedRecord::Ingest(event) => state.record_ingest_event(event),
            PersistedRecord::Trade(_)
            | PersistedRecord::Checkpoint(_)
            | PersistedRecord::Validation(_)
            | PersistedRecord::Execution(_) => {}
        }
    }

    pub async fn set_active_assets(&self, assets: Vec<String>) {
        let mut state = self.inner.write().await;
        state.set_active_assets(assets);
    }

    pub async fn set_last_rotation_us(&self, timestamp_us: u64) {
        let mut state = self.inner.write().await;
        state.last_rotation_us = Some(timestamp_us);
    }

    pub async fn feed_status(&self) -> FeedStatusResponse {
        let mut state = self.inner.write().await;
        state.materialize_all_pending();
        FeedStatusResponse {
            mode: state.mode,
            session_status: state.session_status,
            current_session_id: state.current_session_id.clone(),
            active_asset_count: state.active_assets.len(),
            active_assets: state.active_assets.clone(),
            last_rotation_us: state.last_rotation_us,
            latest_global_warning: state.latest_global_warning.clone(),
        }
    }

    pub async fn active_assets(&self, stale_after_secs: u64) -> Vec<ActiveAssetSummary> {
        let now_us = now_us();
        let mut state = self.inner.write().await;
        state.materialize_all_pending();
        state
            .active_assets
            .iter()
            .map(|asset_id| {
                let asset_state = state.assets.get(asset_id);
                let last_recv = asset_state.and_then(|state| state.last_recv_timestamp_us);
                ActiveAssetSummary {
                    asset_id: asset_id.clone(),
                    last_recv_timestamp_us: last_recv,
                    last_exchange_timestamp_us: asset_state
                        .and_then(|state| state.last_exchange_timestamp_us),
                    stale: is_stale(last_recv, stale_after_secs, now_us),
                    has_book: asset_state
                        .map(|state| state.initialized_from_snapshot)
                        .unwrap_or(false),
                }
            })
            .collect()
    }

    pub async fn is_asset_active(&self, asset_id: &str) -> bool {
        let state = self.inner.read().await;
        state
            .active_assets
            .iter()
            .any(|candidate| candidate == asset_id)
    }

    pub async fn snapshot(
        &self,
        asset_id: &str,
        depth: usize,
        stale_after_secs: u64,
    ) -> Result<LiveOrderBookSnapshot, SnapshotLookupError> {
        let now_us = now_us();
        let mut state = self.inner.write().await;
        state.materialize_all_pending();
        if !state
            .active_assets
            .iter()
            .any(|candidate| candidate == asset_id)
        {
            return Err(SnapshotLookupError::AssetNotActive);
        }
        let Some(asset_state) = state.assets.get(asset_id) else {
            return Err(SnapshotLookupError::SnapshotNotReady);
        };
        if !asset_state.initialized_from_snapshot {
            return Err(SnapshotLookupError::SnapshotNotReady);
        }

        Ok(LiveOrderBookSnapshot {
            asset_id: asset_id.to_string(),
            sequence: asset_state.book.sequence.raw(),
            last_update_us: asset_state.book.last_update_us,
            best_bid: level_pair(asset_state.book.best_bid()),
            best_ask: level_pair(asset_state.book.best_ask()),
            mid_price: asset_state.book.mid_price(),
            spread: asset_state.book.spread(),
            bid_depth: asset_state.book.bid_depth(),
            ask_depth: asset_state.book.ask_depth(),
            bids: asset_state
                .book
                .bids_sorted()
                .into_iter()
                .take(depth)
                .map(level_view)
                .collect(),
            asks: asset_state
                .book
                .asks_sorted()
                .into_iter()
                .take(depth)
                .map(level_view)
                .collect(),
            stale: is_stale(asset_state.last_recv_timestamp_us, stale_after_secs, now_us),
            latest_warning: asset_state.latest_warning.clone(),
        })
    }
}

fn level_view((price, size): (FixedPrice, FixedSize)) -> PriceLevelView {
    PriceLevelView { price, size }
}

fn level_pair(level: Option<(FixedPrice, FixedSize)>) -> Option<PriceLevelView> {
    level.map(level_view)
}

fn is_stale(last_recv_us: Option<u64>, stale_after_secs: u64, now_us: u64) -> bool {
    let Some(last_recv_us) = last_recv_us else {
        return true;
    };
    now_us.saturating_sub(last_recv_us) > stale_after_secs.saturating_mul(1_000_000)
}

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use pb_types::event::{DataSource, EventProvenance, IngestEventKind};
    use pb_types::{BookEventKind, IngestEvent, PersistedRecord, Sequence, Side};

    fn provenance(recv: u64, exchange: u64, sequence: u64) -> EventProvenance {
        EventProvenance {
            recv_timestamp_us: recv,
            exchange_timestamp_us: exchange,
            source: DataSource::WebSocket,
            source_event_id: Some("snapshot-a".to_string()),
            source_session_id: Some("ws-session-1".to_string()),
            sequence: Some(Sequence::new(sequence)),
        }
    }

    fn snapshot_record(side: Side, price: f64, size: f64, sequence: u64) -> PersistedRecord {
        PersistedRecord::Book(pb_types::BookEvent {
            asset_id: AssetId::new("tok1"),
            kind: BookEventKind::Snapshot,
            side,
            price: pb_types::FixedPrice::from_f64(price).unwrap(),
            size: pb_types::FixedSize::from_f64(size).unwrap(),
            provenance: provenance(100, 90, sequence),
        })
    }

    #[tokio::test]
    async fn snapshot_group_materializes_before_non_snapshot_record() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.set_active_assets(vec!["tok1".to_string()]).await;
        model
            .apply_record(snapshot_record(Side::Bid, 0.50, 10.0, 0))
            .await;
        model
            .apply_record(snapshot_record(Side::Ask, 0.60, 20.0, 1))
            .await;
        model
            .apply_record(PersistedRecord::Ingest(IngestEvent {
                asset_id: None,
                kind: IngestEventKind::ReconnectSuccess,
                provenance: EventProvenance {
                    recv_timestamp_us: 101,
                    exchange_timestamp_us: 0,
                    source: DataSource::WebSocket,
                    source_event_id: None,
                    source_session_id: Some("ws-session-1".to_string()),
                    sequence: None,
                },
                expected_sequence: None,
                observed_sequence: None,
                details: None,
            }))
            .await;

        let snapshot = model.snapshot("tok1", 5, 100).await.unwrap();
        assert_eq!(snapshot.bid_depth, 1);
        assert_eq!(snapshot.ask_depth, 1);
        assert_eq!(snapshot.sequence, 1);
    }

    #[tokio::test]
    async fn asset_warning_is_surfaceable() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.set_active_assets(vec!["tok1".to_string()]).await;
        model
            .apply_record(PersistedRecord::Ingest(IngestEvent {
                asset_id: Some(AssetId::new("tok1")),
                kind: IngestEventKind::SequenceGap,
                provenance: EventProvenance {
                    recv_timestamp_us: 200,
                    exchange_timestamp_us: 150,
                    source: DataSource::WebSocket,
                    source_event_id: None,
                    source_session_id: Some("ws-session-1".to_string()),
                    sequence: None,
                },
                expected_sequence: Some(2),
                observed_sequence: Some(5),
                details: Some("gap".to_string()),
            }))
            .await;

        let assets = model.active_assets(10).await;
        assert_eq!(assets[0].asset_id, "tok1");
        let snapshot_err = model.snapshot("tok1", 5, 10).await.unwrap_err();
        assert_eq!(snapshot_err, SnapshotLookupError::SnapshotNotReady);
    }
}
