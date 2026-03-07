use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use pb_replay::{EventReader, ParquetReader, ReplayEngine, ReplayError};
use pb_types::{AssetId, IngestEvent, IngestEventKind, ReplayMode};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::dto::{
    CompletenessLabel, ContinuityWarning, ExecutionEventView, ExecutionTimelineResponse,
    FeedStatusResponse, IntegritySummaryResponse, LatencyTraceView, LiveOrderBookSnapshot,
    ReplayReconstructionResponse,
};
use crate::error::ApiError;
use crate::live_state::{LiveReadModel, SnapshotLookupError};

#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub parquet_base_path: String,
    pub default_depth: usize,
    pub max_depth: usize,
    pub stale_after_secs: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub live: LiveReadModel,
    pub config: ApiConfig,
    pub broadcast: Option<crate::streaming::BookBroadcast>,
}

#[derive(Debug, Deserialize)]
struct DepthQuery {
    depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ReplayQuery {
    asset_id: String,
    at_us: u64,
    mode: String,
    source: Option<String>,
    depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct IntegrityQuery {
    asset_id: String,
    start_us: u64,
    end_us: u64,
}

#[derive(Debug, Deserialize)]
struct ExecutionQuery {
    order_id: Option<String>,
    asset_id: Option<String>,
    start_us: u64,
    end_us: u64,
    limit: Option<usize>,
}

pub fn router(state: AppState) -> Router {
    use axum::routing::any;

    Router::new()
        .route("/api/v1/feed/status", get(feed_status))
        .route("/api/v1/assets/active", get(active_assets))
        .route(
            "/api/v1/orderbooks/{asset_id}/snapshot",
            get(orderbook_snapshot),
        )
        .route("/api/v1/replay/reconstruct", get(replay_reconstruct))
        .route("/api/v1/integrity/summary", get(integrity_summary))
        .route("/api/v1/execution/orders", get(execution_orders))
        .route(
            "/api/v1/streams/orderbook",
            any(crate::streaming::ws_orderbook),
        )
        .with_state(state)
}

pub async fn serve(
    listener: TcpListener,
    state: AppState,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            shutdown.cancelled().await;
        })
        .await
}

async fn feed_status(State(state): State<AppState>) -> Json<FeedStatusResponse> {
    Json(state.live.feed_status().await)
}

async fn active_assets(State(state): State<AppState>) -> Json<Vec<crate::dto::ActiveAssetSummary>> {
    Json(
        state
            .live
            .active_assets(state.config.stale_after_secs)
            .await,
    )
}

async fn orderbook_snapshot(
    State(state): State<AppState>,
    Path(asset_id): Path<String>,
    Query(query): Query<DepthQuery>,
) -> Result<Json<LiveOrderBookSnapshot>, ApiError> {
    let depth = validate_depth(
        query.depth.unwrap_or(state.config.default_depth),
        state.config.max_depth,
    )?;
    match state
        .live
        .snapshot(&asset_id, depth, state.config.stale_after_secs)
        .await
    {
        Ok(snapshot) => Ok(Json(snapshot)),
        Err(SnapshotLookupError::AssetNotActive) => {
            Err(ApiError::NotFound(format!("asset not active: {asset_id}")))
        }
        Err(SnapshotLookupError::SnapshotNotReady) => Err(ApiError::ServiceUnavailable(format!(
            "snapshot not ready for asset: {asset_id}"
        ))),
    }
}

async fn replay_reconstruct(
    State(state): State<AppState>,
    Query(query): Query<ReplayQuery>,
) -> Result<Json<ReplayReconstructionResponse>, ApiError> {
    let depth = validate_depth(
        query.depth.unwrap_or(state.config.default_depth),
        state.config.max_depth,
    )?;
    let source = query.source.as_deref().unwrap_or("parquet");
    if source != "parquet" {
        return Err(ApiError::BadRequest(format!(
            "unsupported replay source: {source}"
        )));
    }
    let mode = parse_replay_mode(&query.mode)?;
    let reader = ParquetReader::new(state.config.parquet_base_path.clone());
    let engine = ReplayEngine::new(reader);
    let asset_id = AssetId::new(query.asset_id.clone());
    let replay = engine
        .reconstruct_at(&asset_id, query.at_us, mode)
        .await
        .map_err(map_replay_error)?;
    let book = replay.book;

    Ok(Json(ReplayReconstructionResponse {
        asset_id: query.asset_id,
        mode: mode.to_string(),
        used_checkpoint: replay.used_checkpoint,
        sequence: book.sequence.raw(),
        last_update_us: book.last_update_us,
        best_bid: book.best_bid().map(level_view),
        best_ask: book.best_ask().map(level_view),
        mid_price: book.mid_price(),
        spread: book.spread(),
        bid_depth: book.bid_depth(),
        ask_depth: book.ask_depth(),
        bids: book
            .bids_sorted()
            .into_iter()
            .take(depth)
            .map(level_view)
            .collect(),
        asks: book
            .asks_sorted()
            .into_iter()
            .take(depth)
            .map(level_view)
            .collect(),
        continuity_events: replay
            .continuity_events
            .into_iter()
            .map(continuity_warning)
            .collect(),
    }))
}

