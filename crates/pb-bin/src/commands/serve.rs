use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use config::Config;
use pb_types::PersistedRecord;
use tokio_util::sync::CancellationToken;

use super::pipeline;

/// In follow mode an asset stays active for this long after its last record.
/// A BTC 5-minute market stops producing records once it resolves, so a window
/// of two rotations lets the expired market drop off the active list without
/// flapping on a quiet-but-live one.
const FOLLOW_IDLE_US: u64 = 10 * 60 * 1_000_000;

/// Standalone serve runtime: checkpoint hydration → WAL tail → HTTP/WS server.
///
/// No venue connectivity — reads state from WAL written by an `ingest` process.
/// With `tokens`, serves exactly those assets (checkpoint hydration per asset).
/// Without, follows the WAL: every asset the ingest process writes becomes
/// active and ages out `FOLLOW_IDLE_US` after its last record, so `serve` can
/// pair with a rotating `auto-ingest`.
pub async fn run(
    settings: Config,
    tokens: Option<String>,
    enable_metrics: bool,
    shutdown: CancellationToken,
    slug_registry: pb_types::SlugRegistry,
) -> Result<()> {
    let token_ids: Vec<String> = tokens
        .as_deref()
        .map(|t| {
            t.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if tokens.is_some() && token_ids.is_empty() {
        anyhow::bail!("--tokens must contain at least one token ID (omit it to follow the WAL)");
    }
    let follow = tokens.is_none();

    if enable_metrics {
        pipeline::start_metrics_server(&settings).await?;
    }

    let api_listen_addr: SocketAddr = settings
        .get_string("api.listen_addr")
        .unwrap_or_else(|_| "127.0.0.1:3000".to_string())
        .parse()?;
    let max_depth = settings.get_int("api.max_depth").unwrap_or(200).max(1) as usize;
    // Clamp default_depth to max_depth: a default above the max would request more
    // levels than any query is allowed to return (a config invariant).
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
    let mode = if follow {
        pb_api::FeedMode::AutoRotate
    } else {
        pb_api::FeedMode::FixedTokens
    };
    let live = pb_api::LiveReadModel::new(mode);
    let broadcast = pb_api::PerAssetBroadcast::new();
    if !follow {
        broadcast.set_active_assets(&token_ids);
        live.set_active_assets(token_ids.clone()).await;
    }
    live.configure_broadcast(broadcast.clone(), default_depth)
        .await;

    // Hydrate from checkpoints + WAL.
    let (parquet_store, parquet_prefix) = pipeline::build_object_store(&parquet_base_path)?;
    let reader = pb_replay::ParquetReader::from_store(parquet_store, parquet_prefix);
    let hydration_result =
        pb_api::hydration::hydrate(&live, Some(&reader), Some(&wal_config), &token_ids).await;
    tracing::info!(
        checkpoints = hydration_result.checkpoints_loaded,
        wal_records = hydration_result.wal_records_replayed,
        recovery_succeeded = hydration_result.recovery_succeeded,
        "serve runtime hydration complete"
    );

    // Health tracking atomics shared between WAL tailer and API.
    let wal_lag_bytes = Arc::new(AtomicU64::new(0));
    let needs_resync = Arc::new(AtomicBool::new(!hydration_result.recovery_succeeded));

    let max_consumer_lag = wal_config.max_consumer_lag_bytes;

    // In follow mode the active set is derived from the records themselves:
    // seed it from what hydration projected, then keep it current from the tail.
    let mut follow_tracker = follow.then(FollowTracker::default);
    if let Some(tracker) = follow_tracker.as_mut() {
        tracker.seed(&live);
        tracker.sync(&live, &broadcast).await;
    }

    // Start tailing WAL for new records (live tail).
    let wal_tail_handle = spawn_wal_tailer(
        wal_config,
        live.clone(),
        broadcast.clone(),
        token_ids.clone(),
        follow_tracker,
        reader.clone(),
        hydration_result.wal_end_position,
        shutdown.child_token(),
        wal_lag_bytes.clone(),
        needs_resync.clone(),
        max_consumer_lag,
    );

    // Build and start HTTP/WS server.
    let historical = pipeline::build_historical_service(&settings).await?;
    let auth_token = pipeline::api_auth_token_from_settings(&settings);
    pipeline::validate_api_auth_boundary(api_listen_addr, auth_token.as_deref())?;

    let state = pb_api::AppState {
        live,
        config: pb_api::ApiConfig {
            parquet_base_path,
            default_depth,
            max_depth,
            stale_after_secs,
            http_request_timeout_secs: pipeline::cfg_int_min(
                &settings,
                "api.http_request_timeout_secs",
                pb_api::DEFAULT_HTTP_REQUEST_TIMEOUT_SECS as i64,
                1,
            ) as u64,
            auth_token,
            // Optional bundled-SPA hosting (empty/unset = API only).
            static_assets_dir: settings
                .get_string("api.static_assets_dir")
                .ok()
                .filter(|d| !d.is_empty()),
        },
        broadcast: Some(broadcast),
        slug_registry,
        historical,
        wal_lag_bytes,
        needs_resync,
    };
    let listener = tokio::net::TcpListener::bind(api_listen_addr).await?;
    tracing::info!(%api_listen_addr, "serve runtime API server bound");

    let serve_result = pb_api::serve(listener, state, shutdown.child_token()).await;
    pipeline::shutdown_handles(vec![wal_tail_handle], "serve task").await;
    serve_result?;
    Ok(())
}

/// Derives the active asset set from the records flowing through the WAL.
///
/// An asset is active while its newest record is within `FOLLOW_IDLE_US` of the
/// newest record seen for any asset. Record timestamps (not wall-clock) drive
/// the window so a fast WAL catch-up and steady-state tailing behave the same.
#[derive(Default)]
struct FollowTracker {
    last_seen_us: HashMap<String, u64>,
    newest_us: u64,
    published: Vec<String>,
}

impl FollowTracker {
    /// Adopt every asset the read model already projected (after hydration).
    fn seed(&mut self, live: &pb_api::LiveReadModel) {
        for (asset_id, last_update_us) in live.known_assets() {
            self.note(asset_id, last_update_us);
        }
    }

    /// Record a tailed record. Returns true when it introduces a new asset.
    fn observe(&mut self, record: &PersistedRecord) -> bool {
        let asset_id = match record {
            PersistedRecord::Book(e) => e.asset_id.as_str(),
            PersistedRecord::Trade(e) => e.asset_id.as_str(),
            _ => return false,
        };
        let Some(ts) = record.recv_timestamp_us() else {
            return false;
        };
        let is_new = !self.last_seen_us.contains_key(asset_id);
        self.note(asset_id.to_string(), ts);
        is_new
    }

    fn note(&mut self, asset_id: String, ts: u64) {
        let slot = self.last_seen_us.entry(asset_id).or_insert(0);
        *slot = (*slot).max(ts);
        self.newest_us = self.newest_us.max(ts);
    }

    /// Assets seen within the idle window of the newest record, sorted.
    fn desired(&self) -> Vec<String> {
        let cutoff = self.newest_us.saturating_sub(FOLLOW_IDLE_US);
        let mut out: Vec<String> = self
            .last_seen_us
            .iter()
            .filter(|(_, ts)| **ts >= cutoff)
            .map(|(id, _)| id.clone())
            .collect();
        out.sort();
        out
    }

    /// Publish the desired set when it differs from what is currently active.
    async fn sync(&mut self, live: &pb_api::LiveReadModel, broadcast: &pb_api::PerAssetBroadcast) {
        let desired = self.desired();
        if desired == self.published {
            return;
        }
        let cutoff = self.newest_us.saturating_sub(FOLLOW_IDLE_US);
        self.last_seen_us.retain(|_, ts| *ts >= cutoff);
        tracing::info!(assets = ?desired, "follow mode active set changed");
        broadcast.set_active_assets(&desired);
        live.set_active_assets(desired.clone()).await;
        live.set_last_rotation_us(super::market_discovery::now_us())
            .await;
        self.published = desired;
    }
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
/// not-ready via `needs_resync` while it cannot serve fresh data. A closed projector is treated as terminal — reopening cannot
/// revive it, so the tailer marks not-ready and stops for the process
/// supervisor to restart serve.
#[allow(clippy::too_many_arguments)]
fn spawn_wal_tailer(
    config: pb_wal::WalConfig,
    live: pb_api::LiveReadModel,
    broadcast: pb_api::PerAssetBroadcast,
    token_ids: Vec<String>,
    mut follow_tracker: Option<FollowTracker>,
    parquet_reader: pb_replay::ParquetReader,
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
                &broadcast,
                follow_tracker.as_mut(),
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
                    if let Some(tracker) = follow_tracker.as_mut() {
                        tracker.seed(&live);
                        tracker.sync(&live, &broadcast).await;
                    }
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
#[allow(clippy::too_many_arguments)]
async fn tail_session(
    mut reader: pb_wal::WalReader,
    config: &pb_wal::WalConfig,
    live: &pb_api::LiveReadModel,
    broadcast: &pb_api::PerAssetBroadcast,
    mut follow_tracker: Option<&mut FollowTracker>,
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
                    let new_asset = follow_tracker
                        .as_mut()
                        .is_some_and(|tracker| tracker.observe(&record));
                    if live.apply_record(record).await {
                        dirty_position = true;
                        if new_asset {
                            if let Some(tracker) = follow_tracker.as_mut() {
                                tracker.sync(live, broadcast).await;
                            }
                        }
                    } else {
                        // The projector is dead. Stop committing positions —
                        // otherwise we would advance the consumer position past
                        // records that were never applied.
                        break TailOutcome::ProjectorDead;
                    }
                }
                Err(e) => {
                    // A CRC-valid frame that fails to decode means the live read
                    // model has diverged from the WAL for this record. We skip it
                    // (rather than break to Resync) to guarantee forward progress —
                    // re-hydration would replay and re-hit the same poison frame,
                    // wedging the tailer in an infinite resync loop. But the skip
                    // must be loud and alertable, not a silent warn: operators reconcile from the WAL if it recurs.
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
                // Idle: let expired markets age out of the active set.
                if let Some(tracker) = follow_tracker.as_mut() {
                    tracker.sync(live, broadcast).await;
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
            broadcast.clone(),
            vec!["tok1".to_string()],
            None,
            pb_replay::ParquetReader::new(dir.path()),
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
            &pb_api::PerAssetBroadcast::new(),
            None,
            &shutdown,
            &Arc::new(AtomicU64::new(0)),
            &needs_resync,
            u64::MAX,
        )
        .await;

        assert!(matches!(outcome, TailOutcome::Resync));
        assert!(needs_resync.load(Ordering::Relaxed));
    }

    #[test]
    fn follow_tracker_ages_out_idle_assets() {
        let mut t = FollowTracker::default();
        t.note("old".to_string(), 1_000_000);
        t.note("new".to_string(), 1_000_000);
        assert_eq!(t.desired(), vec!["new".to_string(), "old".to_string()]);
        // "new" keeps ticking; "old" falls outside the idle window.
        t.note("new".to_string(), 1_000_000 + FOLLOW_IDLE_US + 1);
        assert_eq!(t.desired(), vec!["new".to_string()]);
    }

    #[test]
    fn follow_tracker_observe_reports_new_assets_only() {
        let mut t = FollowTracker::default();
        let first = snapshot_record("tok1", Side::Bid, 5000, 1.0, 1);
        assert!(t.observe(&first));
        assert!(!t.observe(&first));
        assert!(!t.observe(&reconnect_success_record()));
        assert_eq!(t.desired(), vec!["tok1".to_string()]);
    }

    #[tokio::test]
    async fn follow_tracker_sync_publishes_active_set() {
        let live = pb_api::LiveReadModel::new(pb_api::FeedMode::AutoRotate);
        let broadcast = pb_api::PerAssetBroadcast::new();
        let mut t = FollowTracker::default();
        t.note("tok1".to_string(), 1_000_000);
        t.sync(&live, &broadcast).await;
        let status = live.feed_status_raw().await;
        assert_eq!(status.active_asset_count, 1);
        assert!(broadcast.subscribe("tok1").is_some());
    }
}
