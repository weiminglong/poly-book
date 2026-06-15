use pb_book::L2Book;
use pb_types::event::{
    BookCheckpoint, BookEvent, BookEventKind, IngestEvent, IngestEventKind, MarketDataWindow,
    ReplayMode, ReplayValidation,
};
use pb_types::{AssetId, DataSource, EventProvenance, Sequence, Side};
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
        let checkpoint = self
            .reader
            .read_latest_checkpoint(asset_id, target_timestamp_us)
            .await?;
        let start_us = checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_timestamp_us)
            .unwrap_or_else(|| target_timestamp_us.saturating_sub(self.lookback_us));
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

        // Reconstruct by seeding from the checkpoint *strictly before* the
        // reference and replaying deltas forward to the reference timestamp,
        // then compare against the independent reference checkpoint. The old
        // code called `reconstruct_at(reference_ts)`, whose inclusive checkpoint
        // bound re-read the reference checkpoint itself and an empty
        // `[reference_ts, reference_ts]` window — so the book was seeded from
        // the very thing it was compared against and `matched` was always true
        // (audit findings A.8/A.23).
        let reference_us = reference.checkpoint_timestamp_us;
        let seed = self
            .reader
            .read_latest_checkpoint(asset_id, reference_us.saturating_sub(1))
            .await?;
        let start_us = seed
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_timestamp_us)
            .unwrap_or_else(|| reference_us.saturating_sub(self.lookback_us));
        let window = self
            .reader
            .read_market_data(asset_id, start_us, reference_us)
            .await?;
        let (book, _continuity_events, _used_checkpoint) =
            reconstruct_book(asset_id, reference_us, mode, seed, window)?;

        let matched = books_match_checkpoint(&book, &reference);
        let mismatch_summary = if matched {
            None
        } else {
            Some(render_checkpoint_mismatch(&book, &reference))
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
    let reset_boundary_us = latest_reset_boundary_us(&window.ingest_events, target_timestamp_us);
    let mut continuity_events = std::mem::take(&mut window.ingest_events);
    let mut book = L2Book::new(asset_id.clone());
    let mut used_checkpoint = false;
    let checkpoint = checkpoint.filter(|checkpoint| {
        reset_boundary_us
            .map(|reset_ts| checkpoint.provenance.recv_timestamp_us >= reset_ts)
            .unwrap_or(true)
    });
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
                    && reset_boundary_us
                        .map(|reset_ts| event.provenance.recv_timestamp_us >= reset_ts)
                        .unwrap_or(true)
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
                            ingest_ordinal: None,
                        },
                        expected_sequence: Some(book.sequence.raw() + 1),
                        observed_sequence: Some(next_sequence.raw()),
                        details: Some(error.to_string()),
                    });
                    // Do NOT touch live metrics here: this is offline replay, and
                    // incrementing pb_gaps_detected_total polluted live
                    // observability (A.152). The gap is recorded in
                    // continuity_events for the replay caller.
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

    // Surface a crossed/locked reconstructed book to the replay caller as a
    // continuity marker (A.53). Offline analysis only — no live metric.
    if book.check_integrity().is_err() {
        continuity_events.push(IngestEvent {
            asset_id: Some(asset_id.clone()),
            kind: IngestEventKind::SourceReset,
            provenance: EventProvenance {
                recv_timestamp_us: book.last_update_us,
                exchange_timestamp_us: 0,
                source: DataSource::ReplayValidator,
                source_event_id: None,
                source_session_id: None,
                sequence: None,
                ingest_ordinal: None,
            },
            expected_sequence: None,
            observed_sequence: None,
            details: Some("crossed/locked book in reconstructed state".to_string()),
        });
    }

    Ok((book, continuity_events, used_checkpoint))
}

fn latest_reset_boundary_us(events: &[IngestEvent], target_timestamp_us: u64) -> Option<u64> {
    events
        .iter()
        .filter(|event| {
            event.kind.is_continuity_reset()
                && event.provenance.recv_timestamp_us <= target_timestamp_us
        })
        .map(|event| event.provenance.recv_timestamp_us)
        .max()
}

