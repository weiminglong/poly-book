use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{MatchedPath, Path, Query, State};
use axum::http::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use pb_service::{
    AnyExecutionService, AnyIntegrityService, AnyQueryService, AnyReplayService, ExecutionService,
    IntegrityService, QueryService, ReplayService,
};
use pb_types::{AssetId, ReplayMode};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::dto::{
    AssetRef, AssetResolveResponse, CompletenessLabel, ContinuityWarning, DatasetInfo,
    DatasetSchemaResponse, ExecutionEventView, ExecutionTimelineResponse, FeedStatusResponse,
    IntegritySummaryResponse, LatencyTraceView, LiveOrderBookSnapshot, QueryColumn,
    QueryResultResponse, ReplayReconstructionResponse,
};
use crate::error::ApiError;
use crate::live_state::{LiveReadModel, SnapshotLookupError};

#[derive(Debug, Clone)]
pub struct ApiConfig {
    pub parquet_base_path: String,
    pub default_depth: usize,
    pub max_depth: usize,
    pub stale_after_secs: u64,
    pub query_max_rows: usize,
    pub query_timeout_secs: u64,
}

#[derive(Clone)]
pub struct AppState {
    pub live: LiveReadModel,
    pub config: ApiConfig,
    pub broadcast: Option<crate::streaming::BookBroadcast>,
    pub slug_registry: pb_types::SlugRegistry,
    pub replay_service: AnyReplayService,
    pub integrity_service: AnyIntegrityService,
    pub execution_service: AnyExecutionService,
    pub query_service: Option<AnyQueryService>,
    /// WAL consumer lag in bytes, updated by the WAL tailer.
    pub wal_lag_bytes: Arc<AtomicU64>,
    /// Whether the WAL reader detected a segment gap and needs re-hydration.
    pub needs_resync: Arc<AtomicBool>,
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

#[derive(Debug, Deserialize)]
struct ResolveQuery {
    q: String,
}

#[derive(Debug, Deserialize)]
struct QuerySqlRequest {
    sql: String,
    max_rows: Option<usize>,
}

/// Resolve a slug or token ID to the canonical token ID string.
/// If the input matches a registered slug, returns the mapped token ID.
/// Otherwise, passes the input through unchanged (it may be a raw token ID).
fn resolve_asset_id(state: &AppState, input: &str) -> String {
    match state.slug_registry.resolve(input) {
        Some(id) => {
            let resolved = id.to_string();
            if resolved != input {
                tracing::debug!(slug = input, asset_id = %resolved, "resolved slug");
            }
            resolved
        }
        None => input.to_string(),
    }
}

pub fn router(state: AppState) -> Router {
    use axum::routing::any;

    Router::new()
        .route("/health", get(health))
        .route("/api/v1/feed/status", get(feed_status))
        .route("/api/v1/assets/active", get(active_assets))
        .route(
            "/api/v1/orderbooks/{asset_id}/snapshot",
            get(orderbook_snapshot),
        )
        .route("/api/v1/replay/reconstruct", get(replay_reconstruct))
        .route("/api/v1/assets/resolve", get(asset_resolve))
        .route("/api/v1/integrity/summary", get(integrity_summary))
        .route("/api/v1/execution/orders", get(execution_orders))
        .route("/api/v1/query/datasets", get(query_datasets))
        .route("/api/v1/query/sql", post(query_sql))
        .route(
            "/api/v1/streams/orderbook",
            any(crate::streaming::ws_orderbook),
        )
        .layer(middleware::from_fn(track_request_metrics))
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

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let hydrated = state.live.is_hydrated();
    let wal_lag = state.wal_lag_bytes.load(Ordering::Relaxed);
    let needs_resync = state.needs_resync.load(Ordering::Relaxed);
    Json(serde_json::json!({
        "ready": hydrated && !needs_resync,
        "hydrated": hydrated,
        "wal_lag_bytes": wal_lag,
        "needs_resync": needs_resync,
    }))
}

async fn feed_status(State(state): State<AppState>) -> Json<FeedStatusResponse> {
    let mut status = state.live.feed_status_raw().await;
    status.active_assets = status
        .active_assets
        .into_iter()
        .map(|asset_ref| AssetRef {
            slug: state.slug_registry.slug_for_str(&asset_ref.asset_id),
            ..asset_ref
        })
        .collect();
    Json(status)
}

async fn active_assets(State(state): State<AppState>) -> Json<Vec<crate::dto::ActiveAssetSummary>> {
    let mut assets = state
        .live
        .active_assets(state.config.stale_after_secs)
        .await;
    for asset in &mut assets {
        asset.slug = state.slug_registry.slug_for_str(&asset.asset_id);
        asset.label = state.slug_registry.label_for_str(&asset.asset_id);
    }
    Json(assets)
}

async fn orderbook_snapshot(
    State(state): State<AppState>,
    Path(raw_asset_id): Path<String>,
    Query(query): Query<DepthQuery>,
) -> Result<Json<LiveOrderBookSnapshot>, ApiError> {
    let asset_id = resolve_asset_id(&state, &raw_asset_id);
    let depth = validate_depth(
        query.depth.unwrap_or(state.config.default_depth),
        state.config.max_depth,
    )?;
    match state
        .live
        .snapshot(&asset_id, depth, state.config.stale_after_secs)
        .await
    {
        Ok(mut snapshot) => {
            snapshot.slug = state.slug_registry.slug_for_str(&asset_id);
            Ok(Json(snapshot))
        }
        Err(SnapshotLookupError::AssetNotActive) => Err(ApiError::NotFound(format!(
            "asset not active: {raw_asset_id}"
        ))),
        Err(SnapshotLookupError::SnapshotNotReady) => Err(ApiError::ServiceUnavailable(format!(
            "snapshot not ready for asset: {raw_asset_id}"
        ))),
    }
}

async fn replay_reconstruct(
    State(state): State<AppState>,
    Query(query): Query<ReplayQuery>,
) -> Result<Json<ReplayReconstructionResponse>, ApiError> {
    let asset_id_str = resolve_asset_id(&state, &query.asset_id);
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
    let asset_id = AssetId::new(asset_id_str.clone());
    let result = state
        .replay_service
        .reconstruct(&asset_id, query.at_us, mode, Some(depth))
        .await?;

    Ok(Json(ReplayReconstructionResponse {
        asset_id: asset_id_str.clone(),
        slug: state.slug_registry.slug_for_str(&asset_id_str),
        mode: result.mode.to_string(),
        used_checkpoint: result.used_checkpoint,
        sequence: result.sequence,
        last_update_us: result.timestamp_us,
        best_bid: result.best_bid.map(level_view),
        best_ask: result.best_ask.map(level_view),
        mid_price: result.mid_price,
        spread: result.spread,
        bid_depth: result.bid_depth,
        ask_depth: result.ask_depth,
        bids: result.bids.into_iter().map(level_view).collect(),
        asks: result.asks.into_iter().map(level_view).collect(),
        continuity_events: result
            .continuity_events
            .into_iter()
            .map(service_continuity_warning)
            .collect(),
    }))
}

async fn asset_resolve(
    State(state): State<AppState>,
    Query(query): Query<ResolveQuery>,
) -> Json<AssetResolveResponse> {
    match state.slug_registry.resolve(&query.q) {
        Some(asset_id) => {
            let asset_id_str = asset_id.to_string();
            Json(AssetResolveResponse {
                found: true,
                slug: state.slug_registry.slug_for_str(&asset_id_str),
                asset_id: Some(asset_id_str),
            })
        }
        None => Json(AssetResolveResponse {
            found: false,
            asset_id: None,
            slug: None,
        }),
    }
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
    let asset_id_str = resolve_asset_id(&state, &query.asset_id);
    validate_time_window(query.start_us, query.end_us)?;
    let asset_id = AssetId::new(asset_id_str.clone());
    let summary = state
        .integrity_service
        .summary(&asset_id, query.start_us, query.end_us)
        .await?;

    let validation_count = summary.validation_count as u32;
    let validations_matched = summary.validation_match_count as u32;
    let completeness = match summary.completeness {
        pb_service::CompletenessLevel::Full => CompletenessLabel::Complete,
        _ => CompletenessLabel::BestEffort,
    };

    Ok(Json(IntegritySummaryResponse {
        asset_id: asset_id_str.clone(),
        slug: state.slug_registry.slug_for_str(&asset_id_str),
        start_us: summary.start_us,
        end_us: summary.end_us,
        total_book_events: summary.book_event_count as u64,
        total_ingest_events: summary.ingest_event_count as u64,
        reconnect_count: summary.reconnect_count as u32,
        gap_count: summary.gap_count as u32,
        stale_snapshot_skip_count: summary.stale_snapshot_skip_count as u32,
        validation_count,
        validations_matched,
        validations_mismatched: validation_count - validations_matched,
        completeness,
        continuity_events: summary
            .continuity_events
            .into_iter()
            .map(service_continuity_warning)
            .collect(),
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

    let asset_id = query
        .asset_id
        .as_ref()
        .map(|raw| resolve_asset_id(&state, raw))
        .map(|s| AssetId::new(s.as_str()));
    let timeline = state
        .execution_service
        .timeline(
            asset_id.as_ref(),
            query.order_id.as_deref(),
            query.start_us,
            query.end_us,
            limit,
        )
        .await?;

    let views: Vec<ExecutionEventView> = timeline
        .events
        .into_iter()
        .map(execution_event_view)
        .collect();

    Ok(Json(ExecutionTimelineResponse {
        events: views,
        total_count: timeline.total_count as u64,
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

async fn query_datasets(
    State(state): State<AppState>,
) -> Result<Json<DatasetSchemaResponse>, ApiError> {
    let service = state.query_service.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("query workbench is not enabled".to_string())
    })?;
    let datasets = service.list_datasets().await?;
    Ok(Json(DatasetSchemaResponse {
        datasets: datasets
            .into_iter()
            .map(|d| DatasetInfo {
                name: d.name,
                description: d.description,
                columns: d
                    .columns
                    .into_iter()
                    .map(|c| QueryColumn {
                        name: c.name,
                        data_type: c.data_type,
                    })
                    .collect(),
            })
            .collect(),
    }))
}

async fn query_sql(
    State(state): State<AppState>,
    Json(req): Json<QuerySqlRequest>,
) -> Result<Json<QueryResultResponse>, ApiError> {
    let service = state.query_service.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("query workbench is not enabled".to_string())
    })?;

    let guard = pb_service::QueryGuard {
        max_rows: req.max_rows.unwrap_or(state.config.query_max_rows),
        timeout_secs: state.config.query_timeout_secs,
    };

    let result = service.execute_sql(&req.sql, &guard).await?;

    Ok(Json(QueryResultResponse {
        columns: result
            .columns
            .into_iter()
            .map(|c| QueryColumn {
                name: c.name,
                data_type: c.data_type,
            })
            .collect(),
        rows: result.rows,
        row_count: result.row_count as u64,
        truncated: result.truncated,
        execution_time_ms: result.execution_time_ms,
    }))
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

fn service_continuity_warning(event: pb_service::ContinuityEvent) -> ContinuityWarning {
    ContinuityWarning {
        kind: event.kind,
        recv_timestamp_us: event.recv_timestamp_us,
        exchange_timestamp_us: event.exchange_timestamp_us,
        details: event.details,
    }
}

fn level_view(
    (price, size): (pb_types::FixedPrice, pb_types::FixedSize),
) -> crate::dto::PriceLevelView {
    crate::dto::PriceLevelView { price, size }
}

async fn track_request_metrics(req: Request<axum::body::Body>, next: Next) -> Response {
    let method = req.method().clone();
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("<unmatched>")
        .to_string();
    let start = Instant::now();
    let response = next.run(req).await;

    pb_metrics::record_api_request_duration_ms(
        method.as_str(),
        &route,
        response.status().as_u16(),
        start.elapsed().as_secs_f64() * 1_000.0,
    );

    response
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

    async fn test_state(temp_path: String) -> AppState {
        let live = LiveReadModel::new(crate::dto::FeedMode::FixedTokens);
        live.mark_hydrated().await;
        AppState {
            live,
            config: ApiConfig {
                parquet_base_path: temp_path.clone(),
                default_depth: 20,
                max_depth: 200,
                stale_after_secs: 60,
                query_max_rows: 10_000,
                query_timeout_secs: 30,
            },
            broadcast: None,
            slug_registry: pb_types::SlugRegistry::new(),
            replay_service: AnyReplayService::Parquet(pb_service::ParquetReplayService::new(
                &temp_path,
            )),
            integrity_service: AnyIntegrityService::Parquet(
                pb_service::ParquetIntegrityService::new(&temp_path),
            ),
            execution_service: AnyExecutionService::Parquet(
                pb_service::ParquetExecutionService::new(&temp_path),
            ),
            query_service: None,
            wal_lag_bytes: Arc::new(AtomicU64::new(0)),
            needs_resync: Arc::new(AtomicBool::new(false)),
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
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()).await);

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
    async fn orderbook_snapshot_returns_503_until_snapshot_group_materializes() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;
        state.live.set_active_assets(vec!["tok1".to_string()]).await;
        state
            .live
            .apply_record(PersistedRecord::Book(BookEvent {
                asset_id: AssetId::new("tok1"),
                kind: BookEventKind::Snapshot,
                side: Side::Bid,
                price: FixedPrice::from_f64(0.50).unwrap(),
                size: FixedSize::from_f64(10.0).unwrap(),
                provenance: EventProvenance {
                    recv_timestamp_us: 100,
                    exchange_timestamp_us: 90,
                    source: DataSource::WebSocket,
                    source_event_id: Some("snap-1".to_string()),
                    source_session_id: Some("ws-session-1".to_string()),
                    sequence: Some(Sequence::new(0)),
                },
            }))
            .await;

        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orderbooks/tok1/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn live_routes_report_active_assets_and_snapshots() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;
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

        let app = router(test_state(base_path).await);
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

        let app = router(test_state(base_path).await);
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
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()).await);

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

        let app = router(test_state(base_path).await);
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

        let app = router(test_state(base_path).await);
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
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()).await);

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
        let mut state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;
        let broadcast = crate::streaming::PerAssetBroadcast::new();
        broadcast.set_active_assets(&[]);
        state.broadcast = Some(broadcast);

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
        let mut state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;
        let broadcast = crate::streaming::PerAssetBroadcast::new();
        broadcast.set_active_assets(&["tok1".to_string()]);
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

    #[tokio::test]
    async fn orderbook_snapshot_resolves_slug() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;
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
                    recv_timestamp_us: 101,
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

        // Register a slug for tok1
        state
            .slug_registry
            .register("my-market-yes", &AssetId::new("tok1"));

        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/orderbooks/my-market-yes/snapshot?depth=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let snapshot: crate::dto::LiveOrderBookSnapshot = response_json(response).await;
        assert_eq!(snapshot.asset_id, "tok1");
        assert_eq!(snapshot.slug.as_deref(), Some("my-market-yes"));
    }

