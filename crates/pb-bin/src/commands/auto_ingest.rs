use anyhow::Result;
use config::Config;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::market_discovery::{
    current_unix_secs, discover_with_retry, populate_registry, DiscoverOutcome,
};
use super::pipeline;

pub async fn run(
    settings: Config,
    enable_parquet: bool,
    enable_clickhouse: bool,
    enable_metrics: bool,
    shutdown: CancellationToken,
    slug_registry: pb_types::SlugRegistry,
) -> Result<()> {
    tracing::info!("starting auto-ingest with automatic market rotation");

    if enable_metrics {
        pipeline::start_metrics_server(&settings).await?;
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
    let (active_assets_tx, active_assets_rx) = tokio::sync::watch::channel(Vec::<String>::new());

    let checkpoint_handle = pipeline::start_checkpoint_producer(
        &settings,
        active_assets_rx,
        event_tx.clone(),
        &shutdown,
    );

    let sinks = pipeline::start_storage_sinks(&settings, enable_parquet, enable_clickhouse).await?;

    let mut fanout_txs: Vec<tokio::sync::mpsc::Sender<pb_types::PersistedRecord>> = Vec::new();
    let mut fanout_fwd_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    if let Some(ptx) = sinks.parquet_tx.clone() {
        let (ftx, mut frx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
        fanout_txs.push(ftx);
        fanout_fwd_handles.push(tokio::spawn(async move {
            while let Some(event) = frx.recv().await {
                if let Err(e) = ptx.send(event).await {
                    tracing::warn!("parquet sink send failed: {e}");
                    break;
                }
            }
        }));
    }

    if let Some(ctx) = sinks.clickhouse_tx.clone() {
        let (ftx, mut frx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
        fanout_txs.push(ftx);
        fanout_fwd_handles.push(tokio::spawn(async move {
            while let Some(event) = frx.recv().await {
                if let Err(e) = ctx.send(event).await {
                    tracing::warn!("clickhouse sink send failed: {e}");
                    break;
                }
            }
        }));
    }

    // Open the WAL writer. auto-ingest is the production rotating-market mode,
    // so it must persist to the WAL like `ingest` does — otherwise the
    // documented ingest→serve topology has no live tail (audit finding A.75/A.98).
    // A failure to open the durability backbone is fatal.
    let wal_config = pipeline::wal_config_from_settings(&settings);
    let wal_flush_interval = Duration::from_millis(wal_config.flush_interval_ms);
    let wal_sync_interval = Duration::from_millis(wal_config.sync_interval_ms);
    let wal_base_path = wal_config.base_path.clone();
    let mut wal_writer = pb_wal::WalWriter::open(wal_config)
        .map_err(|e| anyhow::anyhow!("failed to open WAL writer: {e}"))?;
    tracing::info!("WAL writer opened");

    // Set when the drain task hits a fatal WAL error so run() can exit non-zero
    // and the supervisor restarts us instead of limping on without durability.
    let wal_failed = Arc::new(AtomicBool::new(false));

    let drain_shutdown = shutdown.clone();
    let drain_wal_failed = wal_failed.clone();
    let fanout_handle = tokio::spawn(async move {
        let mut flush_tick = tokio::time::interval(wal_flush_interval);
        let mut sync_tick = tokio::time::interval(wal_sync_interval);
        let mut prune_tick = tokio::time::interval(Duration::from_secs(60));
        flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        sync_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        prune_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut wal_unflushed = false;
        let mut wal_unsynced = false;

        loop {
            let event = tokio::select! {
                biased;
                _ = prune_tick.tick() => {
                    // Reclaim WAL segments all consumers have advanced past
                    // (A.17/A.20/A.47).
                    let consumers = pipeline::wal_consumer_position_files(&wal_base_path);
                    if let Err(e) = wal_writer.prune_with_backpressure(&consumers) {
                        tracing::warn!(error = %e, "WAL prune failed");
                    }
                    continue;
                }
                _ = flush_tick.tick() => {
                    if wal_unflushed {
                        if let Err(e) = wal_writer.flush() {
                            tracing::error!(error = %e, "WAL flush failed — aborting ingest");
                            drain_wal_failed.store(true, Ordering::SeqCst);
                            drain_shutdown.cancel();
                            break;
                        }
                        wal_unflushed = false;
                    }
                    continue;
                }
                _ = sync_tick.tick() => {
                    if wal_unsynced {
                        if let Err(e) = wal_writer.sync() {
                            tracing::error!(error = %e, "WAL sync failed — aborting ingest");
                            drain_wal_failed.store(true, Ordering::SeqCst);
                            drain_shutdown.cancel();
                            break;
                        }
                        wal_unsynced = false;
                        wal_unflushed = false;
                    }
                    continue;
                }
                ev = event_rx.recv() => match ev {
                    Some(e) => e,
                    None => {
                        tracing::info!("event channel closed, fan-out stopping");
                        break;
                    }
                },
            };

            // WAL first, then fan-out. Encode failure drops the record from both
            // WAL and sinks (keeps them consistent); append failure is fatal.
            match pb_wal::codec::encode(&event) {
                Ok(payload) => {
                    if let Err(e) = wal_writer.append(&payload) {
                        pb_metrics::record_wal_append_failure();
                        tracing::error!(error = %e, "WAL append failed — aborting ingest");
                        drain_wal_failed.store(true, Ordering::SeqCst);
                        drain_shutdown.cancel();
                        break;
                    }
                    wal_unflushed = true;
                    wal_unsynced = true;
                }
                Err(e) => {
                    tracing::error!(error = %e, "WAL encode failed, dropping record");
                    continue;
                }
            }

            if !pipeline::fanout_event(event, fanout_txs.as_slice()).await {
                break;
            }
        }

        // Durably flush + fsync remaining buffered records on shutdown.
        if let Err(e) = wal_writer.sync() {
            tracing::error!(error = %e, "WAL sync failed during shutdown");
        }
    });

    let rate_requests = settings.get_int("feed.rate_limit_requests").unwrap_or(1500) as u32;
    let rate_window = settings
        .get_int("feed.rate_limit_window_secs")
        .unwrap_or(10) as u32;
    let rate_limiter = pb_feed::RateLimiter::with_window(rate_requests, rate_window);

    let rest_config = pb_feed::RestConfig {
        clob_base_url: settings
            .get_string("feed.rest_url")
            .unwrap_or_else(|_| pb_feed::RestConfig::default().clob_base_url),
        gamma_base_url: settings
            .get_string("feed.gamma_url")
            .unwrap_or_else(|_| pb_feed::RestConfig::default().gamma_base_url),
    };
    let rest = pb_feed::RestClient::new(rate_limiter).with_config(rest_config);

    let ws_config = pipeline::ws_config_from_settings(&settings);

    let mut front: Option<Generation> = None;
    let mut active_bucket: Option<u64> = None;

    tracing::info!("auto-ingest pipeline running, press Ctrl+C to stop");

    loop {
        let now_secs = current_unix_secs();
        let current_bucket = now_secs - (now_secs % 300);
        let next_bucket = current_bucket + 300;

        let target_bucket = if active_bucket.is_none() {
            current_bucket
        } else {
            next_bucket
        };

        if active_bucket == Some(target_bucket) {
            let sleep_until = (target_bucket + 300) - 10;
            let sleep_secs = sleep_until.saturating_sub(current_unix_secs());
            if sleep_secs > 0 {
                tracing::debug!(
                    sleep_secs,
                    target_bucket,
                    "already on target, waiting for next boundary"
                );
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(sleep_secs)) => {}
                    _ = shutdown.cancelled() => break,
                }
            }
            continue;
        }

        if active_bucket.is_some() {
            let sleep_until = target_bucket - 10;
            let sleep_secs = sleep_until.saturating_sub(current_unix_secs());
            if sleep_secs > 0 {
                tracing::debug!(
                    sleep_secs,
                    next_bucket = target_bucket,
                    "sleeping until pre-rotation window"
                );
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(sleep_secs)) => {}
                    _ = shutdown.cancelled() => break,
                }
            }
        }

        if shutdown.is_cancelled() {
            break;
        }

        let target_slug = format!("btc-updown-5m-{target_bucket}");

        let discovery = match discover_with_retry(&rest, &target_slug, &shutdown).await {
            DiscoverOutcome::Found(result) => result,
            DiscoverOutcome::Shutdown => break,
            DiscoverOutcome::Failed => continue,
        };
        populate_registry(&slug_registry, &discovery);
        let token_ids = discovery.token_ids;

        // Subscribe to the NEW market while the OLD one keeps running, so the
        // expiring market's final ~10 seconds are still captured instead of
        // being dropped by an early unsubscribe (audit finding A.19/A.75).
        let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<pb_feed::FeedMessage>(2_048);
        let new_token = shutdown.child_token();

        let ws_client =
            pb_feed::WsClient::new(token_ids.clone(), raw_tx)?.with_config(ws_config.clone());
        let ws_cancel = new_token.child_token();
        let ws_handle = tokio::spawn(async move {
            if let Err(e) = ws_client.run_with_token(ws_cancel).await {
                tracing::error!(error = %e, "websocket client failed");
            }
        });

        let mut dispatcher = pb_feed::Dispatcher::new(raw_rx, event_tx.clone());
        let dispatcher_cancel = new_token.child_token();
        let disp_handle = tokio::spawn(async move {
            if let Err(e) = dispatcher.run_with_token(dispatcher_cancel).await {
                tracing::error!(error = %e, "dispatcher failed");
            }
        });

        // Cut over the previous market only after its bucket boundary (when it
        // actually expires), then join its tasks so they are not orphaned
        // (audit finding A.50).
        if let Some(old) = front.take() {
            let wait = target_bucket.saturating_sub(current_unix_secs());
            if wait > 0 {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                    _ = shutdown.cancelled() => {}
                }
            }
            old.token.cancel();
            pipeline::shutdown_handles(old.handles, "rotated-out feed task").await;
        }

        front = Some(Generation {
            token: new_token,
            handles: vec![ws_handle, disp_handle],
        });
        active_bucket = Some(target_bucket);
        let _ = active_assets_tx.send(token_ids.clone());
        pb_metrics::record_rotation();
        tracing::info!(slug = %target_slug, tokens = ?token_ids, "rotated to new market");

        if shutdown.is_cancelled() {
            break;
        }
    }

    tracing::info!("shutting down auto-ingest pipeline");

    if let Some(old) = front.take() {
        old.token.cancel();
        pipeline::shutdown_handles(old.handles, "feed task").await;
    }
    let _ = active_assets_tx.send(Vec::new());

    drop(event_tx);
    drop(sinks.parquet_tx);
    drop(sinks.clickhouse_tx);

    pipeline::shutdown_handles(vec![fanout_handle], "fan-out task").await;
    pipeline::shutdown_handles(fanout_fwd_handles, "fan-out forwarding task").await;
    pipeline::shutdown_handles(sinks.task_handles, "sink").await;
    if let Some(handle) = checkpoint_handle {
        pipeline::shutdown_handles(vec![handle], "checkpoint producer").await;
    }

    // If the drain task aborted on a WAL durability failure, exit non-zero so
    // the supervisor restarts us rather than running on without persistence.
    if wal_failed.load(Ordering::SeqCst) {
        anyhow::bail!("auto-ingest aborted: WAL durability failure");
    }

    tracing::info!("graceful shutdown complete");
    Ok(())
}