/// Sort book events into a deterministic total order for replay.
///
/// The primary/secondary keys are clock-domain timestamps. Within the same
/// timestamp the authoritative tiebreaker is `ingest_ordinal` — a process-
/// monotonic counter stamped at ingest in true arrival order — so a
/// same-microsecond pre-snapshot delta (lower ordinal) sorts *before* its
/// snapshot (higher ordinal), which `sequence` could not express because it
/// resets to 0 on every snapshot (audit finding A.116). Events lacking an
/// ordinal (legacy data written before A.116) sort after those that have one at
/// the same timestamp and fall back to `sequence` plus content tiebreakers
/// (side, price, size, source event id) so the order is still a deterministic
/// total order regardless of the concurrent, unordered Parquet read order
/// (`buffer_unordered`) — without which two replays of the same window could
/// diverge (audit finding A.117).
fn sort_book_events(events: &mut [BookEvent], mode: ReplayMode) {
    events.sort_by(|a, b| {
        let (a_primary, a_secondary) = ordering_keys(a, mode);
        let (b_primary, b_secondary) = ordering_keys(b, mode);
        a_primary
            .cmp(&b_primary)
            .then_with(|| a_secondary.cmp(&b_secondary))
            // None sorts last (legacy data) via MAX sentinel; when present, the
            // ordinal alone is a total order within the timestamp.
            .then_with(|| {
                a.provenance
                    .ingest_ordinal
                    .unwrap_or(u64::MAX)
                    .cmp(&b.provenance.ingest_ordinal.unwrap_or(u64::MAX))
            })
            .then_with(|| {
                a.provenance
                    .sequence
                    .unwrap_or_default()
                    .raw()
                    .cmp(&b.provenance.sequence.unwrap_or_default().raw())
            })
            .then_with(|| side_rank(a.side).cmp(&side_rank(b.side)))
            .then_with(|| a.price.raw().cmp(&b.price.raw()))
            .then_with(|| a.size.raw().cmp(&b.size.raw()))
            .then_with(|| {
                a.provenance
                    .source_event_id
                    .cmp(&b.provenance.source_event_id)
            })
    });
}

/// Primary and secondary ordering timestamps for the given replay clock domain.
fn ordering_keys(event: &BookEvent, mode: ReplayMode) -> (u64, u64) {
    match mode {
        ReplayMode::RecvTime => (event.provenance.recv_timestamp_us, 0),
        ReplayMode::ExchangeTime => (
            normalized_exchange_ts(event),
            event.provenance.recv_timestamp_us,
        ),
    }
}