    #[tokio::test]
    async fn resolve_endpoint_finds_registered_slug() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;
        state
            .slug_registry
            .register("btc-up-5m", &AssetId::new("tok42"));

        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/resolve?q=btc-up-5m")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let result: crate::dto::AssetResolveResponse = response_json(response).await;
        assert!(result.found);
        assert_eq!(result.asset_id.as_deref(), Some("tok42"));
        assert_eq!(result.slug.as_deref(), Some("btc-up-5m"));
    }

    #[tokio::test]
    async fn resolve_endpoint_returns_not_found_for_unknown() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;

        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/resolve?q=nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let result: crate::dto::AssetResolveResponse = response_json(response).await;
        assert!(!result.found);
        assert!(result.asset_id.is_none());
    }

    #[tokio::test]
    async fn health_endpoint_returns_status() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;

        let app = router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response_json(response).await;
        assert_eq!(body["ready"], true);
        assert_eq!(body["hydrated"], true);
        assert_eq!(body["wal_lag_bytes"], 0);
        assert_eq!(body["needs_resync"], false);
    }

    #[tokio::test]
    async fn feed_status_returns_200_with_valid_response() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;
        state.live.set_active_assets(vec!["tok1".to_string()]).await;

        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/feed/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let status: crate::dto::FeedStatusResponse = response_json(response).await;
        assert_eq!(status.active_asset_count, 1);
        assert!(!status.active_assets.is_empty());
    }

    #[tokio::test]
    async fn replay_returns_400_for_invalid_mode() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()).await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/replay/reconstruct?asset_id=tok1&at_us=100&mode=bogus")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn replay_returns_400_for_zero_depth() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()).await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(
                        "/api/v1/replay/reconstruct?asset_id=tok1&at_us=100&mode=recv_time&depth=0",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn query_sql_returns_503_when_disabled() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()).await);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/query/sql")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"sql":"SELECT 1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn query_datasets_returns_503_when_disabled() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let app = router(test_state(tmp_dir.path().to_string_lossy().to_string()).await);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/query/datasets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn health_reports_not_ready_when_resync_needed() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let state = test_state(tmp_dir.path().to_string_lossy().to_string()).await;
        state
            .needs_resync
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = response_json(response).await;
        assert_eq!(body["ready"], false);
        assert_eq!(body["needs_resync"], true);
    }
}
