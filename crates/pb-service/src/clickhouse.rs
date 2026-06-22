//! ClickHouse-backed service implementations for interactive historical queries.

use pb_replay::{ClickHouseReader, EventReader, ReplayEngine};
use pb_types::AssetId;

use crate::{
    assemble_integrity_summary, build_execution_timeline, build_replay_result, map_replay_error,
    ExecutionService, ExecutionTimeline, IntegrityService, IntegritySummary, ReplayResult,
    ReplayService, ServiceError,
};

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
        Ok(build_replay_result(asset_id, at_us, depth, result))
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
        // Push the heavy counts (book events, validations) to the server with
        // count()/countIf() instead of streaming every row back to count it
        // client-side. Only the bounded ingest-event list is
        // materialized, since it feeds both the per-kind tallies and the
        // continuity_events array.
        let (aggregates, ingest_events) = tokio::try_join!(
            async {
                reader
                    .read_integrity_aggregates(asset_id, start_us, end_us)
                    .await
                    .map_err(map_replay_error)
            },
            async {
                reader
                    .read_ingest_events(asset_id, start_us, end_us)
                    .await
                    .map_err(map_replay_error)
            },
        )?;
        Ok(assemble_integrity_summary(
            asset_id,
            start_us,
            end_us,
            aggregates.book_event_count as usize,
            &ingest_events,
            aggregates.validation_count as usize,
            aggregates.validation_match_count as usize,
        ))
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
        offset: usize,
        descending: bool,
    ) -> Result<ExecutionTimeline, ServiceError> {
        let reader = ClickHouseReader::new(&self.url, &self.database);
        let events = reader
            .read_execution_events(order_id, start_us, end_us)
            .await
            .map_err(map_replay_error)?;
        Ok(build_execution_timeline(
            events, asset_id, limit, offset, descending,
        ))
    }
}
