use pb_book::L2Book;
use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, IngestEvent, IngestEventKind, MarketDataWindow,
    ReplayMode, ReplayValidation,
};
use pb_types::{AssetId, EventProvenance, Sequence, Side};
use tracing::debug;

use crate::error::ReplayError;
use crate::reader::EventReader;

const DEFAULT_LOOKBACK_US: u64 = 3_600_000_000;

#[derive(Debug, Clone)]
pub struct ReplayResult {
    pub book: L2Book,
    pub mode: ReplayMode,
    pub used_checkpoint: bool,
    pub continuity_events: Vec<IngestEvent>,
}

pub struct ReplayEngine<R: EventReader> {
    reader: R,
    lookback_us: u64,
}

impl<R: EventReader> ReplayEngine<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            lookback_us: DEFAULT_LOOKBACK_US,
        }
    }

    pub fn with_lookback_us(mut self, lookback_us: u64) -> Self {
        self.lookback_us = lookback_us;
        self
    }

    pub async fn reconstruct_at(
        &self,
        asset_id: &AssetId,
        target_timestamp_us: u64,
        mode: ReplayMode,
    ) -> Result<ReplayResult, ReplayError> {
        let start_us = target_timestamp_us.saturating_sub(self.lookback_us);
        let checkpoint = self
            .reader
            .read_latest_checkpoint(asset_id, target_timestamp_us)
            .await?;
        let window = self
            .reader
            .read_market_data(asset_id, start_us, target_timestamp_us)
            .await?;

        let (book, continuity_events, used_checkpoint) =
            reconstruct_book(asset_id, target_timestamp_us, mode, checkpoint, window)?;

        Ok(ReplayResult {
            book,
            mode,
            used_checkpoint,
            continuity_events,
        })
    }

    pub async fn replay_window(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<MarketDataWindow, ReplayError> {
        self.reader
            .read_market_data(asset_id, start_us, end_us)
            .await
    }

    pub async fn validate_at(
        &self,
        asset_id: &AssetId,
        replay_timestamp_us: u64,
        mode: ReplayMode,
    ) -> Result<Option<ReplayValidation>, ReplayError> {
        self.replay_validation(asset_id, replay_timestamp_us, mode)
            .await
    }

    pub async fn replay_validation(
        &self,
        asset_id: &AssetId,
        replay_timestamp_us: u64,
        mode: ReplayMode,
    ) -> Result<Option<ReplayValidation>, ReplayError> {
        let checkpoints = self
            .reader
            .read_checkpoints(
                asset_id,
                replay_timestamp_us,
                replay_timestamp_us.saturating_add(self.lookback_us),
            )
            .await?;
        let Some(reference) = checkpoints
            .into_iter()
            .find(|checkpoint| checkpoint.checkpoint_timestamp_us > replay_timestamp_us)
        else {
            return Ok(None);
        };

        let replay = self
            .reconstruct_at(asset_id, reference.checkpoint_timestamp_us, mode)
            .await?;
        let matched = books_match_checkpoint(&replay.book, &reference);
        let mismatch_summary = if matched {
            None
        } else {
            Some(render_checkpoint_mismatch(&replay.book, &reference))
        };
        let persisted_at_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        Ok(Some(ReplayValidation {
            asset_id: asset_id.clone(),
            mode,
            replay_timestamp_us,
            reference_timestamp_us: reference.checkpoint_timestamp_us,
            matched,
            mismatch_summary,
            persisted_at_us,
        }))
    }

    pub async fn execution_events(
        &self,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<pb_types::ExecutionEvent>, ReplayError> {
        self.reader
            .read_execution_events(order_id, start_us, end_us)
            .await
    }
}

fn reconstruct_book(
    asset_id: &AssetId,
    target_timestamp_us: u64,
    mode: ReplayMode,
    checkpoint: Option<BookCheckpoint>,
    mut window: MarketDataWindow,
) -> Result<(L2Book, Vec<IngestEvent>, bool), ReplayError> {
    sort_book_events(&mut window.book_events, mode);
    window
        .ingest_events
        .sort_by_key(|event| event.provenance.recv_timestamp_us);
    let mut continuity_events = window.ingest_events.clone();
    let mut book = L2Book::new(asset_id.clone());
    let mut used_checkpoint = false;
    let start_idx = if let Some(checkpoint) = checkpoint {
        debug!(
            asset_id = %asset_id,
            checkpoint_ts = checkpoint.checkpoint_timestamp_us,
            target_ts = target_timestamp_us,
            "reconstructing from checkpoint"
        );
        apply_checkpoint(&mut book, &checkpoint);
        used_checkpoint = true;
        window
            .book_events
            .iter()
            .position(|event| event_ordering_ts(event, mode) > checkpoint.checkpoint_timestamp_us)
            .unwrap_or(window.book_events.len())
    } else {
        let snapshot_idx = window
            .book_events
            .iter()
            .rposition(|event| {
                event.kind == BookEventKind::Snapshot
                    && event_ordering_ts(event, mode) <= target_timestamp_us
            })
            .ok_or_else(|| ReplayError::NoSnapshotFound {
                asset_id: asset_id.to_string(),
                timestamp_us: target_timestamp_us,
            })?;
        let snapshot_time = event_ordering_ts(&window.book_events[snapshot_idx], mode);
        let snapshot_events: Vec<&BookEvent> = window
            .book_events
            .iter()
            .filter(|event| {
                event.kind == BookEventKind::Snapshot
                    && event_ordering_ts(event, mode) == snapshot_time
            })
            .collect();

        debug!(
            asset_id = %asset_id,
            snapshot_ts = snapshot_time,
            target_ts = target_timestamp_us,
            "found snapshot for reconstruction"
        );

        apply_snapshot_events(&mut book, &snapshot_events, snapshot_time);
        snapshot_idx + 1
    };

    let mut idx = start_idx;
    while idx < window.book_events.len() {
        let current_time = event_ordering_ts(&window.book_events[idx], mode);
        if current_time > target_timestamp_us {
            break;
        }
        let event = &window.book_events[idx];
        match event.kind {
            BookEventKind::Snapshot => {
                let snapshot_events: Vec<&BookEvent> = window
                    .book_events
                    .iter()
                    .skip(idx)
                    .take_while(|candidate| {
                        candidate.kind == BookEventKind::Snapshot
                            && event_ordering_ts(candidate, mode) == current_time
                    })
                    .collect();
                apply_snapshot_events(&mut book, &snapshot_events, current_time);
                idx += snapshot_events.len();
                continue;
            }
            BookEventKind::Delta => {
                let next_sequence = event.provenance.sequence.unwrap_or_default();
                if let Err(error) = book.check_sequence(next_sequence) {
                    continuity_events.push(IngestEvent {
                        asset_id: Some(asset_id.clone()),
                        kind: IngestEventKind::SequenceGap,
                        provenance: EventProvenance {
                            recv_timestamp_us: event.provenance.recv_timestamp_us,
                            exchange_timestamp_us: event.provenance.exchange_timestamp_us,
                            source: event.provenance.source,
                            source_event_id: event.provenance.source_event_id.clone(),
                            source_session_id: event.provenance.source_session_id.clone(),
                            sequence: event.provenance.sequence,
                        },
                        expected_sequence: Some(book.sequence.raw() + 1),
                        observed_sequence: Some(next_sequence.raw()),
                        details: Some(error.to_string()),
                    });
                    pb_metrics::record_gap_detected();
                }
                book.apply_delta(
                    event.side,
                    event.price,
                    event.size,
                    next_sequence,
                    event.provenance.recv_timestamp_us,
                );
            }
        }
        idx += 1;
    }

    Ok((book, continuity_events, used_checkpoint))
}

fn sort_book_events(events: &mut [BookEvent], mode: ReplayMode) {
    events.sort_by_key(|event| match mode {
        ReplayMode::RecvTime => (
            event.provenance.recv_timestamp_us,
            0,
            event.provenance.sequence.unwrap_or_default().raw(),
        ),
        ReplayMode::ExchangeTime => (
            normalized_exchange_ts(event),
            event.provenance.recv_timestamp_us,
            event.provenance.sequence.unwrap_or_default().raw(),
        ),
    });
}

fn normalized_exchange_ts(event: &BookEvent) -> u64 {
    if event.provenance.exchange_timestamp_us == 0 {
        event.provenance.recv_timestamp_us
    } else {
        event.provenance.exchange_timestamp_us
    }
}

fn event_ordering_ts(event: &BookEvent, mode: ReplayMode) -> u64 {
    match mode {
        ReplayMode::RecvTime => event.provenance.recv_timestamp_us,
        ReplayMode::ExchangeTime => normalized_exchange_ts(event),
    }
}

fn apply_checkpoint(book: &mut L2Book, checkpoint: &BookCheckpoint) {
    let bids = checkpoint
        .bids
        .iter()
        .map(|level| (level.price, level.size))
        .collect::<Vec<_>>();
    let asks = checkpoint
        .asks
        .iter()
        .map(|level| (level.price, level.size))
        .collect::<Vec<_>>();
    book.apply_snapshot(
        &bids,
        &asks,
        Sequence::default(),
        checkpoint.checkpoint_timestamp_us,
    );
}

fn apply_snapshot_events(book: &mut L2Book, snapshot_events: &[&BookEvent], timestamp_us: u64) {
    let mut bids = Vec::new();
    let mut asks = Vec::new();
    let mut sequence = Sequence::default();
    for event in snapshot_events {
        sequence = event.provenance.sequence.unwrap_or_default();
        match event.side {
            Side::Bid => bids.push((event.price, event.size)),
            Side::Ask => asks.push((event.price, event.size)),
        }
    }
    book.apply_snapshot(&bids, &asks, sequence, timestamp_us);
}

fn books_match_checkpoint(book: &L2Book, checkpoint: &BookCheckpoint) -> bool {
    let checkpoint_bids = checkpoint
        .bids
        .iter()
        .map(|level| (level.price, level.size))
        .collect::<Vec<_>>();
    let checkpoint_asks = checkpoint
        .asks
        .iter()
        .map(|level| (level.price, level.size))
        .collect::<Vec<_>>();
    book.bids_sorted() == checkpoint_bids && book.asks_sorted() == checkpoint_asks
}

fn render_checkpoint_mismatch(book: &L2Book, checkpoint: &BookCheckpoint) -> String {
    format!(
        "bid_depth={} checkpoint_bid_depth={} ask_depth={} checkpoint_ask_depth={} best_bid={:?} checkpoint_best_bid={:?} best_ask={:?} checkpoint_best_ask={:?}",
        book.bid_depth(),
        checkpoint.bids.len(),
        book.ask_depth(),
        checkpoint.asks.len(),
        book.best_bid(),
        checkpoint.bids.first().map(|level| (level.price, level.size)),
        book.best_ask(),
        checkpoint.asks.first().map(|level| (level.price, level.size)),
    )
}