fn side_rank(side: Side) -> u8 {
    match side {
        Side::Bid => 0,
        Side::Ask => 1,
    }
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
    let mut checkpoint_bids: Vec<_> = checkpoint
        .bids
        .iter()
        .map(|level| (level.price, level.size))
        .collect();
    checkpoint_bids.sort_by(|a, b| b.0.cmp(&a.0));
    let mut checkpoint_asks: Vec<_> = checkpoint
        .asks
        .iter()
        .map(|level| (level.price, level.size))
        .collect();
    checkpoint_asks.sort_by_key(|&(price, _)| price);
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

#[cfg(test)]
mod sort_tests {
    use super::*;
    use pb_types::event::BookEventKind;
    use pb_types::{FixedPrice, FixedSize};

    fn book_event(
        kind: BookEventKind,
        side: Side,
        price: u32,
        size: u64,
        recv_ts: u64,
        seq: u64,
        source_event_id: Option<&str>,
    ) -> BookEvent {
        BookEvent {
            asset_id: AssetId::new("tok"),
            kind,
            side,
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::new(size),
            provenance: EventProvenance {
                recv_timestamp_us: recv_ts,
                exchange_timestamp_us: 0,
                source: DataSource::WebSocket,
                source_event_id: source_event_id.map(|s| s.to_string()),
                source_session_id: None,
                sequence: Some(Sequence::new(seq)),
                ingest_ordinal: None,
            },
        }
    }

    /// Different read orders of the same multiset of events must produce an
    /// identical sorted order (deterministic replay — A.117).
    #[test]
    fn sort_is_deterministic_across_input_permutations() {
        // A batch of events that collide on (recv_ts, sequence) so only the
        // content tiebreakers distinguish them.
        let base = vec![
            book_event(BookEventKind::Delta, Side::Bid, 5000, 10, 100, 0, Some("a")),
            book_event(BookEventKind::Delta, Side::Ask, 6000, 20, 100, 0, Some("b")),
            book_event(
                BookEventKind::Snapshot,
                Side::Bid,
                5000,
                30,
                100,
                0,
                Some("c"),
            ),
            book_event(BookEventKind::Delta, Side::Bid, 5000, 40, 100, 0, Some("a")),
            book_event(BookEventKind::Delta, Side::Ask, 5500, 50, 100, 1, None),
            book_event(BookEventKind::Delta, Side::Bid, 4900, 60, 99, 9, Some("z")),
        ];

        let mut canonical = base.clone();
        sort_book_events(&mut canonical, ReplayMode::RecvTime);

        // Several deterministic permutations (reversed, rotated) must all sort to
        // the same canonical order.
        let mut reversed: Vec<BookEvent> = base.iter().rev().cloned().collect();
        sort_book_events(&mut reversed, ReplayMode::RecvTime);
        assert_eq!(reversed, canonical);

        let mut rotated = base.clone();
        rotated.rotate_left(3);
        sort_book_events(&mut rotated, ReplayMode::RecvTime);
        assert_eq!(rotated, canonical);
    }

    /// The sort must be a strict total order on the content tiebreakers (no
    /// adjacent elements compare Equal), otherwise ties could still resolve
    /// nondeterministically under an unstable read order.
    #[test]
    fn sort_breaks_all_ties_deterministically() {
        let mut events = vec![
            book_event(BookEventKind::Delta, Side::Bid, 5000, 10, 100, 0, Some("a")),
            book_event(BookEventKind::Delta, Side::Ask, 6000, 20, 100, 0, Some("b")),
            book_event(
                BookEventKind::Snapshot,
                Side::Bid,
                7000,
                30,
                100,
                0,
                Some("c"),
            ),
            book_event(BookEventKind::Delta, Side::Ask, 5500, 50, 100, 0, Some("d")),
        ];
        sort_book_events(&mut events, ReplayMode::RecvTime);
        for pair in events.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            let differs = side_rank(a.side) != side_rank(b.side)
                || a.price.raw() != b.price.raw()
                || a.size.raw() != b.size.raw()
                || a.provenance.source_event_id != b.provenance.source_event_id
                || a.provenance.sequence != b.provenance.sequence;
            assert!(
                differs,
                "adjacent events are indistinguishable: {a:?} vs {b:?}"
            );
        }
    }

    fn with_ordinal(mut event: BookEvent, ordinal: u64) -> BookEvent {
        event.provenance.ingest_ordinal = Some(ordinal);
        event
    }

    /// A same-microsecond delta that arrived *before* the snapshot (lower ingest
    /// ordinal) must sort before the snapshot, even though its `sequence` is
    /// higher than the snapshot's reset-to-0 sequence (audit finding A.116).
    #[test]
    fn pre_snapshot_delta_sorts_before_snapshot_via_ingest_ordinal() {
        // Same recv_ts. Delta arrived first (ordinal 7) with sequence 42; the
        // snapshot arrived next (ordinal 8) and reset sequence to 0.
        let delta = with_ordinal(
            book_event(
                BookEventKind::Delta,
                Side::Bid,
                5000,
                10,
                100,
                42,
                Some("d"),
            ),
            7,
        );
        let snapshot = with_ordinal(
            book_event(
                BookEventKind::Snapshot,
                Side::Bid,
                5000,
                30,
                100,
                0,
                Some("s"),
            ),
            8,
        );

        // Present the snapshot first to prove ordering is by ordinal, not input.
        let mut events = vec![snapshot, delta];
        sort_book_events(&mut events, ReplayMode::RecvTime);

        assert_eq!(
            events[0].kind,
            BookEventKind::Delta,
            "delta must sort first"
        );
        assert_eq!(events[0].provenance.ingest_ordinal, Some(7));
        assert_eq!(events[1].kind, BookEventKind::Snapshot);
        assert_eq!(events[1].provenance.ingest_ordinal, Some(8));
    }

    /// Without the ingest ordinal (legacy data), the snapshot's reset sequence
    /// (0) makes the pre-snapshot delta sort AFTER it — the exact A.116 bug. This
    /// documents why the ordinal is required (it is not a regression: legacy data
    /// has no better signal).
    #[test]
    fn legacy_events_without_ordinal_fall_back_to_sequence() {
        let delta = book_event(
            BookEventKind::Delta,
            Side::Bid,
            5000,
            10,
            100,
            42,
            Some("d"),
        );
        let snapshot = book_event(
            BookEventKind::Snapshot,
            Side::Bid,
            5000,
            30,
            100,
            0,
            Some("s"),
        );
        let mut events = vec![delta, snapshot];
        sort_book_events(&mut events, ReplayMode::RecvTime);
        // Snapshot (seq 0) sorts before delta (seq 42) — the legacy fallback.
        assert_eq!(events[0].kind, BookEventKind::Snapshot);
        assert_eq!(events[1].kind, BookEventKind::Delta);
    }
}