/// A single market-subscription generation: its cancellation token and the
/// feed/dispatcher task handles, kept together so the generation can be cleanly
/// cancelled and joined on rotation or shutdown.
struct Generation {
    token: CancellationToken,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

#[cfg(test)]
mod tests {
    use super::pipeline::fanout_event;
    use pb_types::{DataSource, EventProvenance, IngestEvent, IngestEventKind, PersistedRecord};

    fn sample_record() -> PersistedRecord {
        PersistedRecord::Ingest(IngestEvent {
            asset_id: None,
            kind: IngestEventKind::ReconnectSuccess,
            provenance: EventProvenance {
                recv_timestamp_us: 1,
                exchange_timestamp_us: 0,
                source: DataSource::System,
                source_event_id: None,
                source_session_id: None,
                sequence: None,
            },
            expected_sequence: None,
            observed_sequence: None,
            details: None,
        })
    }

    #[tokio::test]
    async fn fanout_event_fails_closed_when_any_sink_channel_is_closed() {
        let (open_tx, mut open_rx) = tokio::sync::mpsc::channel(4);
        let (closed_tx, closed_rx) = tokio::sync::mpsc::channel(4);
        drop(closed_rx);

        let fanout_txs = vec![open_tx, closed_tx];
        let ok = fanout_event(sample_record(), fanout_txs.as_slice()).await;

        assert!(!ok);
        let received = open_rx.recv().await.unwrap();
        assert!(matches!(received, PersistedRecord::Ingest(_)));
    }

