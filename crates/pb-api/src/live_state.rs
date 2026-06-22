use std::collections::HashMap;
use std::sync::Arc;

use pb_book::L2Book;
use pb_types::event::{BookCheckpoint, BookEvent, BookEventKind, IngestEvent, PersistedRecord};
use pb_types::{AssetId, FixedPrice, FixedSize};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::dto::{
    ActiveAssetSummary, AssetRef, BookUpdateMessage, ContinuityWarning, FeedMode,
    FeedStatusResponse, LiveOrderBookSnapshot, PriceLevelView, SessionStatus,
};

// ---------------------------------------------------------------------------
// Published read-only state (shared via watch channel, zero-contention reads)
// ---------------------------------------------------------------------------

/// Read-only projection of the live state, published by the projector task.
/// Readers access this via `watch::Receiver::borrow()` without any locking.
#[derive(Debug, Clone)]
pub(crate) struct PublishedState {
    pub(crate) mode: FeedMode,
    pub(crate) session_status: SessionStatus,
    pub(crate) current_session_id: Option<String>,
    pub(crate) active_assets: Vec<String>,
    pub(crate) last_rotation_us: Option<u64>,
    pub(crate) latest_global_warning: Option<ContinuityWarning>,
    pub(crate) assets: HashMap<String, Arc<AssetReadView>>,
    /// Whether checkpoint hydration has completed (or was skipped).
    pub(crate) hydrated: bool,
}

/// Read-only projection of one asset's book state.
#[derive(Debug, Clone)]
pub(crate) struct AssetReadView {
    pub(crate) sequence: u64,
    pub(crate) last_update_us: u64,
    pub(crate) best_bid: Option<PriceLevelView>,
    pub(crate) best_ask: Option<PriceLevelView>,
    pub(crate) mid_price: Option<f64>,
    pub(crate) spread: Option<f64>,
    pub(crate) bid_depth: usize,
    pub(crate) ask_depth: usize,
    pub(crate) bids: Vec<PriceLevelView>,
    pub(crate) asks: Vec<PriceLevelView>,
    pub(crate) initialized_from_snapshot: bool,
    pub(crate) has_pending_snapshot: bool,
    pub(crate) last_recv_timestamp_us: Option<u64>,
    pub(crate) last_exchange_timestamp_us: Option<u64>,
    pub(crate) latest_warning: Option<ContinuityWarning>,
}

// ---------------------------------------------------------------------------
// Internal mutable state (owned exclusively by the projector task)
// ---------------------------------------------------------------------------

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
    hydrated: bool,
}

#[derive(Debug, Default)]
struct ApplyOutcome {
    changed_assets: Vec<String>,
    broadcast_assets: Vec<String>,
    should_publish: bool,
}

impl ApplyOutcome {
    fn mark_asset_changed(&mut self, asset_id: String) {
        push_unique_asset(&mut self.changed_assets, asset_id);
        self.should_publish = true;
    }

