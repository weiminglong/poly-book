use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use config::Config;
use tokio_util::sync::CancellationToken;

use super::pipeline;

/// Standalone serve runtime: checkpoint hydration → WAL tail → HTTP/WS server.
///
/// No venue connectivity — reads state from WAL written by an `ingest` process.
pub async fn run(
    settings: Config,
    tokens: String,
    enable_metrics: bool,
    shutdown: CancellationToken,
    slug_registry: pb_types::SlugRegistry,
) -> Result<()> {
    let token_ids: Vec<String> = tokens
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if token_ids.is_empty() {
        anyhow::bail!("--tokens must contain at least one token ID");
    }

    if enable_metrics {
        pipeline::start_metrics_server(&settings).await?;
    }

    let api_listen_addr: SocketAddr = settings
        .get_string("api.listen_addr")
        .unwrap_or_else(|_| "0.0.0.0:3000".to_string())
        .parse()?;
    let default_depth = settings.get_int("api.default_depth").unwrap_or(20).max(1) as usize;
    let max_depth = settings.get_int("api.max_depth").unwrap_or(200).max(1) as usize;
    let stale_after_secs = settings
        .get_int("api.stale_after_secs")
        .unwrap_or(15)
        .max(1) as u64;
    let parquet_base_path = settings
        .get_string("storage.parquet_base_path")
        .unwrap_or_else(|_| "./data".to_string());
    let wal_config = pipeline::wal_config_from_settings(&settings);

    // Build live read model.
    let live = pb_api::LiveReadModel::new(pb_api::FeedMode::FixedTokens);
    let broadcast = pb_api::PerAssetBroadcast::new();
    broadcast.set_active_assets(&token_ids);
    live.set_active_assets(token_ids.clone()).await;

    // Hydrate from checkpoints + WAL.
    let reader = pb_replay::ParquetReader::new(&parquet_base_path);
    let hydration_result = pb_api::hydration::hydrate(
        &live,
        Some(&reader),
        Some(&wal_config.base_path),
        &token_ids,
    )
    .await;
    tracing::info!(
        checkpoints = hydration_result.checkpoints_loaded,
        wal_records = hydration_result.wal_records_replayed,
        "serve runtime hydration complete"
    );

    // Health tracking atomics shared between WAL tailer and API.
    let wal_lag_bytes = Arc::new(AtomicU64::new(0));
    let needs_resync = Arc::new(AtomicBool::new(false));

    let max_consumer_lag = wal_config.max_consumer_lag_bytes;

    // Start tailing WAL for new records (live tail).
    let wal_tail_handle = spawn_wal_tailer(
        wal_config,
        live.clone(),
        broadcast.clone(),
        default_depth,
        shutdown.child_token(),
        wal_lag_bytes.clone(),
        needs_resync.clone(),
        max_consumer_lag,
    );

    // Build and start HTTP/WS server.
    let (replay_service, integrity_service, execution_service) =
        pipeline::build_services(&settings).await;
    let query_service = pipeline::build_query_service(&settings).await;
    let (query_max_rows, query_timeout_secs) = pipeline::query_config_from_settings(&settings);

    // Optionally start gRPC server.
    let (grpc_enabled, grpc_addr) = pipeline::grpc_config_from_settings(&settings);
    let grpc_handle = if grpc_enabled {
        Some(
            pb_grpc::start_grpc_server(
                grpc_addr,
                replay_service.clone(),
                integrity_service.clone(),
                execution_service.clone(),
                shutdown.child_token(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?,
        )
    } else {
        None
    };

    let state = pb_api::AppState {
        live,
        config: pb_api::ApiConfig {
            parquet_base_path,
            default_depth,
            max_depth,
            stale_after_secs,
            query_max_rows,
            query_timeout_secs,
        },
        broadcast: Some(broadcast),
        slug_registry,
        replay_service,
        integrity_service,
        execution_service,
        query_service,
        wal_lag_bytes,
        needs_resync,
    };
    let listener = tokio::net::TcpListener::bind(api_listen_addr).await?;
    tracing::info!(%api_listen_addr, "serve runtime API server bound");

    let serve_result = pb_api::serve(listener, state, shutdown.child_token()).await;
    let mut handles = vec![wal_tail_handle];
    if let Some(h) = grpc_handle {
        handles.push(h);
    }
    pipeline::shutdown_handles(handles, "serve task").await;
    serve_result?;
    Ok(())
}

/// Spawn a background task that continuously tails the WAL for new records
/// and feeds them through the projector. Updates health atomics for the
/// `/health` endpoint.
#[allow(clippy::too_many_arguments)]
fn spawn_wal_tailer(
    config: pb_wal::WalConfig,
    live: pb_api::LiveReadModel,
    _broadcast: pb_api::PerAssetBroadcast,
    _default_depth: usize,
    shutdown: CancellationToken,
    lag_bytes_atomic: Arc<AtomicU64>,
    needs_resync_atomic: Arc<AtomicBool>,
    max_consumer_lag_bytes: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = match pb_wal::WalReader::open(config, "serve-live") {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "failed to open WAL reader for live tailing");
                return;
            }
        };

        let poll_interval = std::time::Duration::from_millis(50);

        loop {
            if shutdown.is_cancelled() {
                break;
            }

            // Check for segment gap (pruned segments).
            if reader.needs_resync() {
                tracing::warn!("WAL segment gap detected, triggering re-hydration");
                needs_resync_atomic.store(true, Ordering::Relaxed);
                break;
            }

            // Update lag tracking.
            if let Some(lag) = reader.lag_bytes() {
                lag_bytes_atomic.store(lag, Ordering::Relaxed);
                if lag > max_consumer_lag_bytes {
                    tracing::warn!(
                        lag_bytes = lag,
                        threshold = max_consumer_lag_bytes,
                        "WAL consumer lag exceeds threshold"
                    );
                }
            }

            match reader.next() {
                Ok(Some(payload)) => match pb_wal::codec::decode(&payload) {
                    Ok(record) => {
                        live.apply_record(record).await;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to decode WAL record");
                    }
                },
                Ok(None) => {
                    // No new records — poll again after a short delay.
                    tokio::select! {
                        _ = tokio::time::sleep(poll_interval) => {}
                        _ = shutdown.cancelled() => break,
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "WAL read error during live tailing");
                    tokio::select! {
                        _ = tokio::time::sleep(poll_interval) => {}
                        _ = shutdown.cancelled() => break,
                    }
                }
            }
        }

        if let Err(e) = reader.commit_position() {
            tracing::warn!(error = %e, "failed to commit WAL reader position on shutdown");
        }
    })
}
