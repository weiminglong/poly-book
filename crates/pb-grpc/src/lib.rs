//! gRPC read surface for the poly-book workstation.

use std::net::SocketAddr;

use tokio_util::sync::CancellationToken;
use tonic::{Request, Response, Status};
use tracing::info;

/// Generated protobuf types and service traits.
pub mod proto {
    tonic::include_proto!("pb.workstation.v1");
}

use pb_service::{
    AnyExecutionService, AnyIntegrityService, AnyReplayService, CompletenessLevel,
    ExecutionService, IntegrityService, ReplayService, ServiceError,
};
use pb_types::event::ReplayMode;
use proto::workstation_service_server::{WorkstationService, WorkstationServiceServer};

/// Maximum gRPC message size (encode + decode). Bounds response serialization so
/// a wide query cannot OOM the serve process (HFT-review finding); large enough
/// for legitimate reconstruct/timeline responses.
const MAX_GRPC_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn service_error_to_status(err: ServiceError) -> Status {
    match err {
        ServiceError::NotFound(msg) => Status::not_found(msg),
        ServiceError::InvalidParams(msg) => Status::invalid_argument(msg),
        ServiceError::Unavailable(msg) => Status::unavailable(msg),
        ServiceError::Internal(msg) => Status::internal(msg),
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn price_level_to_proto(
    price: pb_types::FixedPrice,
    size: pb_types::FixedSize,
) -> proto::PriceLevel {
    proto::PriceLevel {
        price: price.to_string(),
        size: size.to_string(),
    }
}

fn continuity_to_proto(event: &pb_service::ContinuityEvent) -> proto::ContinuityEvent {
    proto::ContinuityEvent {
        kind: event.kind.clone(),
        recv_timestamp_us: event.recv_timestamp_us,
        exchange_timestamp_us: event.exchange_timestamp_us,
        details: event.details.clone(),
    }
}

/// Map the internal completeness level to the SAME two-value domain the HTTP API
/// exposes (`complete` / `best_effort`, see pb-api `CompletenessLabel`). The gRPC
/// surface previously emitted a divergent four-value domain
/// (full/partial/sparse/empty), so a client could not treat the two surfaces
/// interchangeably (HFT-review #15).
fn completeness_to_str(level: CompletenessLevel) -> &'static str {
    match level {
        CompletenessLevel::Full => "complete",
        _ => "best_effort",
    }
}

fn parse_replay_mode(s: &str) -> Result<ReplayMode, Status> {
    match s {
        "recv_time" => Ok(ReplayMode::RecvTime),
        "exchange_time" => Ok(ReplayMode::ExchangeTime),
        other => Err(Status::invalid_argument(format!(
            "unknown replay mode: {other}, expected \"recv_time\" or \"exchange_time\""
        ))),
    }
}

// ---------------------------------------------------------------------------
// Service implementation
// ---------------------------------------------------------------------------

/// gRPC server backed by the pb-service trait implementations.
pub struct GrpcWorkstationService {
    replay: AnyReplayService,
    integrity: AnyIntegrityService,
    execution: AnyExecutionService,
    /// Upper bound on requested reconstruct depth, mirroring the HTTP API's
    /// `api.max_depth` so both surfaces reject the same oversized requests.
    max_depth: usize,
}

impl GrpcWorkstationService {
    pub fn new(
        replay: AnyReplayService,
        integrity: AnyIntegrityService,
        execution: AnyExecutionService,
        max_depth: usize,
    ) -> Self {
        Self {
            replay,
            integrity,
            execution,
            max_depth,
        }
    }
}

#[tonic::async_trait]
impl WorkstationService for GrpcWorkstationService {
    async fn reconstruct(
        &self,
        request: Request<proto::ReconstructRequest>,
    ) -> Result<Response<proto::ReconstructResponse>, Status> {
        let req = request.into_inner();
        let mode = parse_replay_mode(&req.mode)?;
        let asset_id = pb_types::AssetId::new(&*req.asset_id);
        // Mirror the HTTP guard: an explicit depth of 0 is invalid (it would
        // request zero levels). None means "service default" (HFT-review #24).
        let depth = match req.depth {
            Some(0) => {
                return Err(Status::invalid_argument("depth must be greater than zero"));
            }
            Some(d) => {
                // Mirror the HTTP guard's upper bound (api.max_depth): without it the
                // gRPC surface would return more levels than HTTP allows for the same
                // query. The result is book-bounded and the response is size-capped, so
                // this is a contract-consistency guard rather than a DoS fix.
                let d = d as usize;
                if d > self.max_depth {
                    return Err(Status::invalid_argument(format!(
                        "depth {d} exceeds max_depth {}",
                        self.max_depth
                    )));
                }
                Some(d)
            }
            None => None,
        };

        let result = self
            .replay
            .reconstruct(&asset_id, req.at_us, mode, depth)
            .await
            .map_err(service_error_to_status)?;

        let resp = proto::ReconstructResponse {
            asset_id: result.asset_id,
            timestamp_us: result.timestamp_us,
            mode: result.mode.to_string(),
            sequence: result.sequence,
            best_bid: result.best_bid.map(|(p, s)| price_level_to_proto(p, s)),
            best_ask: result.best_ask.map(|(p, s)| price_level_to_proto(p, s)),
            mid_price: result.mid_price,
            spread: result.spread,
            // Saturate rather than silently truncate usize->u32 (HFT-review #26);
            // a depth above u32::MAX is absurd but truncation would corrupt it.
            bid_depth: u32::try_from(result.bid_depth).unwrap_or(u32::MAX),
            ask_depth: u32::try_from(result.ask_depth).unwrap_or(u32::MAX),
            bids: result
                .bids
                .iter()
                .map(|(p, s)| price_level_to_proto(*p, *s))
                .collect(),
            asks: result
                .asks
                .iter()
                .map(|(p, s)| price_level_to_proto(*p, *s))
                .collect(),
            used_checkpoint: result.used_checkpoint,
            continuity_events: result
                .continuity_events
                .iter()
                .map(continuity_to_proto)
                .collect(),
        };

        Ok(Response::new(resp))
    }

    async fn integrity_summary(
        &self,
        request: Request<proto::IntegritySummaryRequest>,
    ) -> Result<Response<proto::IntegritySummaryResponse>, Status> {
        let req = request.into_inner();
        let asset_id = pb_types::AssetId::new(&*req.asset_id);

        let summary = self
            .integrity
            .summary(&asset_id, req.start_us, req.end_us)
            .await
            .map_err(service_error_to_status)?;

        let resp = proto::IntegritySummaryResponse {
            asset_id: summary.asset_id,
            start_us: summary.start_us,
            end_us: summary.end_us,
            book_event_count: summary.book_event_count as u64,
            trade_event_count: summary.trade_event_count as u64,
            ingest_event_count: summary.ingest_event_count as u64,
            checkpoint_count: summary.checkpoint_count as u32,
            reconnect_count: summary.reconnect_count as u32,
            gap_count: summary.gap_count as u32,
            stale_snapshot_skip_count: summary.stale_snapshot_skip_count as u32,
            validation_count: summary.validation_count as u32,
            validation_match_count: summary.validation_match_count as u32,
            completeness: completeness_to_str(summary.completeness).into(),
            continuity_events: summary
                .continuity_events
                .iter()
                .map(continuity_to_proto)
                .collect(),
        };

        Ok(Response::new(resp))
    }

    async fn execution_timeline(
        &self,
        request: Request<proto::ExecutionTimelineRequest>,
    ) -> Result<Response<proto::ExecutionTimelineResponse>, Status> {
        let req = request.into_inner();
        let asset_id = req.asset_id.as_deref().map(pb_types::AssetId::new);
        let order_id = req.order_id.as_deref();

        let timeline = self
            .execution
            .timeline(
                asset_id.as_ref(),
                order_id,
                req.start_us,
                req.end_us,
                req.limit as usize,
                req.offset as usize,
                req.descending,
            )
            .await
            .map_err(service_error_to_status)?;

        let events = timeline
            .events
            .iter()
            .map(|e| proto::ExecutionEvent {
                event_timestamp_us: e.event_timestamp_us,
                asset_id: e.asset_id.as_ref().map(|id| id.to_string()),
                order_id: e.order_id.clone(),
                client_order_id: e.client_order_id.clone(),
                venue_order_id: e.venue_order_id.clone(),
                kind: e.kind.to_string(),
                side: e.side.map(|s| s.to_string()),
                price: e.price.map(|p| p.to_string()),
                size: e.size.map(|s| s.to_string()),
                status: e.status.clone(),
                reason: e.reason.clone(),
                latency: Some(proto::LatencyTrace {
                    market_data_recv_us: e.latency.market_data_recv_us,
                    normalization_done_us: e.latency.normalization_done_us,
                    strategy_decision_us: e.latency.strategy_decision_us,
                    order_submit_us: e.latency.order_submit_us,
                    exchange_ack_us: e.latency.exchange_ack_us,
                    exchange_fill_us: e.latency.exchange_fill_us,
                }),
            })
            .collect();

        let resp = proto::ExecutionTimelineResponse {
            events,
            total_count: timeline.total_count as u64,
        };

        Ok(Response::new(resp))
    }
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

/// Start the gRPC server. Returns a `JoinHandle` for the spawned task.
pub async fn start_grpc_server(
    addr: SocketAddr,
    replay: AnyReplayService,
    integrity: AnyIntegrityService,
    execution: AnyExecutionService,
    max_depth: usize,
    shutdown: CancellationToken,
) -> Result<tokio::task::JoinHandle<()>, Box<dyn std::error::Error + Send + Sync>> {
    let service = GrpcWorkstationService::new(replay, integrity, execution, max_depth);
    // Bound the response encode size. The default permits multi-GB messages, so a
    // wide reconstruct/timeline query against a busy asset could try to serialize
    // an enormous response and OOM the serve process (HFT-review finding). 16 MiB
    // comfortably holds legitimate responses while capping a runaway one; it pairs
    // with the per-read LIMITs pushed into the ClickHouse reader.
    let workstation = WorkstationServiceServer::new(service)
        .max_encoding_message_size(MAX_GRPC_MESSAGE_BYTES)
        .max_decoding_message_size(MAX_GRPC_MESSAGE_BYTES);
    let server = tonic::transport::Server::builder().add_service(workstation);

    // Bind up front so a bind failure (e.g. port in use) is returned to the
    // caller instead of being swallowed inside the spawned task while we log
    // "bound" and let `serve` run on silently without gRPC (audit finding A.112).
    let incoming = tonic::transport::server::TcpIncoming::bind(addr)?;
    info!(%addr, "gRPC server bound");

    let handle = tokio::spawn(async move {
        let shutdown_signal = shutdown.cancelled_owned();
        if let Err(e) = server
            .serve_with_incoming_shutdown(incoming, shutdown_signal)
            .await
        {
            tracing::error!(error = %e, "gRPC server error");
        }
    });

    Ok(handle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use proto::workstation_service_client::WorkstationServiceClient;

    use std::sync::Arc;

    /// Build Parquet-backed services rooted in a temp directory.
    fn build_test_services(
        base_path: &str,
    ) -> (AnyReplayService, AnyIntegrityService, AnyExecutionService) {
        (
            AnyReplayService::Parquet(pb_service::ParquetReplayService::new(base_path)),
            AnyIntegrityService::Parquet(pb_service::ParquetIntegrityService::new(base_path)),
            AnyExecutionService::Parquet(pb_service::ParquetExecutionService::new(base_path)),
        )
    }

    /// Write test book-event Parquet data so the service has something to query.
    async fn write_test_data(base_path: &str) {
        use pb_store::ParquetSink;
        use pb_types::event::*;
        use pb_types::{AssetId, FixedPrice, FixedSize, PersistedRecord};
        use tokio::sync::mpsc;

        let (tx, rx) = mpsc::channel::<PersistedRecord>(64);
        let store: Arc<dyn object_store::ObjectStore> =
            Arc::new(object_store::local::LocalFileSystem::new());
        let sink = ParquetSink::new(rx, store, base_path.to_string())
            .with_flush_interval(std::time::Duration::from_millis(50));

        let asset_id = AssetId::new("test-asset");
        let base_ts: u64 = 1_700_000_000_000_000;

        // Snapshot event.
        tx.send(PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Bid,
            price: FixedPrice::from_f64(0.55).unwrap(),
            size: FixedSize::from_f64(100.0).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: base_ts,
                exchange_timestamp_us: base_ts - 1000,
                source: DataSource::WebSocket,
                source_event_id: Some("ev-1".into()),
                source_session_id: Some("ses-1".into()),
                sequence: Some(pb_types::Sequence::new(1)),
                ingest_ordinal: None,
            },
        }))
        .await
        .unwrap();

        tx.send(PersistedRecord::Book(BookEvent {
            asset_id: asset_id.clone(),
            kind: BookEventKind::Snapshot,
            side: Side::Ask,
            price: FixedPrice::from_f64(0.60).unwrap(),
            size: FixedSize::from_f64(50.0).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: base_ts,
                exchange_timestamp_us: base_ts - 1000,
                source: DataSource::WebSocket,
                source_event_id: Some("ev-1".into()),
                source_session_id: Some("ses-1".into()),
                sequence: Some(pb_types::Sequence::new(1)),
                ingest_ordinal: None,
            },
        }))
        .await
        .unwrap();

        drop(tx);
        sink.run().await.unwrap();
    }

    /// Find an available port by binding to 0.
    async fn free_port() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn test_reconstruct_rpc() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();
        write_test_data(base).await;

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        // Give the server a moment to bind.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let resp = client
            .reconstruct(proto::ReconstructRequest {
                asset_id: "test-asset".into(),
                at_us: 1_700_000_000_000_000 + 1,
                mode: "recv_time".into(),
                depth: None,
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.asset_id, "test-asset");
        assert!(resp.best_bid.is_some());
        assert!(resp.best_ask.is_some());

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_integrity_summary_rpc() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();
        write_test_data(base).await;

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let resp = client
            .integrity_summary(proto::IntegritySummaryRequest {
                asset_id: "test-asset".into(),
                start_us: 1_700_000_000_000_000 - 1,
                end_us: 1_700_000_000_000_000 + 1_000_000,
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.asset_id, "test-asset");
        assert!(resp.book_event_count >= 2);

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_execution_timeline_rpc() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let resp = client
            .execution_timeline(proto::ExecutionTimelineRequest {
                asset_id: None,
                order_id: None,
                start_us: 1,
                end_us: 86_400_000_001, // exactly the 24h max window
                limit: 10,
                offset: 0,
                descending: false,
            })
            .await
            .unwrap()
            .into_inner();

        // No execution data written — expect empty result.
        assert_eq!(resp.total_count, 0);
        assert!(resp.events.is_empty());

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_invalid_replay_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let result = client
            .reconstruct(proto::ReconstructRequest {
                asset_id: "test-asset".into(),
                at_us: 1_700_000_000_000_000,
                mode: "invalid_mode".into(),
                depth: None,
            })
            .await;

        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    // -----------------------------------------------------------------------
    // Unit tests for conversion helpers (no server needed)
    // -----------------------------------------------------------------------

    #[test]
    fn completeness_to_str_matches_http_two_value_domain() {
        // Must mirror the HTTP CompletenessLabel domain (complete / best_effort)
        // so the two surfaces are interchangeable (HFT-review #15).
        assert_eq!(completeness_to_str(CompletenessLevel::Full), "complete");
        assert_eq!(
            completeness_to_str(CompletenessLevel::Partial),
            "best_effort"
        );
        assert_eq!(
            completeness_to_str(CompletenessLevel::Sparse),
            "best_effort"
        );
        assert_eq!(completeness_to_str(CompletenessLevel::Empty), "best_effort");
    }

    #[test]
    fn parse_replay_mode_valid() {
        assert_eq!(
            parse_replay_mode("recv_time").unwrap(),
            ReplayMode::RecvTime
        );
        assert_eq!(
            parse_replay_mode("exchange_time").unwrap(),
            ReplayMode::ExchangeTime
        );
    }

    #[test]
    fn parse_replay_mode_invalid() {
        let err = parse_replay_mode("bad").unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("bad"));
    }

    #[test]
    fn parse_replay_mode_empty() {
        let err = parse_replay_mode("").unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn price_level_to_proto_formats_correctly() {
        let price = pb_types::FixedPrice::from_f64(0.55).unwrap();
        let size = pb_types::FixedSize::from_f64(100.0).unwrap();
        let level = price_level_to_proto(price, size);
        assert_eq!(level.price, price.to_string());
        assert_eq!(level.size, size.to_string());
    }

    #[test]
    fn continuity_to_proto_maps_all_fields() {
        let event = pb_service::ContinuityEvent {
            kind: "sequence_gap".to_string(),
            recv_timestamp_us: 1000,
            exchange_timestamp_us: 900,
            details: Some("gap detail".to_string()),
        };
        let proto = continuity_to_proto(&event);
        assert_eq!(proto.kind, "sequence_gap");
        assert_eq!(proto.recv_timestamp_us, 1000);
        assert_eq!(proto.exchange_timestamp_us, 900);
        assert_eq!(proto.details.as_deref(), Some("gap detail"));
    }

    #[test]
    fn continuity_to_proto_with_none_details() {
        let event = pb_service::ContinuityEvent {
            kind: "reconnect_start".to_string(),
            recv_timestamp_us: 500,
            exchange_timestamp_us: 400,
            details: None,
        };
        let proto = continuity_to_proto(&event);
        assert!(proto.details.is_none());
    }

    #[test]
    fn service_error_to_status_not_found() {
        let err = ServiceError::NotFound("thing missing".into());
        let status = service_error_to_status(err);
        assert_eq!(status.code(), tonic::Code::NotFound);
        assert!(status.message().contains("thing missing"));
    }

    #[test]
    fn service_error_to_status_invalid_argument() {
        let err = ServiceError::InvalidParams("bad param".into());
        let status = service_error_to_status(err);
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn service_error_to_status_unavailable() {
        let err = ServiceError::Unavailable("down".into());
        let status = service_error_to_status(err);
        assert_eq!(status.code(), tonic::Code::Unavailable);
    }

    #[test]
    fn service_error_to_status_internal() {
        let err = ServiceError::Internal("oops".into());
        let status = service_error_to_status(err);
        assert_eq!(status.code(), tonic::Code::Internal);
    }

    // -----------------------------------------------------------------------
    // Additional RPC tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_reconstruct_with_depth_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();
        write_test_data(base).await;

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let resp = client
            .reconstruct(proto::ReconstructRequest {
                asset_id: "test-asset".into(),
                at_us: 1_700_000_000_000_000 + 1,
                mode: "recv_time".into(),
                depth: Some(1),
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.asset_id, "test-asset");
        assert!(resp.bids.len() <= 1);
        assert!(resp.asks.len() <= 1);

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_reconstruct_rejects_depth_above_max() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();
        write_test_data(base).await;

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        // max_depth = 5 mirrors the HTTP api.max_depth guard.
        let handle = start_grpc_server(addr, replay, integrity, execution, 5, shutdown.clone())
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let status = client
            .reconstruct(proto::ReconstructRequest {
                asset_id: "test-asset".into(),
                at_us: 1_700_000_000_000_000 + 1,
                mode: "recv_time".into(),
                depth: Some(6),
            })
            .await
            .expect_err("depth above max_depth must be rejected");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(
            status.message().contains("max_depth"),
            "unexpected message: {}",
            status.message()
        );

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_reconstruct_not_found_for_missing_asset() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();
        // No test data written

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let result = client
            .reconstruct(proto::ReconstructRequest {
                asset_id: "nonexistent".into(),
                at_us: 1_000_000,
                mode: "recv_time".into(),
                depth: None,
            })
            .await;

        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::NotFound);

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_reconstruct_exchange_time_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();
        write_test_data(base).await;

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let resp = client
            .reconstruct(proto::ReconstructRequest {
                asset_id: "test-asset".into(),
                at_us: 1_700_000_000_000_000 + 1,
                mode: "exchange_time".into(),
                depth: None,
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.asset_id, "test-asset");
        assert_eq!(resp.mode, "exchange_time");

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_integrity_summary_response_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();
        write_test_data(base).await;

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let start_us = 1_700_000_000_000_000 - 1;
        let end_us = 1_700_000_000_000_000 + 1_000_000;
        let resp = client
            .integrity_summary(proto::IntegritySummaryRequest {
                asset_id: "test-asset".into(),
                start_us,
                end_us,
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.asset_id, "test-asset");
        assert_eq!(resp.start_us, start_us);
        assert_eq!(resp.end_us, end_us);
        // completeness mirrors the HTTP two-value domain (HFT-review #15).
        assert!(["complete", "best_effort"].contains(&resp.completeness.as_str()));

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_execution_timeline_with_asset_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let resp = client
            .execution_timeline(proto::ExecutionTimelineRequest {
                asset_id: Some("test-asset".into()),
                order_id: None,
                start_us: 1,
                end_us: 86_400_000_001, // exactly the 24h max window
                limit: 10,
                offset: 0,
                descending: false,
            })
            .await
            .unwrap()
            .into_inner();

        // No execution data, but no error
        assert_eq!(resp.total_count, 0);

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }

    #[tokio::test]
    async fn test_execution_timeline_with_order_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_str().unwrap();

        let port = free_port().await;
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let shutdown = CancellationToken::new();
        let (replay, integrity, execution) = build_test_services(base);

        let handle = start_grpc_server(
            addr,
            replay,
            integrity,
            execution,
            usize::MAX,
            shutdown.clone(),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut client = WorkstationServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .unwrap();

        let resp = client
            .execution_timeline(proto::ExecutionTimelineRequest {
                asset_id: None,
                order_id: Some("order-xyz".into()),
                start_us: 1,
                end_us: 86_400_000_001, // exactly the 24h max window
                limit: 50,
                offset: 0,
                descending: false,
            })
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.total_count, 0);
        assert!(resp.events.is_empty());

        shutdown.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
    }
}