    fn mark_asset_broadcast(&mut self, asset_id: String) {
        push_unique_asset(&mut self.broadcast_assets, asset_id);
        self.should_publish = true;
    }
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
            hydrated: false,
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
        self.check_book_integrity(asset_id);
        true
    }

    /// Run the crossed/locked-book invariant check on an asset's current book
    /// and surface a violation (metric + warning) instead of silently serving a
    /// crossed book.
    fn check_book_integrity(&mut self, asset_id: &str) {
        let state = self.ensure_asset(asset_id);
        if let Err(err) = state.book.check_integrity() {
            let detail = err.to_string();
            state.latest_warning = Some(ContinuityWarning {
                kind: "crossed_book".to_string(),
                recv_timestamp_us: state.last_recv_timestamp_us.unwrap_or(0),
                exchange_timestamp_us: state.last_exchange_timestamp_us.unwrap_or(0),
                details: Some(detail.clone()),
            });
            pb_metrics::record_crossed_book();
            tracing::warn!(asset_id, error = %detail, "crossed/locked book detected on live path");
        }
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
        self.check_book_integrity(&asset_id);
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
            | pb_types::IngestEventKind::SourceReset
            | pb_types::IngestEventKind::BookMismatch => {}
        }
        if let Some(asset_id) = event.asset_id.as_ref() {
            let state = self.ensure_asset(asset_id.as_str());
            state.latest_warning = Some(warning);
        } else {
            self.latest_global_warning = Some(warning);
        }
    }

    /// Apply a checkpoint directly to restore book state during hydration.
    fn apply_checkpoint(&mut self, checkpoint: &BookCheckpoint) {
        let asset_id = checkpoint.asset_id.as_str();
        let state = self.ensure_asset(asset_id);
        let bids: Vec<(FixedPrice, FixedSize)> =
            checkpoint.bids.iter().map(|l| (l.price, l.size)).collect();
        let asks: Vec<(FixedPrice, FixedSize)> =
            checkpoint.asks.iter().map(|l| (l.price, l.size)).collect();
        state.book.apply_snapshot(
            &bids,
            &asks,
            pb_types::Sequence::default(),
            checkpoint.checkpoint_timestamp_us,
        );
        state.initialized_from_snapshot = true;
        state.last_recv_timestamp_us = Some(checkpoint.provenance.recv_timestamp_us);
        state.last_exchange_timestamp_us = Some(checkpoint.provenance.exchange_timestamp_us);
        let asset_id = asset_id.to_string();
        self.check_book_integrity(&asset_id);
    }

    /// Apply a record and return the list of asset IDs that had book updates
    /// (materialized snapshots or deltas).
    fn apply_record(&mut self, record: PersistedRecord) -> ApplyOutcome {
        let materialized_assets = self.materialize_pending_before_record(&record);
        let mut outcome = ApplyOutcome::default();

        for asset_id in materialized_assets {
            outcome.mark_asset_changed(asset_id.clone());
            outcome.mark_asset_broadcast(asset_id);
        }

        match record {
            PersistedRecord::Book(event) => {
                let asset_id = event.asset_id.to_string();
                match event.kind {
                    BookEventKind::Snapshot => {
                        self.record_snapshot_event(event);
                        outcome.mark_asset_changed(asset_id);
                    }
                    BookEventKind::Delta => {
                        self.record_delta_event(event);
                        outcome.mark_asset_changed(asset_id.clone());
                        outcome.mark_asset_broadcast(asset_id);
                    }
                }
            }
            PersistedRecord::Ingest(event) => {
                let asset_id = event.asset_id.as_ref().map(ToString::to_string);
                self.record_ingest_event(event);
                outcome.should_publish = true;
                if let Some(asset_id) = asset_id {
                    outcome.mark_asset_changed(asset_id);
                }
            }
            PersistedRecord::Trade(_)
            | PersistedRecord::Checkpoint(_)
            | PersistedRecord::Validation(_)
            | PersistedRecord::Execution(_) => {}
        }

        outcome
    }

    /// Build the published read-only projection of current state.
    fn build_published_state(&self) -> PublishedState {
        let assets = self
            .assets
            .iter()
            .filter_map(|(asset_id, _)| {
                self.build_asset_view(asset_id)
                    .map(|view| (asset_id.clone(), view))
            })
            .collect();

        PublishedState {
            mode: self.mode,
            session_status: self.session_status,
            current_session_id: self.current_session_id.clone(),
            active_assets: self.active_assets.clone(),
            last_rotation_us: self.last_rotation_us,
            latest_global_warning: self.latest_global_warning.clone(),
            assets,
            hydrated: self.hydrated,
        }
    }

    fn build_asset_view(&self, asset_id: &str) -> Option<Arc<AssetReadView>> {
        let state = self.assets.get(asset_id)?;
        let has_pending = self.pending_snapshots.contains_key(asset_id);
        Some(Arc::new(AssetReadView {
            sequence: state.book.sequence.raw(),
            last_update_us: state.book.last_update_us,
            best_bid: state.book.best_bid().map(level_view),
            best_ask: state.book.best_ask().map(level_view),
            mid_price: state.book.mid_price(),
            spread: state.book.spread(),
            bid_depth: state.book.bid_depth(),
            ask_depth: state.book.ask_depth(),
            bids: state
                .book
                .top_bids(state.book.bid_depth())
                .into_iter()
                .map(level_view)
                .collect(),
            asks: state
                .book
                .top_asks(state.book.ask_depth())
                .into_iter()
                .map(level_view)
                .collect(),
            initialized_from_snapshot: state.initialized_from_snapshot,
            has_pending_snapshot: has_pending,
            last_recv_timestamp_us: state.last_recv_timestamp_us,
            last_exchange_timestamp_us: state.last_exchange_timestamp_us,
            latest_warning: state.latest_warning.clone(),
        }))
    }

    /// Build a broadcast update message for a given asset.
    fn build_book_update(&self, asset_id: &str, depth: usize) -> Option<BookUpdateMessage> {
        if !self
            .active_assets
            .iter()
            .any(|candidate| candidate == asset_id)
        {
            return None;
        }
        let state = self.assets.get(asset_id)?;
        if !state.initialized_from_snapshot {
            return None;
        }
        Some(BookUpdateMessage {
            asset_id: asset_id.to_string(),
            slug: None,
            sequence: state.book.sequence.raw(),
            last_update_us: state.book.last_update_us,
            // True totals, not the depth-capped array lengths.
            bid_depth: state.book.bid_depth(),
            ask_depth: state.book.ask_depth(),
            bids: state
                .book
                .top_bids(depth)
                .into_iter()
                .map(level_view)
                .collect(),
            asks: state
                .book
                .top_asks(depth)
                .into_iter()
                .map(level_view)
                .collect(),
            mid_price: state.book.mid_price(),
            spread: state.book.spread(),
        })
    }
}