const EXECUTION_DEFAULT_LIMIT: usize = 100;
const EXECUTION_MAX_LIMIT: usize = 1000;
const MAX_QUERY_WINDOW_US: u64 = 24 * 3_600 * 1_000_000; // 24 hours

fn validate_time_window(start_us: u64, end_us: u64) -> Result<(), ApiError> {
    if start_us >= end_us {
        return Err(ApiError::BadRequest(
            "start_us must be less than end_us".to_string(),
        ));
    }
    if end_us - start_us > MAX_QUERY_WINDOW_US {
        return Err(ApiError::BadRequest(format!(
            "time window exceeds maximum of {} hours",
            MAX_QUERY_WINDOW_US / 3_600_000_000
        )));
    }
    Ok(())
}

async fn integrity_summary(
    State(state): State<AppState>,
    Query(query): Query<IntegrityQuery>,
) -> Result<Json<IntegritySummaryResponse>, ApiError> {
    validate_time_window(query.start_us, query.end_us)?;
    let reader = ParquetReader::new(state.config.parquet_base_path.clone());
    let asset_id = AssetId::new(query.asset_id.clone());

    let window = reader
        .read_market_data(&asset_id, query.start_us, query.end_us)
        .await
        .map_err(map_replay_error)?;
    let validations = reader
        .read_validations(&asset_id, query.start_us, query.end_us)
        .await
        .map_err(map_replay_error)?;

    let total_book_events = window.book_events.len() as u64;
    let total_ingest_events = window.ingest_events.len() as u64;

    let mut reconnect_count = 0u32;
    let mut gap_count = 0u32;
    let mut stale_snapshot_skip_count = 0u32;

    for event in &window.ingest_events {
        match event.kind {
            IngestEventKind::ReconnectStart | IngestEventKind::ReconnectSuccess => {
                reconnect_count += 1;
            }
            IngestEventKind::SequenceGap => gap_count += 1,
            IngestEventKind::StaleSnapshotSkip => stale_snapshot_skip_count += 1,
            IngestEventKind::SourceReset => gap_count += 1,
        }
    }

    let validation_count = validations.len() as u32;
    let validations_matched = validations.iter().filter(|v| v.matched).count() as u32;
    let validations_mismatched = validation_count - validations_matched;

    let has_continuity_boundaries = reconnect_count > 0 || gap_count > 0;
    let completeness = if has_continuity_boundaries {
        CompletenessLabel::BestEffort
    } else {
        CompletenessLabel::Complete
    };

    let continuity_events = window
        .ingest_events
        .into_iter()
        .map(continuity_warning)
        .collect();

    Ok(Json(IntegritySummaryResponse {
        asset_id: query.asset_id,
        start_us: query.start_us,
        end_us: query.end_us,
        total_book_events,
        total_ingest_events,
        reconnect_count,
        gap_count,
        stale_snapshot_skip_count,
        validation_count,
        validations_matched,
        validations_mismatched,
        completeness,
        continuity_events,
    }))
}

async fn execution_orders(
    State(state): State<AppState>,
    Query(query): Query<ExecutionQuery>,
) -> Result<Json<ExecutionTimelineResponse>, ApiError> {
    validate_time_window(query.start_us, query.end_us)?;
    let limit = query.limit.unwrap_or(EXECUTION_DEFAULT_LIMIT);
    if limit == 0 || limit > EXECUTION_MAX_LIMIT {
        return Err(ApiError::BadRequest(format!(
            "limit must be between 1 and {EXECUTION_MAX_LIMIT}"
        )));
    }

    let reader = ParquetReader::new(state.config.parquet_base_path.clone());
    let mut events = reader
        .read_execution_events(query.order_id.as_deref(), query.start_us, query.end_us)
        .await
        .map_err(map_replay_error)?;

    if let Some(asset_filter) = &query.asset_id {
        events.retain(|e| {
            e.asset_id
                .as_ref()
                .map(|id| id.as_str() == asset_filter)
                .unwrap_or(false)
        });
    }

    let total_count = events.len() as u64;
    events.truncate(limit);

    let views: Vec<ExecutionEventView> = events.into_iter().map(execution_event_view).collect();

    Ok(Json(ExecutionTimelineResponse {
        events: views,
        total_count,
    }))
}

