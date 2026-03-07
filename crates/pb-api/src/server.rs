use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use pb_replay::{ParquetReader, ReplayEngine, ReplayError};
use pb_types::{AssetId, IngestEvent, ReplayMode};
use serde::Deserialize;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::dto::{
    ContinuityWarning, FeedStatusResponse, LiveOrderBookSnapshot, ReplayReconstructionResponse,
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

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/feed/status", get(feed_status))
        .route("/api/v1/assets/active", get(active_assets))
        .route(
            "/api/v1/orderbooks/{asset_id}/snapshot",
            get(orderbook_snapshot),
        )
        .route("/api/v1/replay/reconstruct", get(replay_reconstruct))
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
}
