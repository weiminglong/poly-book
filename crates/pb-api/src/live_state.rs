use std::collections::HashMap;
use std::sync::Arc;

use pb_book::L2Book;
use pb_types::event::{BookEvent, BookEventKind, IngestEvent, PersistedRecord};
use pb_types::{AssetId, FixedPrice, FixedSize};
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::dto::{
    ActiveAssetSummary, AssetRef, BookUpdateMessage, ContinuityWarning, FeedMode,
    FeedStatusResponse, LiveOrderBookSnapshot, PriceLevelView, SessionStatus,
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

    fn materialize_all_pending(&mut self) -> Vec<String> {
        let keys: Vec<String> = self.pending_snapshots.keys().cloned().collect();
        let mut materialized = Vec::new();
        for asset_id in keys {
            if self.materialize_pending_for_asset(&asset_id) {
                materialized.push(asset_id);
            }
        }
        materialized
    }

    fn materialize_pending_before_record(&mut self, record: &PersistedRecord) -> Vec<String> {
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
                let mut materialized = Vec::new();
                for asset_id in stale_keys {
                    if self.materialize_pending_for_asset(&asset_id) {
                        materialized.push(asset_id);
                    }
                }
                materialized
            }
            _ => self.materialize_all_pending(),
        }
    }

    fn materialize_pending_for_asset(&mut self, asset_id: &str) -> bool {
        let Some(pending) = self.pending_snapshots.remove(asset_id) else {
            return false;
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
        true
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
                                if broadcast.has_subscribers() {
                                    for update in model
                                        .apply_record_and_build_updates(record, default_depth)
                                        .await
                                    {
                                        broadcast.send(update);
                                    }
                                } else {
                                    model.apply_record(record).await;
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
        let _ = self.apply_record_and_build_updates(record, 0).await;
    }

    pub async fn apply_record_and_build_updates(
        &self,
        record: PersistedRecord,
        depth: usize,
    ) -> Vec<BookUpdateMessage> {
        let mut state = self.inner.write().await;
        let materialized_assets = state.materialize_pending_before_record(&record);
        let current_book_asset = match record {
            PersistedRecord::Book(event) => {
                let asset_id = event.asset_id.to_string();
                match event.kind {
                    BookEventKind::Snapshot => {
                        state.record_snapshot_event(event);
                        None
                    }
                    BookEventKind::Delta => {
                        state.record_delta_event(event);
                        Some(asset_id)
                    }
                }
            }
            PersistedRecord::Ingest(event) => {
                state.record_ingest_event(event);
                None
            }
            PersistedRecord::Trade(_)
            | PersistedRecord::Checkpoint(_)
            | PersistedRecord::Validation(_)
            | PersistedRecord::Execution(_) => None,
        };

        if depth == 0 {
            return Vec::new();
        }

        let mut updates = Vec::new();
        for asset_id in materialized_assets {
            if let Some(update) = state.materialized_book_update_message(&asset_id, depth) {
                updates.push(update);
            }
        }
        if let Some(asset_id) = current_book_asset {
            if let Some(update) = state.materialized_book_update_message(&asset_id, depth) {
                updates.push(update);
            }
        }
        updates
    }

    pub async fn set_active_assets(&self, assets: Vec<String>) {
        let mut state = self.inner.write().await;
        state.set_active_assets(assets);
    }

    pub async fn set_last_rotation_us(&self, timestamp_us: u64) {
        let mut state = self.inner.write().await;
        state.last_rotation_us = Some(timestamp_us);
    }

    pub async fn feed_status_raw(&self) -> FeedStatusResponse {
        let state = self.inner.read().await;
        FeedStatusResponse {
            mode: state.mode,
            session_status: state.session_status,
            current_session_id: state.current_session_id.clone(),
            active_asset_count: state.active_assets.len(),
            active_assets: state
                .active_assets
                .iter()
                .map(|id| AssetRef {
                    asset_id: id.clone(),
                    slug: None,
                })
                .collect(),
            last_rotation_us: state.last_rotation_us,
            latest_global_warning: state.latest_global_warning.clone(),
        }
    }

    pub async fn active_assets(&self, stale_after_secs: u64) -> Vec<ActiveAssetSummary> {
        let now_us = now_us();
        let state = self.inner.read().await;
        state
            .active_assets
            .iter()
            .map(|asset_id| {
                let asset_state = state.assets.get(asset_id);
                let last_recv = asset_state.and_then(|state| state.last_recv_timestamp_us);
                ActiveAssetSummary {
                    asset_id: asset_id.clone(),
                    slug: None,
                    label: None,
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
        let state = self.inner.read().await;
        state.snapshot(asset_id, depth, stale_after_secs, now_us)
    }
}

impl LiveState {
    fn snapshot(
        &self,
        asset_id: &str,
        depth: usize,
        stale_after_secs: u64,
        now_us: u64,
    ) -> Result<LiveOrderBookSnapshot, SnapshotLookupError> {
        if !self
            .active_assets
            .iter()
            .any(|candidate| candidate == asset_id)
        {
            return Err(SnapshotLookupError::AssetNotActive);
        }

        let asset_state = self.assets.get(asset_id);
        if self.pending_snapshots.contains_key(asset_id) {
            if let Some(asset_state) = asset_state.filter(|state| state.initialized_from_snapshot) {
                return Ok(build_live_snapshot(
                    asset_id,
                    asset_state,
                    depth,
                    stale_after_secs,
                    now_us,
                ));
            }
            return Err(SnapshotLookupError::SnapshotNotReady);
        }

        let Some(asset_state) = asset_state else {
            return Err(SnapshotLookupError::SnapshotNotReady);
        };
        if !asset_state.initialized_from_snapshot {
            return Err(SnapshotLookupError::SnapshotNotReady);
        }

        Ok(build_live_snapshot(
            asset_id,
            asset_state,
            depth,
            stale_after_secs,
            now_us,
        ))
    }

    fn materialized_book_update_message(
        &self,
        asset_id: &str,
        depth: usize,
    ) -> Option<BookUpdateMessage> {
        if depth == 0
            || !self
                .active_assets
                .iter()
                .any(|candidate| candidate == asset_id)
        {
            return None;
        }

        let asset_state = self.assets.get(asset_id)?;
        if !asset_state.initialized_from_snapshot {
            return None;
        }

        Some(build_book_update(asset_id, asset_state, depth))
    }
}

fn build_live_snapshot(
    asset_id: &str,
    asset_state: &AssetState,
    depth: usize,
    stale_after_secs: u64,
    now_us: u64,
) -> LiveOrderBookSnapshot {
    LiveOrderBookSnapshot {
        asset_id: asset_id.to_string(),
        slug: None,
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
            .top_bids(depth)
            .into_iter()
            .map(level_view)
            .collect(),
        asks: asset_state
            .book
            .top_asks(depth)
            .into_iter()
            .map(level_view)
            .collect(),
        stale: is_stale(asset_state.last_recv_timestamp_us, stale_after_secs, now_us),
        latest_warning: asset_state.latest_warning.clone(),
    }
}

fn build_book_update(asset_id: &str, asset_state: &AssetState, depth: usize) -> BookUpdateMessage {
    BookUpdateMessage {
        asset_id: asset_id.to_string(),
        slug: None,
        sequence: asset_state.book.sequence.raw(),
        last_update_us: asset_state.book.last_update_us,
        bids: asset_state
            .book
            .top_bids(depth)
            .into_iter()
            .map(level_view)
            .collect(),
        asks: asset_state
            .book
            .top_asks(depth)
            .into_iter()
            .map(level_view)
            .collect(),
        mid_price: asset_state.book.mid_price(),
        spread: asset_state.book.spread(),
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
    async fn snapshot_stays_not_ready_until_group_materializes() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.set_active_assets(vec!["tok1".to_string()]).await;

        model
            .apply_record(snapshot_record(Side::Bid, 0.50, 10.0, 0))
            .await;
        let first = model.snapshot("tok1", 5, 100).await.unwrap_err();
        assert_eq!(first, SnapshotLookupError::SnapshotNotReady);

        model
            .apply_record(snapshot_record(Side::Ask, 0.60, 20.0, 1))
            .await;
        let second = model.snapshot("tok1", 5, 100).await.unwrap_err();
        assert_eq!(second, SnapshotLookupError::SnapshotNotReady);

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

        let third = model.snapshot("tok1", 5, 100).await.unwrap();
        assert_eq!(third.bid_depth, 1);
        assert_eq!(third.ask_depth, 1);
        assert_eq!(third.sequence, 1);
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
