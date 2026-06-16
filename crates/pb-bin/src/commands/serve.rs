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
    let max_depth = settings.get_int("api.max_depth").unwrap_or(200).max(1) as usize;
    // Clamp default_depth to max_depth: a default above the max would request more
    // levels than any query is allowed to return (HFT-review: config invariant).
    let default_depth =
        (settings.get_int("api.default_depth").unwrap_or(20).max(1) as usize).min(max_depth);
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
    let hydration_result =
        pb_api::hydration::hydrate(&live, Some(&reader), Some(&wal_config), &token_ids).await;
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
        token_ids.clone(),
        parquet_base_path.clone(),
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
            // Optional bearer-token auth (A.158/P2-SEC-2); empty/unset = open.
            auth_token: settings
                .get_string("api.auth_token")
                .ok()
                .filter(|t| !t.is_empty()),
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

/// Why a single tail session ended, so the supervising recovery loop can decide
/// whether to reopen, re-hydrate, or stop.
enum TailOutcome {
    /// A WAL segment gap was detected (our position was pruned away). The caller
    /// should re-hydrate from checkpoints + the current WAL and reopen at the
    /// fresh tail.
    Resync,
    /// The live projector channel is closed — reopening cannot help; stop.
    ProjectorDead,
    /// Shutdown was requested.
    Shutdown,
}

/// Spawn a background task that continuously tails the WAL for new records and
/// feeds them through the projector. Updates health atomics for `/health/ready`.
///
/// The tailer is self-healing: instead of dying permanently on a transient
/// reader-open failure or a segment-gap resync, it retries the open with
/// exponential backoff and re-hydrates the read model on a gap, reflecting
/// not-ready via `needs_resync` while it cannot serve fresh data (audit finding
/// P2-SUP-1). A closed projector is treated as terminal — reopening cannot
/// revive it, so the tailer marks not-ready and stops for the process
/// supervisor to restart serve.
#[allow(clippy::too_many_arguments)]
fn spawn_wal_tailer(
    config: pb_wal::WalConfig,
    live: pb_api::LiveReadModel,
    token_ids: Vec<String>,
    parquet_base_path: String,
    start_position: Option<pb_wal::WalPosition>,
    shutdown: CancellationToken,
    lag_bytes_atomic: Arc<AtomicU64>,
    needs_resync_atomic: Arc<AtomicBool>,
    max_consumer_lag_bytes: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut start_position = start_position;
        let mut open_backoff = Duration::from_millis(100);
        let max_open_backoff = Duration::from_secs(5);

        'recovery: loop {
            if shutdown.is_cancelled() {
                break;
            }

            // Open (or reopen) the reader, retrying transient failures with
            // bounded exponential backoff. While we cannot open it, the runtime
            // is not ready to serve fresh data.
            let reader = loop {
                if shutdown.is_cancelled() {
                    return;
                }
                let reader_result = match start_position {
                    Some(position) => {
                        pb_wal::WalReader::open_at(config.clone(), "serve-live", position)
                    }
                    None => pb_wal::WalReader::open(config.clone(), "serve-live"),
                };
                match reader_result {
                    Ok(r) => {
                        needs_resync_atomic.store(false, Ordering::Relaxed);
                        open_backoff = Duration::from_millis(100);
                        break r;
                    }
                    Err(e) => {
                        needs_resync_atomic.store(true, Ordering::Relaxed);
                        tracing::warn!(
                            error = %e,
                            backoff_ms = open_backoff.as_millis() as u64,
                            "failed to open WAL reader; retrying"
                        );
                        tokio::select! {
                            _ = tokio::time::sleep(open_backoff) => {}
                            _ = shutdown.cancelled() => return,
                        }
                        open_backoff = (open_backoff * 2).min(max_open_backoff);
                    }
                }
            };

            let outcome = tail_session(
                reader,
                &config,
                &live,
                &shutdown,
                &lag_bytes_atomic,
                &needs_resync_atomic,
                max_consumer_lag_bytes,
            )
            .await;

            match outcome {
                TailOutcome::Shutdown => break 'recovery,
                TailOutcome::ProjectorDead => {
                    needs_resync_atomic.store(true, Ordering::Relaxed);
                    tracing::error!("live projector is not accepting records; WAL tailer stopping");
                    break 'recovery;
                }
                TailOutcome::Resync => {
                    needs_resync_atomic.store(true, Ordering::Relaxed);
                    tracing::warn!("WAL segment gap detected; re-hydrating live read model");
                    let parquet_reader = pb_replay::ParquetReader::new(&parquet_base_path);
                    let hydration = pb_api::hydration::hydrate(
                        &live,
                        Some(&parquet_reader),
                        Some(&config),
                        &token_ids,
                    )
                    .await;
                    tracing::info!(
                        checkpoints = hydration.checkpoints_loaded,
                        wal_records = hydration.wal_records_replayed,
                        "serve tailer re-hydration complete"
                    );
                    start_position = hydration.wal_end_position;
                    // Brief pause so a persistent gap does not spin re-hydration.
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                        _ = shutdown.cancelled() => break 'recovery,
                    }
                }
            }
        }
    })
}