// ---------------------------------------------------------------------------
// Command channel (mutations sent to the projector task)
// ---------------------------------------------------------------------------

enum ProjectorCommand {
    /// Apply a record (fire-and-forget, used by the consumer forwarder).
    Record(PersistedRecord),
    /// Apply a record and ack when done (used by tests / sync callers).
    RecordAck(PersistedRecord, oneshot::Sender<()>),
    /// Set active assets and ack when done.
    SetActiveAssets(Vec<String>, oneshot::Sender<()>),
    /// Set last rotation timestamp and ack when done.
    SetLastRotationUs(u64, oneshot::Sender<()>),
    /// Configure broadcast for WS streaming.
    ConfigureBroadcast(crate::streaming::PerAssetBroadcast, usize),
    /// Apply a checkpoint directly (used during hydration).
    HydrateCheckpoint(BookCheckpoint, oneshot::Sender<()>),
    /// Mark the read model as hydrated (ready to serve).
    MarkHydrated(oneshot::Sender<()>),
}

// ---------------------------------------------------------------------------
// Projector task (single writer, owns all mutable state)
// ---------------------------------------------------------------------------

struct Projector {
    state: LiveState,
    published: PublishedState,
    watch_tx: watch::Sender<Arc<PublishedState>>,
    broadcast: Option<(crate::streaming::PerAssetBroadcast, usize)>,
}

impl Projector {
    fn new(state: LiveState, watch_tx: watch::Sender<Arc<PublishedState>>) -> Self {
        Self {
            published: state.build_published_state(),
            state,
            watch_tx,
            broadcast: None,
        }
    }

    fn handle_command(&mut self, cmd: ProjectorCommand) {
        match cmd {
            ProjectorCommand::Record(record) => {
                self.apply_and_broadcast(record);
            }
            ProjectorCommand::RecordAck(record, ack) => {
                self.apply_and_broadcast(record);
                let _ = ack.send(());
            }
            ProjectorCommand::SetActiveAssets(assets, ack) => {
                self.state.set_active_assets(assets);
                self.published.active_assets = self.state.active_assets.clone();
                self.rebuild_all_asset_views();
                self.publish();
                let _ = ack.send(());
            }
            ProjectorCommand::SetLastRotationUs(ts, ack) => {
                self.state.last_rotation_us = Some(ts);
                self.published.last_rotation_us = Some(ts);
                self.publish();
                let _ = ack.send(());
            }
            ProjectorCommand::ConfigureBroadcast(broadcast, depth) => {
                self.broadcast = Some((broadcast, depth));
            }
            ProjectorCommand::HydrateCheckpoint(checkpoint, ack) => {
                let asset_id = checkpoint.asset_id.to_string();
                self.state.apply_checkpoint(&checkpoint);
                self.refresh_asset_views(std::slice::from_ref(&asset_id));
                self.publish();
                let _ = ack.send(());
            }
            ProjectorCommand::MarkHydrated(ack) => {
                self.state.hydrated = true;
                self.published.hydrated = true;
                self.publish();
                let _ = ack.send(());
            }
        }
    }

    fn apply_and_broadcast(&mut self, record: PersistedRecord) {
        let outcome = self.state.apply_record(record);
        if !outcome.should_publish {
            return;
        }

        self.sync_metadata();
        self.refresh_asset_views(&outcome.changed_assets);
        self.publish();

        // Send broadcast updates for assets that changed.
        if let Some((ref broadcast, depth)) = self.broadcast {
            if broadcast.has_subscribers() {
                for asset_id in &outcome.broadcast_assets {
                    if let Some(update) = self.state.build_book_update(asset_id, depth) {
                        broadcast.send(update);
                    }
                }
            }
        }
    }

    fn sync_metadata(&mut self) {
        self.published.session_status = self.state.session_status;
        self.published.current_session_id = self.state.current_session_id.clone();
        self.published.last_rotation_us = self.state.last_rotation_us;
        self.published.latest_global_warning = self.state.latest_global_warning.clone();
        self.published.hydrated = self.state.hydrated;
    }

    fn rebuild_all_asset_views(&mut self) {
        self.published.assets = self
            .state
            .assets
            .keys()
            .filter_map(|asset_id| {
                self.state
                    .build_asset_view(asset_id)
                    .map(|view| (asset_id.clone(), view))
            })
            .collect();
    }

