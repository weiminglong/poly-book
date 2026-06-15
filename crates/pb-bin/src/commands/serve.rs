use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
        .unwrap_or_else(|_| "127.0.0.1:3000".to_string())
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
    live.configure_broadcast(broadcast.clone(), default_depth)
        .await;

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
        hydration_result.wal_end_position,
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
    start_position: Option<pb_wal::WalPosition>,
    shutdown: CancellationToken,
    lag_bytes_atomic: Arc<AtomicU64>,
    needs_resync_atomic: Arc<AtomicBool>,
    max_consumer_lag_bytes: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let position_commit_interval_ms = config.position_commit_interval_ms;
        let reader_result = match start_position {
            Some(position) => pb_wal::WalReader::open_at(config, "serve-live", position),
            None => pb_wal::WalReader::open(config, "serve-live"),
        };
        let mut reader = match reader_result {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "failed to open WAL reader for live tailing");
                return;
            }
        };

        let poll_interval = Duration::from_millis(50);
        let commit_interval = Duration::from_millis(position_commit_interval_ms);
        let mut last_commit = Instant::now();
        let mut dirty_position = false;

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
                        dirty_position = true;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to decode WAL record");
                    }
                },
                Ok(None) => {
                    if dirty_position && last_commit.elapsed() >= commit_interval {
                        commit_reader_position(&reader, &mut dirty_position, &mut last_commit);
                    }
                    // No new records — poll again after a short delay.
                    tokio::select! {
                        _ = tokio::time::sleep(poll_interval) => {}
                        _ = shutdown.cancelled() => break,
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "WAL read error during live tailing");
                    if dirty_position && last_commit.elapsed() >= commit_interval {
                        commit_reader_position(&reader, &mut dirty_position, &mut last_commit);
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(poll_interval) => {}
                        _ = shutdown.cancelled() => break,
                    }
                }
            }
        }

        if dirty_position {
            commit_reader_position(&reader, &mut dirty_position, &mut last_commit);
        }
        if let Err(e) = reader.commit_position() {
            tracing::warn!(error = %e, "failed to commit WAL reader position on shutdown");
        }
    })
}

fn commit_reader_position(
    reader: &pb_wal::WalReader,
    dirty_position: &mut bool,
    last_commit: &mut Instant,
) {
    match reader.commit_position() {
        Ok(()) => {
            *dirty_position = false;
            *last_commit = Instant::now();
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to commit WAL reader position");
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::timeout;

    use super::*;
    use pb_types::event::{
        BookEvent, BookEventKind, DataSource, EventProvenance, PersistedRecord, Side,
    };
    use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

    fn snapshot_record(
        asset_id: &str,
        side: Side,
        price: u32,
        size: f64,
        seq: u64,
    ) -> PersistedRecord {
        PersistedRecord::Book(BookEvent {
            asset_id: AssetId::new(asset_id),
            kind: BookEventKind::Snapshot,
            side,
            price: FixedPrice::new(price).unwrap(),
            size: FixedSize::from_f64(size).unwrap(),
            provenance: EventProvenance {
                recv_timestamp_us: 1_700_000_000_000_000,
                exchange_timestamp_us: 1_700_000_000_000_000,
                source: DataSource::WebSocket,
                source_event_id: Some("snap-1".to_string()),
                source_session_id: Some("ws-session-1".to_string()),
                sequence: Some(Sequence::new(seq)),
            },
        })
    }

    fn reconnect_success_record() -> PersistedRecord {
        PersistedRecord::Ingest(pb_types::IngestEvent {
            asset_id: None,
            kind: pb_types::IngestEventKind::ReconnectSuccess,
            provenance: EventProvenance {
                recv_timestamp_us: 1_700_000_000_100_000,
                exchange_timestamp_us: 0,
                source: DataSource::WebSocket,
                source_event_id: None,
                source_session_id: Some("ws-session-1".to_string()),
                sequence: None,
            },
            expected_sequence: None,
            observed_sequence: None,
            details: None,
        })
    }

    #[tokio::test]
    async fn wal_tailer_broadcasts_updates_in_serve_mode() {
        let dir = tempfile::tempdir().unwrap();
        let config = pb_wal::WalConfig {
            base_path: dir.path().to_path_buf(),
            ..pb_wal::WalConfig::default()
        };
        let mut writer = pb_wal::WalWriter::open(config.clone()).unwrap();
        for record in [
            snapshot_record("tok1", Side::Bid, 5000, 10.0, 0),
            snapshot_record("tok1", Side::Ask, 6000, 20.0, 1),
            reconnect_success_record(),
        ] {
            writer
                .append(&pb_wal::codec::encode(&record).unwrap())
                .unwrap();
        }
        writer.flush().unwrap();

        let live = pb_api::LiveReadModel::new(pb_api::FeedMode::FixedTokens);
        let broadcast = pb_api::PerAssetBroadcast::new();
        broadcast.set_active_assets(&["tok1".to_string()]);
        live.set_active_assets(vec!["tok1".to_string()]).await;
        live.configure_broadcast(broadcast.clone(), 20).await;
        let mut rx = broadcast.subscribe("tok1").unwrap();

        let shutdown = CancellationToken::new();
        let handle = spawn_wal_tailer(
            config,
            live,
            broadcast,
            20,
            None,
            shutdown.clone(),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicBool::new(false)),
            256 * 1024 * 1024,
        );

        let update = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for broadcast")
            .expect("broadcast closed");
        assert_eq!(update.asset_id, "tok1");
        assert!(!update.bids.is_empty());
        assert!(!update.asks.is_empty());

        shutdown.cancel();
        let _ = handle.await;
    }
}