fn execution_event_view(event: pb_types::ExecutionEvent) -> ExecutionEventView {
    ExecutionEventView {
        event_timestamp_us: event.event_timestamp_us,
        asset_id: event.asset_id.map(|id| id.to_string()),
        order_id: event.order_id,
        client_order_id: event.client_order_id,
        venue_order_id: event.venue_order_id,
        kind: event.kind.to_string(),
        side: event.side.map(|s| s.to_string()),
        price: event.price,
        size: event.size,
        status: event.status,
        reason: event.reason,
        latency: LatencyTraceView {
            market_data_recv_us: event.latency.market_data_recv_us,
            normalization_done_us: event.latency.normalization_done_us,
            strategy_decision_us: event.latency.strategy_decision_us,
            order_submit_us: event.latency.order_submit_us,
            exchange_ack_us: event.latency.exchange_ack_us,
            exchange_fill_us: event.latency.exchange_fill_us,
        },
    }
}

fn validate_depth(depth: usize, max_depth: usize) -> Result<usize, ApiError> {
    if depth == 0 {
        return Err(ApiError::BadRequest(
            "depth must be greater than zero".to_string(),
        ));
    }
    if depth > max_depth {
        return Err(ApiError::BadRequest(format!(
            "depth {depth} exceeds max_depth {max_depth}"
        )));
    }
    Ok(depth)
}

fn parse_replay_mode(raw: &str) -> Result<ReplayMode, ApiError> {
    match raw {
        "recv_time" => Ok(ReplayMode::RecvTime),
        "exchange_time" => Ok(ReplayMode::ExchangeTime),
        other => Err(ApiError::BadRequest(format!(
            "invalid replay mode: {other}"
        ))),
    }
}

fn map_replay_error(error: ReplayError) -> ApiError {
    match error {
        ReplayError::NoSnapshotFound {
            asset_id,
            timestamp_us,
        } => ApiError::NotFound(format!(
            "no snapshot found for asset {asset_id} before timestamp {timestamp_us}"
        )),
        other => ApiError::Internal(other.to_string()),
    }
}

fn continuity_warning(event: IngestEvent) -> ContinuityWarning {
    ContinuityWarning {
        kind: event.kind.to_string(),
        recv_timestamp_us: event.provenance.recv_timestamp_us,
        exchange_timestamp_us: event.provenance.exchange_timestamp_us,
        details: event.details,
    }
}