    #[tokio::test]
    async fn fanout_event_empty_slice_returns_true() {
        let ok = fanout_event(sample_record(), &[]).await;
        assert!(ok);
    }

    #[tokio::test]
    async fn fanout_event_single_open_channel_returns_true() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let ok = fanout_event(sample_record(), &[tx]).await;
        assert!(ok);
        let received = rx.recv().await.unwrap();
        assert!(matches!(received, PersistedRecord::Ingest(_)));
    }

    #[tokio::test]
    async fn fanout_event_single_closed_channel_returns_false() {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        drop(rx);
        let ok = fanout_event(sample_record(), &[tx]).await;
        assert!(!ok);
    }

    #[tokio::test]
    async fn fanout_event_two_open_channels_delivers_to_both() {
        let (tx1, mut rx1) = tokio::sync::mpsc::channel(4);
        let (tx2, mut rx2) = tokio::sync::mpsc::channel(4);
        let ok = fanout_event(sample_record(), &[tx1, tx2]).await;
        assert!(ok);
        let r1 = rx1.recv().await.unwrap();
        let r2 = rx2.recv().await.unwrap();
        assert!(matches!(r1, PersistedRecord::Ingest(_)));
        assert!(matches!(r2, PersistedRecord::Ingest(_)));
    }
}