    fn refresh_asset_views(&mut self, asset_ids: &[String]) {
        for asset_id in asset_ids {
            if let Some(view) = self.state.build_asset_view(asset_id) {
                self.published.assets.insert(asset_id.clone(), view);
            } else {
                self.published.assets.remove(asset_id);
            }
        }
    }

    fn publish(&self) {
        let snapshot = Arc::new(self.published.clone());
        let _ = self.watch_tx.send(snapshot);
    }

    async fn run(mut self, mut cmd_rx: mpsc::Receiver<ProjectorCommand>, token: CancellationToken) {
        loop {
            tokio::select! {
                _ = token.cancelled() => break,
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(cmd) => self.handle_command(cmd),
                        None => break,
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API (LiveReadModel)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotLookupError {
    AssetNotActive,
    SnapshotNotReady,
}

/// Live read model backed by a single-writer projector task.
///
/// All mutations go through the projector via a command channel.
/// All reads use `watch::Receiver::borrow()` — zero contention with the writer.
#[derive(Clone)]
pub struct LiveReadModel {
    cmd_tx: mpsc::Sender<ProjectorCommand>,
    state_rx: watch::Receiver<Arc<PublishedState>>,
}

impl LiveReadModel {
    /// Create a new live read model and spawn the internal projector task.
    ///
    /// The projector task runs until the cancellation token is triggered
    /// or all command senders are dropped.
    pub fn new(mode: FeedMode) -> Self {
        let state = LiveState::new(mode);
        let initial = Arc::new(state.build_published_state());
        let (watch_tx, watch_rx) = watch::channel(initial);
        let (cmd_tx, cmd_rx) = mpsc::channel::<ProjectorCommand>(4_096);

        let projector = Projector::new(state, watch_tx);
        // Spawn with a long-lived token; the projector stops when cmd_rx closes.
        let token = CancellationToken::new();
        tokio::spawn(projector.run(cmd_rx, token));

        Self {
            cmd_tx,
            state_rx: watch_rx,
        }
    }

    /// Spawn a forwarder task that reads records from `rx` and sends them
    /// to the projector. Returns the forwarder's join handle.
    pub fn spawn_consumer(
        &self,
        mut rx: mpsc::Receiver<PersistedRecord>,
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let cmd_tx = self.cmd_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        // Drain records already buffered in the channel before
                        // exiting, so a shutdown does not abandon in-flight feed
                        // updates the producers had already sent (mirrors the storage-sink drain pattern). Bounded by the
                        // current buffer — no new records arrive after cancellation.
                        while let Ok(record) = rx.try_recv() {
                            if cmd_tx.send(ProjectorCommand::Record(record)).await.is_err() {
                                break;
                            }
                        }
                        break;
                    }
                    record = rx.recv() => {
                        match record {
                            Some(record) => {
                                if cmd_tx.send(ProjectorCommand::Record(record)).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        })
    }

    /// Spawn a forwarder task that reads records from `rx`, sends them to
    /// the projector (which handles broadcast internally), and returns the
    /// forwarder's join handle.
    pub fn spawn_consumer_with_broadcast(
        &self,
        mut rx: mpsc::Receiver<PersistedRecord>,
        broadcast: crate::streaming::PerAssetBroadcast,
        default_depth: usize,
        token: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let cmd_tx = self.cmd_tx.clone();
        tokio::spawn(async move {
            // Configure the projector's broadcast as the first command.
            // Records will queue behind this, ensuring broadcast is set before
            // any record processing.
            if cmd_tx
                .send(ProjectorCommand::ConfigureBroadcast(
                    broadcast,
                    default_depth,
                ))
                .await
                .is_err()
            {
                return;
            }

            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        // Drain records already buffered in the channel before
                        // exiting, so a shutdown does not abandon in-flight feed
                        // updates the producers had already sent (mirrors the storage-sink drain pattern). Bounded by the
                        // current buffer — no new records arrive after cancellation.
                        while let Ok(record) = rx.try_recv() {
                            if cmd_tx.send(ProjectorCommand::Record(record)).await.is_err() {
                                break;
                            }
                        }
                        break;
                    }
                    record = rx.recv() => {
                        match record {
                            Some(record) => {
                                if cmd_tx.send(ProjectorCommand::Record(record)).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        })
    }

    /// Configure projector-side broadcast fanout for direct `apply_record()`
    /// callers, such as the separated `serve` runtime's WAL tailer.
    pub async fn configure_broadcast(
        &self,
        broadcast: crate::streaming::PerAssetBroadcast,
        default_depth: usize,
    ) {
        let _ = self
            .cmd_tx
            .send(ProjectorCommand::ConfigureBroadcast(
                broadcast,
                default_depth,
            ))
            .await;
    }

    /// Apply a single record and wait for it to be processed.
    /// Apply a record through the projector. Returns `false` if the projector
    /// task is dead (the command channel is closed or the ack was dropped), so
    /// the WAL tailer can stop committing consumer positions for records that
    /// were never applied instead of silently advancing.
    pub async fn apply_record(&self, record: PersistedRecord) -> bool {
        let (ack_tx, ack_rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(ProjectorCommand::RecordAck(record, ack_tx))
            .await
            .is_err()
        {
            return false;
        }
        ack_rx.await.is_ok()
    }

    /// Set the active asset list. Waits for the projector to process the change.
    pub async fn set_active_assets(&self, assets: Vec<String>) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self
            .cmd_tx
            .send(ProjectorCommand::SetActiveAssets(assets, ack_tx))
            .await;
        let _ = ack_rx.await;
    }

    /// Set the last rotation timestamp.
    pub async fn set_last_rotation_us(&self, timestamp_us: u64) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self
            .cmd_tx
            .send(ProjectorCommand::SetLastRotationUs(timestamp_us, ack_tx))
            .await;
        let _ = ack_rx.await;
    }

    /// Apply a checkpoint directly to restore book state during hydration.
    pub async fn hydrate_checkpoint(&self, checkpoint: BookCheckpoint) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self
            .cmd_tx
            .send(ProjectorCommand::HydrateCheckpoint(checkpoint, ack_tx))
            .await;
        let _ = ack_rx.await;
    }

    /// Mark the read model as hydrated (ready to serve).
    pub async fn mark_hydrated(&self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self
            .cmd_tx
            .send(ProjectorCommand::MarkHydrated(ack_tx))
            .await;
        let _ = ack_rx.await;
    }

    /// Check whether the read model has completed hydration.
    pub fn is_hydrated(&self) -> bool {
        self.state_rx.borrow().hydrated
    }

    /// Read feed status from the latest published state. Zero contention.
    pub async fn feed_status_raw(&self) -> FeedStatusResponse {
        let state = self.state_rx.borrow().clone();
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

    /// Read active asset summaries. Zero contention.
    pub async fn active_assets(&self, stale_after_secs: u64) -> Vec<ActiveAssetSummary> {
        let now_us = now_us();
        let state = self.state_rx.borrow().clone();
        state
            .active_assets
            .iter()
            .map(|asset_id| {
                let asset = state.assets.get(asset_id);
                let last_recv = asset.and_then(|a| a.last_recv_timestamp_us);
                ActiveAssetSummary {
                    asset_id: asset_id.clone(),
                    slug: None,
                    label: None,
                    last_recv_timestamp_us: last_recv,
                    last_exchange_timestamp_us: asset.and_then(|a| a.last_exchange_timestamp_us),
                    stale: is_stale(last_recv, stale_after_secs, now_us),
                    has_book: asset.map(|a| a.initialized_from_snapshot).unwrap_or(false),
                }
            })
            .collect()
    }

    /// Check if an asset is active. Zero contention.
    pub async fn is_asset_active(&self, asset_id: &str) -> bool {
        let state = self.state_rx.borrow().clone();
        state
            .active_assets
            .iter()
            .any(|candidate| candidate == asset_id)
    }

    /// Read a book snapshot for a specific asset. Zero contention.
    pub async fn snapshot(
        &self,
        asset_id: &str,
        depth: usize,
        stale_after_secs: u64,
    ) -> Result<LiveOrderBookSnapshot, SnapshotLookupError> {
        let now_us = now_us();
        let state = self.state_rx.borrow().clone();
        snapshot_from_published(&state, asset_id, depth, stale_after_secs, now_us)
    }
}

fn snapshot_from_published(
    state: &PublishedState,
    asset_id: &str,
    depth: usize,
    stale_after_secs: u64,
    now_us: u64,
) -> Result<LiveOrderBookSnapshot, SnapshotLookupError> {
    if !state
        .active_assets
        .iter()
        .any(|candidate| candidate == asset_id)
    {
        return Err(SnapshotLookupError::AssetNotActive);
    }

    let Some(asset) = state.assets.get(asset_id) else {
        return Err(SnapshotLookupError::SnapshotNotReady);
    };

    // If a pending snapshot exists and the asset hasn't been initialized yet,
    // report not ready.
    if asset.has_pending_snapshot && !asset.initialized_from_snapshot {
        return Err(SnapshotLookupError::SnapshotNotReady);
    }

    if !asset.initialized_from_snapshot {
        return Err(SnapshotLookupError::SnapshotNotReady);
    }

    Ok(LiveOrderBookSnapshot {
        asset_id: asset_id.to_string(),
        slug: None,
        sequence: asset.sequence,
        last_update_us: asset.last_update_us,
        best_bid: asset.best_bid.clone(),
        best_ask: asset.best_ask.clone(),
        mid_price: asset.mid_price,
        spread: asset.spread,
        bid_depth: asset.bid_depth,
        ask_depth: asset.ask_depth,
        bids: asset.bids.iter().take(depth).cloned().collect(),
        asks: asset.asks.iter().take(depth).cloned().collect(),
        stale: is_stale(asset.last_recv_timestamp_us, stale_after_secs, now_us),
        latest_warning: asset.latest_warning.clone(),
    })
}

fn level_view((price, size): (FixedPrice, FixedSize)) -> PriceLevelView {
    PriceLevelView { price, size }
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

fn push_unique_asset(assets: &mut Vec<String>, asset_id: String) {
    if assets.iter().all(|candidate| candidate != &asset_id) {
        assets.push(asset_id);
    }
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
            ingest_ordinal: None,
        }
    }

    fn snapshot_record(side: Side, price: f64, size: f64, sequence: u64) -> PersistedRecord {
        snapshot_record_for("tok1", side, price, size, sequence)
    }

    fn snapshot_record_for(
        asset_id: &str,
        side: Side,
        price: f64,
        size: f64,
        sequence: u64,
    ) -> PersistedRecord {
        PersistedRecord::Book(pb_types::BookEvent {
            asset_id: AssetId::new(asset_id),
            kind: BookEventKind::Snapshot,
            side,
            price: pb_types::FixedPrice::from_f64(price).unwrap(),
            size: pb_types::FixedSize::from_f64(size).unwrap(),
            provenance: provenance(100, 90, sequence),
        })
    }

    fn delta_record_for(
        asset_id: &str,
        side: Side,
        price: f64,
        size: f64,
        recv_timestamp_us: u64,
        exchange_timestamp_us: u64,
        sequence: u64,
    ) -> PersistedRecord {
        PersistedRecord::Book(pb_types::BookEvent {
            asset_id: AssetId::new(asset_id),
            kind: BookEventKind::Delta,
            side,
            price: pb_types::FixedPrice::from_f64(price).unwrap(),
            size: pb_types::FixedSize::from_f64(size).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us,
                exchange_timestamp_us,
                source: DataSource::WebSocket,
                source_event_id: None,
                source_session_id: None,
                sequence: Some(Sequence::new(sequence)),
                ingest_ordinal: None,
            },
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
                    ingest_ordinal: None,
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
    async fn crossed_book_on_delta_is_detected_and_surfaced() {
        // A delta that lifts the best bid above the best ask produces a crossed
        // book, which must be flagged rather than served silently.
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.set_active_assets(vec!["tok1".to_string()]).await;
        model
            .apply_record(snapshot_record(Side::Bid, 0.50, 10.0, 0))
            .await;
        model
            .apply_record(snapshot_record(Side::Ask, 0.60, 20.0, 1))
            .await;
        // The delta also materializes the pending (valid) snapshot first, then
        // crosses the book.
        model
            .apply_record(delta_record_for("tok1", Side::Bid, 0.65, 5.0, 200, 190, 2))
            .await;

        let snapshot = model.snapshot("tok1", 5, 100).await.unwrap();
        let warning = snapshot
            .latest_warning
            .expect("crossed book should surface a warning");
        assert_eq!(warning.kind, "crossed_book");
    }

    #[tokio::test]
    async fn crossed_book_at_snapshot_materialization_is_detected() {
        // A snapshot GROUP that is itself crossed (best bid >= best ask) must be
        // flagged when it materializes. This is the snapshot path
        // (materialize_pending_for_asset -> check_book_integrity), distinct from
        // the delta path covered above.
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.set_active_assets(vec!["tok1".to_string()]).await;
        // Crossed snapshot: bid 0.60 sits above ask 0.50.
        model
            .apply_record(snapshot_record(Side::Bid, 0.60, 10.0, 0))
            .await;
        model
            .apply_record(snapshot_record(Side::Ask, 0.50, 20.0, 1))
            .await;
        // A non-snapshot record forces the pending snapshot group to materialize.
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
                    ingest_ordinal: None,
                },
                expected_sequence: None,
                observed_sequence: None,
                details: None,
            }))
            .await;

        let snapshot = model.snapshot("tok1", 5, 100).await.unwrap();
        let warning = snapshot
            .latest_warning
            .expect("crossed snapshot should surface a warning at materialization");
        assert_eq!(warning.kind, "crossed_book");
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
                    ingest_ordinal: None,
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
                    ingest_ordinal: None,
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

    #[tokio::test]
    async fn feed_status_reflects_session_state() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        let status = model.feed_status_raw().await;
        assert_eq!(status.session_status, SessionStatus::Starting);
        assert_eq!(status.active_asset_count, 0);
        assert!(status.current_session_id.is_none());
        assert!(status.last_rotation_us.is_none());
    }

    #[tokio::test]
    async fn feed_status_after_reconnect_success() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model
            .apply_record(PersistedRecord::Ingest(IngestEvent {
                asset_id: None,
                kind: IngestEventKind::ReconnectSuccess,
                provenance: EventProvenance {
                    recv_timestamp_us: 200,
                    exchange_timestamp_us: 0,
                    source: DataSource::WebSocket,
                    source_event_id: None,
                    source_session_id: Some("session-42".to_string()),
                    sequence: None,
                    ingest_ordinal: None,
                },
                expected_sequence: None,
                observed_sequence: None,
                details: None,
            }))
            .await;

        let status = model.feed_status_raw().await;
        assert_eq!(status.session_status, SessionStatus::Connected);
        assert_eq!(status.current_session_id.as_deref(), Some("session-42"));
    }

    #[tokio::test]
    async fn feed_status_during_reconnect() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model
            .apply_record(PersistedRecord::Ingest(IngestEvent {
                asset_id: None,
                kind: IngestEventKind::ReconnectStart,
                provenance: EventProvenance {
                    recv_timestamp_us: 200,
                    exchange_timestamp_us: 0,
                    source: DataSource::WebSocket,
                    source_event_id: None,
                    source_session_id: None,
                    sequence: None,
                    ingest_ordinal: None,
                },
                expected_sequence: None,
                observed_sequence: None,
                details: None,
            }))
            .await;

        let status = model.feed_status_raw().await;
        assert_eq!(status.session_status, SessionStatus::Reconnecting);
    }