/// Tail a single open reader until it must be reopened, re-hydrated, or stopped.
async fn tail_session(
    mut reader: pb_wal::WalReader,
    config: &pb_wal::WalConfig,
    live: &pb_api::LiveReadModel,
    shutdown: &CancellationToken,
    lag_bytes_atomic: &Arc<AtomicU64>,
    needs_resync_atomic: &Arc<AtomicBool>,
    max_consumer_lag_bytes: u64,
) -> TailOutcome {
    let poll_interval = Duration::from_millis(50);
    let commit_interval = Duration::from_millis(config.position_commit_interval_ms);
    let mut last_commit = Instant::now();
    let mut dirty_position = false;

    let outcome = loop {
        if shutdown.is_cancelled() {
            break TailOutcome::Shutdown;
        }

        // Check for segment gap (pruned segments).
        if reader.needs_resync() {
            needs_resync_atomic.store(true, Ordering::Relaxed);
            break TailOutcome::Resync;
        }

        // Update lag tracking.
        if let Some(lag) = reader.lag_bytes() {
            lag_bytes_atomic.store(lag, Ordering::Relaxed);
            pb_metrics::set_wal_consumer_lag_bytes(lag);
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
                    if live.apply_record(record).await {
                        dirty_position = true;
                    } else {
                        // The projector is dead. Stop committing positions —
                        // otherwise we would advance the consumer position past
                        // records that were never applied (A.45).
                        break TailOutcome::ProjectorDead;
                    }
                }
                Err(e) => {
                    // A CRC-valid frame that fails to decode means the live read
                    // model has diverged from the WAL for this record. We skip it
                    // (rather than break to Resync) to guarantee forward progress —
                    // re-hydration would replay and re-hit the same poison frame,
                    // wedging the tailer in an infinite resync loop. But the skip
                    // must be loud and alertable, not a silent warn (HFT-review
                    // finding): operators reconcile from the WAL if it recurs.
                    pb_metrics::record_wal_decode_error();
                    tracing::error!(
                        error = %e,
                        "failed to decode CRC-valid WAL record during live tailing; \
                         skipping — live read model has diverged for this record"
                    );
                }
            },
            Ok(None) => {
                if dirty_position && last_commit.elapsed() >= commit_interval {
                    commit_reader_position(&reader, &mut dirty_position, &mut last_commit);
                }
                // No new records — poll again after a short delay.
                tokio::select! {
                    _ = tokio::time::sleep(poll_interval) => {}
                    _ = shutdown.cancelled() => break TailOutcome::Shutdown,
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "WAL read error during live tailing");
                if dirty_position && last_commit.elapsed() >= commit_interval {
                    commit_reader_position(&reader, &mut dirty_position, &mut last_commit);
                }
                tokio::select! {
                    _ = tokio::time::sleep(poll_interval) => {}
                    _ = shutdown.cancelled() => break TailOutcome::Shutdown,
                }
            }
        }
    };

    if dirty_position {
        commit_reader_position(&reader, &mut dirty_position, &mut last_commit);
    }
    if let Err(e) = reader.commit_position() {
        tracing::warn!(error = %e, "failed to commit WAL reader position");
    }
    outcome
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
                ingest_ordinal: None,
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
                ingest_ordinal: None,
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
            vec!["tok1".to_string()],
            dir.path().to_string_lossy().to_string(),
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

    #[tokio::test]
    async fn tail_session_returns_resync_on_segment_gap() {
        // A tailer whose committed position has been pruned away must report a
        // resync (so the recovery loop re-hydrates) rather than dying silently.
        let dir = tempfile::tempdir().unwrap();
        let config = pb_wal::WalConfig {
            base_path: dir.path().to_path_buf(),
            segment_size: 128, // tiny → many sealed segments
            max_segments: 100,
            ..pb_wal::WalConfig::default()
        };
        let mut writer = pb_wal::WalWriter::open(config.clone()).unwrap();
        for i in 0..20u64 {
            let rec = snapshot_record("tok1", Side::Bid, 5000 + i as u32, 10.0, i);
            writer
                .append(&pb_wal::codec::encode(&rec).unwrap())
                .unwrap();
        }
        writer.flush().unwrap();

        // Consume one record as "serve-live" and commit, leaving the committed
        // position in an early (soon-to-be-pruned) segment.
        {
            let mut reader = pb_wal::WalReader::open(config.clone(), "serve-live").unwrap();
            reader.next().unwrap();
            reader.commit_position().unwrap();
        }

        // Prune ignoring the consumer list → every sealed segment (including the
        // one the committed position references) is removed, creating a gap.
        writer.prune(&[]).unwrap();

        let live = pb_api::LiveReadModel::new(pb_api::FeedMode::FixedTokens);
        live.set_active_assets(vec!["tok1".to_string()]).await;

        let reader = pb_wal::WalReader::open(config.clone(), "serve-live").unwrap();
        let shutdown = CancellationToken::new();
        let needs_resync = Arc::new(AtomicBool::new(false));
        let outcome = tail_session(
            reader,
            &config,
            &live,
            &shutdown,
            &Arc::new(AtomicU64::new(0)),
            &needs_resync,
            u64::MAX,
        )
        .await;

        assert!(matches!(outcome, TailOutcome::Resync));
        assert!(needs_resync.load(Ordering::Relaxed));
    }
}