fn level_view(
    (price, size): (pb_types::FixedPrice, pb_types::FixedSize),
) -> crate::dto::PriceLevelView {
    crate::dto::PriceLevelView { price, size }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use object_store::ObjectStore;
    use pb_store::ParquetRecordWriter;
    use pb_types::event::{
        BookEvent, BookEventKind, DataSource, EventProvenance, PersistedRecord, Side,
    };
    use pb_types::{FixedPrice, FixedSize, Sequence};
    use tower::ServiceExt;

    use super::*;

    fn test_state(temp_path: String) -> AppState {
        AppState {
            live: LiveReadModel::new(crate::dto::FeedMode::FixedTokens),
            config: ApiConfig {
                parquet_base_path: temp_path,
                default_depth: 20,
                max_depth: 200,
                stale_after_secs: 60,
            },
            broadcast: None,
        }
    }

    async fn response_json<T: serde::de::DeserializeOwned>(
        response: axum::response::Response,
    ) -> T {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn orderbook_snapshot_returns_404_for_inactive_asset() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orderbooks/tok1/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn live_routes_report_active_assets_and_snapshots() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let state = test_state(tmp_dir.path().to_string_lossy().to_string());
        state.live.set_active_assets(vec!["tok1".to_string()]).await;
        let provenance = EventProvenance {
            recv_timestamp_us: 100,
            exchange_timestamp_us: 90,
            source: DataSource::WebSocket,
            source_event_id: Some("snap-1".to_string()),
            source_session_id: Some("ws-session-1".to_string()),
            sequence: Some(Sequence::new(0)),
        };
        state
            .live
            .apply_record(PersistedRecord::Book(BookEvent {
                asset_id: AssetId::new("tok1"),
                kind: BookEventKind::Snapshot,
                side: Side::Bid,
                price: FixedPrice::from_f64(0.50).unwrap(),
                size: FixedSize::from_f64(10.0).unwrap(),
                provenance: provenance.clone(),
            }))
            .await;
        state
            .live
            .apply_record(PersistedRecord::Book(BookEvent {
                asset_id: AssetId::new("tok1"),
                kind: BookEventKind::Snapshot,
                side: Side::Ask,
                price: FixedPrice::from_f64(0.60).unwrap(),
                size: FixedSize::from_f64(20.0).unwrap(),
                provenance: EventProvenance {
                    sequence: Some(Sequence::new(1)),
                    ..provenance
                },
            }))
            .await;
        state
            .live
            .apply_record(PersistedRecord::Ingest(pb_types::IngestEvent {
                asset_id: None,
                kind: pb_types::IngestEventKind::ReconnectSuccess,
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

        let app = router(state.clone());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/active")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let assets: Vec<crate::dto::ActiveAssetSummary> = response_json(response).await;
        assert_eq!(assets.len(), 1);
        assert!(assets[0].has_book);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orderbooks/tok1/snapshot?depth=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let snapshot: crate::dto::LiveOrderBookSnapshot = response_json(response).await;
        assert_eq!(snapshot.bid_depth, 1);
        assert_eq!(snapshot.ask_depth, 1);
        assert_eq!(snapshot.bids.len(), 1);
    }

    #[tokio::test]
    async fn replay_reconstruct_reads_from_parquet() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = ParquetRecordWriter::new(
            Arc::new(object_store::local::LocalFileSystem::new()) as Arc<dyn ObjectStore>,
            base_path.clone(),
        );
        let base_ts = 1_700_000_000_000_000u64;
        writer
            .write_batch(&[
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("tok1"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Bid,
                    price: FixedPrice::new(5000).unwrap(),
                    size: FixedSize::from_f64(100.0).unwrap(),
                    provenance: EventProvenance {
                        recv_timestamp_us: base_ts,
                        exchange_timestamp_us: base_ts,
                        source: DataSource::WebSocket,
                        source_event_id: Some("snap-1".to_string()),
                        source_session_id: Some("ws-session-1".to_string()),
                        sequence: Some(Sequence::new(0)),
                    },
                }),
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("tok1"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Ask,
                    price: FixedPrice::new(5500).unwrap(),
                    size: FixedSize::from_f64(110.0).unwrap(),
                    provenance: EventProvenance {
                        recv_timestamp_us: base_ts,
                        exchange_timestamp_us: base_ts,
                        source: DataSource::WebSocket,
                        source_event_id: Some("snap-1".to_string()),
                        source_session_id: Some("ws-session-1".to_string()),
                        sequence: Some(Sequence::new(1)),
                    },
                }),
            ])
            .await
            .unwrap();

        let app = router(test_state(base_path));
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/replay/reconstruct?asset_id=tok1&at_us={base_ts}&mode=recv_time"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let replay: crate::dto::ReplayReconstructionResponse = response_json(response).await;
        assert_eq!(replay.asset_id, "tok1");
        assert_eq!(replay.bids.len(), 1);
        assert_eq!(replay.asks.len(), 1);
    }

    fn parquet_writer(base_path: &str) -> ParquetRecordWriter {
        ParquetRecordWriter::new(
            Arc::new(object_store::local::LocalFileSystem::new()) as Arc<dyn ObjectStore>,
            base_path.to_string(),
        )
    }

    fn test_provenance(ts: u64, seq: u64) -> EventProvenance {
        EventProvenance {
            recv_timestamp_us: ts,
            exchange_timestamp_us: ts,
            source: DataSource::WebSocket,
            source_event_id: None,
            source_session_id: Some("ws-session-1".to_string()),
            sequence: Some(Sequence::new(seq)),
        }
    }

    #[tokio::test]
    async fn integrity_summary_returns_counts_from_parquet() {
        use pb_types::event::IngestEventKind;

        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        writer
            .write_batch(&[
                PersistedRecord::Book(BookEvent {
                    asset_id: AssetId::new("tok1"),
                    kind: BookEventKind::Snapshot,
                    side: Side::Bid,
                    price: FixedPrice::new(5000).unwrap(),
                    size: FixedSize::from_f64(100.0).unwrap(),
                    provenance: test_provenance(base_ts, 0),
                }),
                PersistedRecord::Ingest(pb_types::IngestEvent {
                    asset_id: Some(AssetId::new("tok1")),
                    kind: IngestEventKind::SequenceGap,
                    provenance: test_provenance(base_ts + 100, 0),
                    expected_sequence: Some(1),
                    observed_sequence: Some(3),
                    details: Some("gap".to_string()),
                }),
                PersistedRecord::Ingest(pb_types::IngestEvent {
                    asset_id: None,
                    kind: IngestEventKind::ReconnectStart,
                    provenance: test_provenance(base_ts + 200, 0),
                    expected_sequence: None,
                    observed_sequence: None,
                    details: None,
                }),
            ])
            .await
            .unwrap();

        let app = router(test_state(base_path));
        let end_ts = base_ts + 1_000_000;
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/integrity/summary?asset_id=tok1&start_us={base_ts}&end_us={end_ts}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let summary: crate::dto::IntegritySummaryResponse = response_json(response).await;
        assert_eq!(summary.asset_id, "tok1");
        assert_eq!(summary.total_book_events, 1);
        assert!(summary.total_ingest_events >= 1);
        assert!(summary.gap_count >= 1);
        assert_eq!(
            summary.completeness,
            crate::dto::CompletenessLabel::BestEffort
        );
    }

    #[tokio::test]
    async fn integrity_summary_returns_400_for_invalid_range() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/integrity/summary?asset_id=tok1&start_us=200&end_us=100")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn execution_orders_returns_timeline_from_parquet() {
        use pb_types::event::{ExecutionEvent, ExecutionEventKind, LatencyTrace};

        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        writer
            .write_batch(&[
                PersistedRecord::Execution(ExecutionEvent {
                    event_timestamp_us: base_ts,
                    asset_id: Some(AssetId::new("tok1")),
                    order_id: "order-1".to_string(),
                    client_order_id: Some("client-1".to_string()),
                    venue_order_id: None,
                    kind: ExecutionEventKind::SubmitIntent,
                    side: Some(Side::Bid),
                    price: Some(FixedPrice::new(5000).unwrap()),
                    size: Some(FixedSize::from_f64(10.0).unwrap()),
                    status: None,
                    reason: None,
                    latency: LatencyTrace::default(),
                }),
                PersistedRecord::Execution(ExecutionEvent {
                    event_timestamp_us: base_ts + 100,
                    asset_id: Some(AssetId::new("tok1")),
                    order_id: "order-1".to_string(),
                    client_order_id: Some("client-1".to_string()),
                    venue_order_id: Some("venue-1".to_string()),
                    kind: ExecutionEventKind::ExchangeAck,
                    side: Some(Side::Bid),
                    price: Some(FixedPrice::new(5000).unwrap()),
                    size: Some(FixedSize::from_f64(10.0).unwrap()),
                    status: Some("accepted".to_string()),
                    reason: None,
                    latency: LatencyTrace::default(),
                }),
            ])
            .await
            .unwrap();

        let app = router(test_state(base_path));
        let end_ts = base_ts + 1_000_000;
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/execution/orders?start_us={base_ts}&end_us={end_ts}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let timeline: crate::dto::ExecutionTimelineResponse = response_json(response).await;
        assert_eq!(timeline.total_count, 2);
        assert_eq!(timeline.events.len(), 2);
        assert_eq!(timeline.events[0].order_id, "order-1");
        assert_eq!(timeline.events[0].kind, "submit_intent");
        assert_eq!(timeline.events[1].kind, "exchange_ack");
    }

    #[tokio::test]
    async fn execution_orders_filters_by_order_id() {
        use pb_types::event::{ExecutionEvent, ExecutionEventKind, LatencyTrace};

        let tmp_dir = tempfile::tempdir().unwrap();
        let base_path = tmp_dir.path().to_string_lossy().to_string();
        let writer = parquet_writer(&base_path);
        let base_ts = 1_700_000_000_000_000u64;

        writer
            .write_batch(&[
                PersistedRecord::Execution(ExecutionEvent {
                    event_timestamp_us: base_ts,
                    asset_id: Some(AssetId::new("tok1")),
                    order_id: "order-A".to_string(),
                    client_order_id: None,
                    venue_order_id: None,
                    kind: ExecutionEventKind::SubmitIntent,
                    side: Some(Side::Bid),
                    price: None,
                    size: None,
                    status: None,
                    reason: None,
                    latency: LatencyTrace::default(),
                }),
                PersistedRecord::Execution(ExecutionEvent {
                    event_timestamp_us: base_ts + 50,
                    asset_id: Some(AssetId::new("tok1")),
                    order_id: "order-B".to_string(),
                    client_order_id: None,
                    venue_order_id: None,
                    kind: ExecutionEventKind::SubmitIntent,
                    side: Some(Side::Ask),
                    price: None,
                    size: None,
                    status: None,
                    reason: None,
                    latency: LatencyTrace::default(),
                }),
            ])
            .await
            .unwrap();

        let app = router(test_state(base_path));
        let end_ts = base_ts + 1_000_000;
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/execution/orders?order_id=order-A&start_us={base_ts}&end_us={end_ts}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let timeline: crate::dto::ExecutionTimelineResponse = response_json(response).await;
        assert_eq!(timeline.total_count, 1);
        assert_eq!(timeline.events[0].order_id, "order-A");
    }

    #[tokio::test]
    async fn execution_orders_returns_400_for_invalid_limit() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/execution/orders?start_us=100&end_us=200&limit=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn ws_orderbook_returns_404_for_inactive_asset() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut state = test_state(tmp_dir.path().to_string_lossy().to_string());
        state.broadcast = Some(crate::streaming::BookBroadcast::new());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let server_handle = tokio::spawn(async move {
            crate::serve(listener, state, shutdown_clone).await.unwrap();
        });

        let url = format!(
            "ws://127.0.0.1:{}/api/v1/streams/orderbook?asset_id=nope",
            addr.port()
        );
        let result = tokio_tungstenite::connect_async(&url).await;
        assert!(
            result.is_err() || {
                let (_, response) = result.unwrap();
                response.status() != hyper::StatusCode::SWITCHING_PROTOCOLS
            }
        );

        shutdown.cancel();
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn ws_orderbook_receives_initial_snapshot() {
        use futures_util::StreamExt;

        let tmp_dir = tempfile::tempdir().unwrap();
        let mut state = test_state(tmp_dir.path().to_string_lossy().to_string());
        let broadcast = crate::streaming::BookBroadcast::new();
        state.broadcast = Some(broadcast.clone());

        state.live.set_active_assets(vec!["tok1".to_string()]).await;
        state
            .live
            .apply_record(PersistedRecord::Book(BookEvent {
                asset_id: AssetId::new("tok1"),
                kind: BookEventKind::Snapshot,
                side: Side::Bid,
                price: FixedPrice::from_f64(0.50).unwrap(),
                size: FixedSize::from_f64(10.0).unwrap(),
                provenance: test_provenance(100, 0),
            }))
            .await;
        state
            .live
            .apply_record(PersistedRecord::Book(BookEvent {
                asset_id: AssetId::new("tok1"),
                kind: BookEventKind::Snapshot,
                side: Side::Ask,
                price: FixedPrice::from_f64(0.60).unwrap(),
                size: FixedSize::from_f64(20.0).unwrap(),
                provenance: test_provenance(100, 1),
            }))
            .await;
        state
            .live
            .apply_record(PersistedRecord::Ingest(pb_types::IngestEvent {
                asset_id: None,
                kind: pb_types::IngestEventKind::ReconnectSuccess,
                provenance: EventProvenance {
                    recv_timestamp_us: 200,
                    exchange_timestamp_us: 0,
                    source: DataSource::WebSocket,
                    source_event_id: None,
                    source_session_id: None,
                    sequence: None,
                },
                expected_sequence: None,
                observed_sequence: None,
                details: None,
            }))
            .await;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shutdown = CancellationToken::new();
        let shutdown_clone = shutdown.clone();
        let server_handle = tokio::spawn(async move {
            crate::serve(listener, state, shutdown_clone).await.unwrap();
        });

        let url = format!(
            "ws://127.0.0.1:{}/api/v1/streams/orderbook?asset_id=tok1",
            addr.port()
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
            .await
            .expect("timed out waiting for ws message")
            .expect("stream ended")
            .expect("ws error");

        let text = msg.into_text().unwrap();
        let update: crate::dto::BookUpdateMessage = serde_json::from_str(&text).unwrap();
        assert_eq!(update.asset_id, "tok1");
        assert!(!update.bids.is_empty());
        assert!(!update.asks.is_empty());

        let _ = ws.close(None).await;
        shutdown.cancel();
        let _ = server_handle.await;
    }
}