    #[tokio::test]
    async fn set_active_assets_prunes_old_books() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model
            .set_active_assets(vec!["tok1".to_string(), "tok2".to_string()])
            .await;

        // Apply snapshot for tok1
        model
            .apply_record(snapshot_record(Side::Bid, 0.50, 10.0, 0))
            .await;
        model
            .apply_record(snapshot_record(Side::Ask, 0.60, 20.0, 1))
            .await;

        // Rotate to only tok2
        model.set_active_assets(vec!["tok2".to_string()]).await;

        assert!(!model.is_asset_active("tok1").await);
        assert!(model.is_asset_active("tok2").await);
    }

    #[tokio::test]
    async fn set_last_rotation_us() {
        let model = LiveReadModel::new(FeedMode::AutoRotate);
        model.set_last_rotation_us(555).await;
        let status = model.feed_status_raw().await;
        assert_eq!(status.last_rotation_us, Some(555));
    }

    #[tokio::test]
    async fn is_hydrated_default_false() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        assert!(!model.is_hydrated());
    }

    #[tokio::test]
    async fn mark_hydrated_sets_flag() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.mark_hydrated().await;
        assert!(model.is_hydrated());
    }

    #[tokio::test]
    async fn snapshot_returns_not_active_for_unknown_asset() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        let err = model.snapshot("unknown", 5, 100).await.unwrap_err();
        assert_eq!(err, SnapshotLookupError::AssetNotActive);
    }

    #[tokio::test]
    async fn active_assets_includes_stale_info() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.set_active_assets(vec!["tok1".to_string()]).await;
        let assets = model.active_assets(60).await;
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].asset_id, "tok1");
        // no data received, so stale and no book
        assert!(assets[0].stale);
        assert!(!assets[0].has_book);
    }

    #[tokio::test]
    async fn hydrate_checkpoint_initializes_book() {
        use pb_types::event::{BookCheckpoint, PriceLevel};

        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.set_active_assets(vec!["tok1".to_string()]).await;

        let checkpoint = BookCheckpoint {
            asset_id: AssetId::new("tok1"),
            checkpoint_timestamp_us: 500,
            bids: vec![PriceLevel {
                price: pb_types::FixedPrice::from_f64(0.50).unwrap(),
                size: pb_types::FixedSize::from_f64(10.0).unwrap(),
            }],
            asks: vec![PriceLevel {
                price: pb_types::FixedPrice::from_f64(0.60).unwrap(),
                size: pb_types::FixedSize::from_f64(20.0).unwrap(),
            }],
            provenance: EventProvenance {
                recv_timestamp_us: 500,
                exchange_timestamp_us: 490,
                source: DataSource::WebSocket,
                source_event_id: None,
                source_session_id: None,
                sequence: None,
                ingest_ordinal: None,
            },
            wal_offset: None,
        };

        model.hydrate_checkpoint(checkpoint).await;
        model.mark_hydrated().await;

        let snap = model.snapshot("tok1", 5, 999_999).await.unwrap();
        assert_eq!(snap.bid_depth, 1);
        assert_eq!(snap.ask_depth, 1);
    }

    #[tokio::test]
    async fn global_warning_surfaced_in_feed_status() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model
            .apply_record(PersistedRecord::Ingest(IngestEvent {
                asset_id: None,
                kind: IngestEventKind::SequenceGap,
                provenance: EventProvenance {
                    recv_timestamp_us: 999,
                    exchange_timestamp_us: 0,
                    source: DataSource::WebSocket,
                    source_event_id: None,
                    source_session_id: None,
                    sequence: None,
                    ingest_ordinal: None,
                },
                expected_sequence: Some(1),
                observed_sequence: Some(5),
                details: Some("global gap".to_string()),
            }))
            .await;

        let status = model.feed_status_raw().await;
        let warning = status.latest_global_warning.unwrap();
        assert_eq!(warning.kind, "sequence_gap");
        assert_eq!(warning.details.as_deref(), Some("global gap"));
    }

    #[tokio::test]
    async fn delta_event_updates_existing_book() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.set_active_assets(vec!["tok1".to_string()]).await;

        // First apply a snapshot group, then materialize
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
                    source_session_id: None,
                    sequence: None,
                    ingest_ordinal: None,
                },
                expected_sequence: None,
                observed_sequence: None,
                details: None,
            }))
            .await;

        // Now apply a delta
        model
            .apply_record(PersistedRecord::Book(pb_types::BookEvent {
                asset_id: AssetId::new("tok1"),
                kind: BookEventKind::Delta,
                side: Side::Bid,
                price: pb_types::FixedPrice::from_f64(0.55).unwrap(),
                size: pb_types::FixedSize::from_f64(5.0).unwrap(),
                provenance: EventProvenance {
                    recv_timestamp_us: 102,
                    exchange_timestamp_us: 100,
                    source: DataSource::WebSocket,
                    source_event_id: None,
                    source_session_id: None,
                    sequence: Some(Sequence::new(2)),
                    ingest_ordinal: None,
                },
            }))
            .await;

        let snap = model.snapshot("tok1", 5, 100).await.unwrap();
        assert_eq!(snap.bid_depth, 2); // original bid + delta bid
    }

    #[tokio::test]
    async fn concurrent_readers_see_consistent_state() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model.set_active_assets(vec!["tok1".to_string()]).await;

        // Apply snapshot + materialize.
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
                    source_session_id: None,
                    sequence: None,
                    ingest_ordinal: None,
                },
                expected_sequence: None,
                observed_sequence: None,
                details: None,
            }))
            .await;

        // Multiple concurrent readers should all see the same consistent state.
        let m1 = model.clone();
        let m2 = model.clone();
        let (s1, s2) = tokio::join!(m1.snapshot("tok1", 5, 100), m2.snapshot("tok1", 5, 100),);
        let s1 = s1.unwrap();
        let s2 = s2.unwrap();
        assert_eq!(s1.sequence, s2.sequence);
        assert_eq!(s1.bid_depth, s2.bid_depth);
        assert_eq!(s1.ask_depth, s2.ask_depth);
    }

    #[tokio::test]
    async fn publish_reuses_unchanged_asset_views() {
        let model = LiveReadModel::new(FeedMode::FixedTokens);
        model
            .set_active_assets(vec!["tok1".to_string(), "tok2".to_string()])
            .await;

        model
            .apply_record(snapshot_record_for("tok1", Side::Bid, 0.50, 10.0, 0))
            .await;
        model
            .apply_record(snapshot_record_for("tok1", Side::Ask, 0.60, 20.0, 1))
            .await;
        model
            .apply_record(snapshot_record_for("tok2", Side::Bid, 0.40, 8.0, 2))
            .await;
        model
            .apply_record(snapshot_record_for("tok2", Side::Ask, 0.70, 12.0, 3))
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
                    ingest_ordinal: None,
                },
                expected_sequence: None,
                observed_sequence: None,
                details: None,
            }))
            .await;

        let before = model.state_rx.borrow().clone();
        let tok1_before = Arc::as_ptr(before.assets.get("tok1").unwrap());
        let tok2_before = Arc::as_ptr(before.assets.get("tok2").unwrap());

        model
            .apply_record(delta_record_for("tok1", Side::Bid, 0.55, 5.0, 102, 100, 4))
            .await;

        let after = model.state_rx.borrow().clone();
        let tok1_after = Arc::as_ptr(after.assets.get("tok1").unwrap());
        let tok2_after = Arc::as_ptr(after.assets.get("tok2").unwrap());

        assert_ne!(tok1_before, tok1_after);
        assert_eq!(tok2_before, tok2_after);
    }
}
