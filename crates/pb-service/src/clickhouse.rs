//! ClickHouse-backed service implementations for interactive historical queries.

use pb_replay::{ClickHouseReader, EventReader, ReplayEngine, ReplayError};
use pb_types::event::IngestEventKind;
use pb_types::AssetId;

use crate::{
    CompletenessLevel, ContinuityEvent, ExecutionService, ExecutionTimeline, IntegrityService,
    IntegritySummary, ReplayResult, ReplayService, ServiceError,
};

fn map_replay_error(error: ReplayError) -> ServiceError {
    match error {
        ReplayError::NoSnapshotFound {
            asset_id,
            timestamp_us,
        } => ServiceError::NotFound(format!(
            "no snapshot found for asset {asset_id} before timestamp {timestamp_us}"
        )),
        other => ServiceError::Internal(other.to_string()),
    }
}

fn ingest_to_continuity(event: &pb_types::IngestEvent) -> ContinuityEvent {
    ContinuityEvent {
        kind: event.kind.to_string(),
        recv_timestamp_us: event.provenance.recv_timestamp_us,
        exchange_timestamp_us: event.provenance.exchange_timestamp_us,
        details: event.details.clone(),
    }
}

// ---------------------------------------------------------------------------
// ClickHouseReplayService
// ---------------------------------------------------------------------------

/// Replay service backed by ClickHouse for interactive queries.
#[derive(Clone)]
pub struct ClickHouseReplayService {
    url: String,
    database: String,
}

impl ClickHouseReplayService {
    pub fn new(url: impl Into<String>, database: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            database: database.into(),
        }
    }
}

impl ReplayService for ClickHouseReplayService {
    async fn reconstruct(
        &self,
        asset_id: &AssetId,
        at_us: u64,
        mode: pb_types::event::ReplayMode,
        depth: Option<usize>,
    ) -> Result<ReplayResult, ServiceError> {
        let reader = ClickHouseReader::new(&self.url, &self.database);
        let engine = ReplayEngine::new(reader);
        let result = engine
            .reconstruct_at(asset_id, at_us, mode)
            .await
            .map_err(map_replay_error)?;
        let book = &result.book;
        let d = depth.unwrap_or(book.bid_depth().max(book.ask_depth()));

        Ok(ReplayResult {
            asset_id: asset_id.to_string(),
            timestamp_us: at_us,
            mode,
            sequence: book.sequence.raw(),
            best_bid: book.best_bid(),
            best_ask: book.best_ask(),
            mid_price: book.mid_price(),
            spread: book.spread(),
            bid_depth: book.bid_depth(),
            ask_depth: book.ask_depth(),
            bids: book.top_bids(d),
            asks: book.top_asks(d),
            used_checkpoint: result.used_checkpoint,
            continuity_events: result
                .continuity_events
                .iter()
                .map(ingest_to_continuity)
                .collect(),
        })
    }
}

// ---------------------------------------------------------------------------
// ClickHouseIntegrityService
// ---------------------------------------------------------------------------

/// Integrity service backed by ClickHouse for interactive queries.
#[derive(Clone)]
pub struct ClickHouseIntegrityService {
    url: String,
    database: String,
}

impl ClickHouseIntegrityService {
    pub fn new(url: impl Into<String>, database: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            database: database.into(),
        }
    }
}

impl IntegrityService for ClickHouseIntegrityService {
    async fn summary(
        &self,
        asset_id: &AssetId,
        start_us: u64,
        end_us: u64,
    ) -> Result<IntegritySummary, ServiceError> {
        let reader = ClickHouseReader::new(&self.url, &self.database);
        let window = reader
            .read_market_data(asset_id, start_us, end_us)
            .await
            .map_err(map_replay_error)?;
        let validations = reader
            .read_validations(asset_id, start_us, end_us)
            .await
            .map_err(map_replay_error)?;

        let book_event_count = window.book_events.len();
        let ingest_event_count = window.ingest_events.len();

        let mut reconnect_count = 0usize;
        let mut gap_count = 0usize;
        let mut stale_snapshot_skip_count = 0usize;

        for event in &window.ingest_events {
            match event.kind {
                IngestEventKind::ReconnectStart | IngestEventKind::ReconnectSuccess => {
                    reconnect_count += 1;
                }
                IngestEventKind::SequenceGap | IngestEventKind::SourceReset => {
                    gap_count += 1;
                }
                IngestEventKind::StaleSnapshotSkip => {
                    stale_snapshot_skip_count += 1;
                }
            }
        }

        let validation_count = validations.len();
        let validation_match_count = validations.iter().filter(|v| v.matched).count();

        let has_boundaries = reconnect_count > 0 || gap_count > 0;
        let completeness = if book_event_count == 0 {
            CompletenessLevel::Empty
        } else if has_boundaries {
            CompletenessLevel::Partial
        } else {
            CompletenessLevel::Full
        };

        let continuity_events = window
            .ingest_events
            .iter()
            .map(ingest_to_continuity)
            .collect();

        Ok(IntegritySummary {
            asset_id: asset_id.to_string(),
            start_us,
            end_us,
            book_event_count,
            trade_event_count: 0,
            ingest_event_count,
            checkpoint_count: 0,
            reconnect_count,
            gap_count,
            stale_snapshot_skip_count,
            validation_count,
            validation_match_count,
            completeness,
            continuity_events,
        })
    }
}

// ---------------------------------------------------------------------------
// ClickHouseExecutionService
// ---------------------------------------------------------------------------

/// Execution service backed by ClickHouse for interactive queries.
#[derive(Clone)]
pub struct ClickHouseExecutionService {
    url: String,
    database: String,
}

impl ClickHouseExecutionService {
    pub fn new(url: impl Into<String>, database: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            database: database.into(),
        }
    }
}

impl ExecutionService for ClickHouseExecutionService {
    async fn timeline(
        &self,
        asset_id: Option<&AssetId>,
        order_id: Option<&str>,
        start_us: u64,
        end_us: u64,
        limit: usize,
    ) -> Result<ExecutionTimeline, ServiceError> {
        let reader = ClickHouseReader::new(&self.url, &self.database);
        let mut events = reader
            .read_execution_events(order_id, start_us, end_us)
            .await
            .map_err(map_replay_error)?;

        if let Some(filter_id) = asset_id {
            events.retain(|e| {
                e.asset_id
                    .as_ref()
                    .map(|id| id == filter_id)
                    .unwrap_or(false)
            });
        }

        let total_count = events.len();
        events.truncate(limit);

        Ok(ExecutionTimeline {
            events,
            total_count,
        })
    }
}
