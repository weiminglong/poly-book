use anyhow::Result;
use config::Config;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::market_discovery::{current_unix_secs, discover_with_retry, DiscoverOutcome};
use super::pipeline;

async fn fanout_event(
    event: pb_types::PersistedRecord,
    fanout_txs: &[tokio::sync::mpsc::Sender<pb_types::PersistedRecord>],
) -> bool {
    match fanout_txs {
        [] => true,
        [a] => {
            if let Err(e) = a.send(event).await {
                tracing::warn!("fan-out channel closed: {e}");
                return false;
            }
            true
        }
        [a, b] => {
            let ev_a = event.clone();
            let (ra, rb) = tokio::join!(a.send(ev_a), b.send(event));
            if ra.is_err() || rb.is_err() {
                if let Err(e) = ra {
                    tracing::warn!("fan-out channel 0 closed: {e}");
                }
                if let Err(e) = rb {
                    tracing::warn!("fan-out channel 1 closed: {e}");
                }
                return false;
            }
            true
        }
        _ => unreachable!("at most 2 sinks"),
    }
}

pub async fn run(
    settings: Config,
    enable_parquet: bool,
    enable_clickhouse: bool,
    enable_metrics: bool,
    shutdown: CancellationToken,
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

    let fanout_handle = tokio::spawn(async move {
        loop {
            match event_rx.recv().await {
                Some(event) => {
                    if !fanout_event(event, fanout_txs.as_slice()).await {
                        break;
                    }
                }
                None => {
                    tracing::info!("event channel closed, fan-out stopping");
                    break;
                }
            }
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

    let mut front_token: Option<CancellationToken> = None;
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

        let token_ids = match discover_with_retry(&rest, &target_slug, &shutdown).await {
            DiscoverOutcome::Found(ids) => ids,
            DiscoverOutcome::Shutdown => break,
            DiscoverOutcome::Failed => continue,
        };

        if let Some(old) = front_token.take() {
            old.cancel();
            tokio::task::yield_now().await;
        }

        let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<pb_feed::FeedMessage>(2_048);
        let new_token = shutdown.child_token();

        let ws_client =
            pb_feed::WsClient::new(token_ids.clone(), raw_tx)?.with_config(ws_config.clone());
        let ws_cancel = new_token.child_token();
        tokio::spawn(async move {
            if let Err(e) = ws_client.run_with_token(ws_cancel).await {
                tracing::error!(error = %e, "websocket client failed");
            }
        });

        let mut dispatcher = pb_feed::Dispatcher::new(raw_rx, event_tx.clone());
        let dispatcher_cancel = new_token.child_token();
        tokio::spawn(async move {
            if let Err(e) = dispatcher.run_with_token(dispatcher_cancel).await {
                tracing::error!(error = %e, "dispatcher failed");
            }
        });

        front_token = Some(new_token);
        active_bucket = Some(target_bucket);
        let _ = active_assets_tx.send(token_ids.clone());
        pb_metrics::record_rotation();
        tracing::info!(slug = %target_slug, tokens = ?token_ids, "rotated to new market");
    }

    tracing::info!("shutting down auto-ingest pipeline");

    if let Some(old) = front_token.take() {
        old.cancel();
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

    tracing::info!("graceful shutdown complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fanout_event;
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
}
