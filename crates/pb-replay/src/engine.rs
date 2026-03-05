use pb_book::L2Book;
use pb_types::event::{EventType, OrderbookEvent};
use pb_types::{AssetId, Sequence};
use tracing::debug;

use crate::error::ReplayError;
use crate::reader::EventReader;

/// Default lookback window (1 hour in microseconds) for finding snapshots.
const DEFAULT_LOOKBACK_US: u64 = 3_600_000_000;

/// Engine for reconstructing orderbook state from stored events.
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

    /// Set the lookback window for finding snapshots.
    pub fn with_lookback_us(mut self, lookback_us: u64) -> Self {
        self.lookback_us = lookback_us;
        self
    }

    /// Reconstruct the L2Book at a specific point in time.
    ///
    /// Finds the most recent snapshot before `target_timestamp_us`,
    /// then applies all deltas up to the target timestamp.
    pub async fn reconstruct_at(
        &self,
        asset_id: &AssetId,
        target_timestamp_us: u64,
    ) -> Result<L2Book, ReplayError> {
        let start_us = target_timestamp_us.saturating_sub(self.lookback_us);
        let events = self
            .reader
            .read_events(asset_id, start_us, target_timestamp_us)
            .await?;

        // Find the most recent snapshot before target
        let snapshot_idx = events
            .iter()
            .rposition(|e| {
                e.event_type == EventType::Snapshot && e.recv_timestamp_us <= target_timestamp_us
            })
            .ok_or_else(|| ReplayError::NoSnapshotFound {
                asset_id: asset_id.to_string(),
                timestamp_us: target_timestamp_us,
            })?;

        let snapshot_event = &events[snapshot_idx];
        let snapshot_ts = snapshot_event.recv_timestamp_us;

        debug!(
            asset_id = %asset_id,
            snapshot_ts,
            target_ts = target_timestamp_us,
            "found snapshot for reconstruction"
        );

        // Collect all snapshot events at this exact timestamp to build the full snapshot
        let snapshot_events: Vec<&OrderbookEvent> = events
            .iter()
            .filter(|e| e.event_type == EventType::Snapshot && e.recv_timestamp_us == snapshot_ts)
            .collect();

        let mut book = L2Book::new(asset_id.clone());

        // Build bid/ask arrays from snapshot events
        let mut bids = Vec::new();
        let mut asks = Vec::new();
        let mut snap_seq = Sequence::default();

        for evt in &snapshot_events {
            snap_seq = evt.sequence;
            match evt.side {
                Some(pb_types::Side::Bid) => {
                    bids.push((evt.price, evt.size));
                }
                Some(pb_types::Side::Ask) => {
                    asks.push((evt.price, evt.size));
                }
                None => {
                    // Snapshot event without side - skip
                }
            }
        }

        book.apply_snapshot(&bids, &asks, snap_seq, snapshot_ts);

        // Apply deltas between snapshot and target.
        // Sequences are per-asset, so check_sequence detects real gaps.
        for event in events.iter().skip(snapshot_idx + 1) {
            if event.recv_timestamp_us > target_timestamp_us {
                break;
            }
            if event.event_type == EventType::Delta {
                if let Err(e) = book.check_sequence(event.sequence) {
                    tracing::warn!(
                        asset_id = %asset_id,
                        expected = book.sequence.raw() + 1,
                        got = event.sequence.raw(),
                        "sequence gap during replay"
                    );
                    pb_metrics::record_gap_detected();
                    let _ = e;
                }
                if let Some(side) = event.side {
                    book.apply_delta(
                        side,
                        event.price,
                        event.size,
                        event.sequence,
                        event.recv_timestamp_us,
                    );
                }
            }
        }

        Ok(book)
    }

    /// Read all events in the given time range, ordered by recv_timestamp_us.
    pub async fn replay_range(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<Vec<OrderbookEvent>, ReplayError> {
        self.reader.read_events(asset_id, start_us, end_us).await
    }
}
