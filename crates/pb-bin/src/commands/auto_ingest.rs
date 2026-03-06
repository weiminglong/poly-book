use anyhow::Result;
use config::Config;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

enum DiscoverOutcome {
    Found(Vec<String>),
    Shutdown,
    Failed,
}

fn current_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs()
}

fn extract_token_ids(events: &[pb_types::wire::GammaEvent]) -> Vec<String> {
    let mut ids = Vec::new();
    for event in events {
        let markets = match &event.markets {
            Some(m) => m,
            None => continue,
        };
        for market in markets {
            let raw = match &market.clob_token_ids {
                Some(s) => s,
                None => continue,
            };
            // Try JSON array first: ["tok1","tok2"]
            if let Ok(parsed) = serde_json::from_str::<Vec<String>>(raw) {
                ids.extend(parsed);
            } else {
                // Fallback: comma-separated
                ids.extend(raw.split(',').map(|s| s.trim().to_string()));
            }
        }
    }
    ids
}

async fn discover_with_retry(
    rest: &pb_feed::RestClient,
    slug: &str,
    shutdown: &CancellationToken,
) -> DiscoverOutcome {
    let mut delay_ms = 2000u64;
    for attempt in 1..=5 {
        if shutdown.is_cancelled() {
            return DiscoverOutcome::Shutdown;
        }
        match rest.discover_by_slug(slug).await {
            Ok(events) => {
                let ids = extract_token_ids(&events);
                if !ids.is_empty() {
                    return DiscoverOutcome::Found(ids);
                }
                tracing::warn!(slug, attempt, "slug returned no token IDs, retrying");
            }
            Err(e) => {
                tracing::warn!(slug, attempt, error = %e, "discovery request failed, retrying");
            }
        }
        pb_metrics::record_discovery_failure();
        if attempt < 5 {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                _ = shutdown.cancelled() => return DiscoverOutcome::Shutdown,
            }
            delay_ms = (delay_ms * 2).min(8_000);
        }
    }
    tracing::error!(slug, "discovery failed after 5 attempts, skipping window");
    DiscoverOutcome::Failed
}

pub async fn run(
    settings: Config,
    enable_parquet: bool,
    enable_clickhouse: bool,
    enable_metrics: bool,
    shutdown: CancellationToken,
) -> Result<()> {
    tracing::info!("starting auto-ingest with automatic market rotation");

    // --- Metrics server (persistent) ---
    if enable_metrics {
        let metrics_addr: SocketAddr = settings
            .get_string("metrics.listen_addr")
            .unwrap_or_else(|_| "0.0.0.0:9090".to_string())
            .parse()?;
        let metrics_endpoint = settings
            .get_string("metrics.endpoint")
            .unwrap_or_else(|_| "/metrics".to_string());

        let handle = pb_metrics::install_recorder()
            .map_err(|e| anyhow::anyhow!("failed to install metrics recorder: {e}"))?;
        pb_metrics::register_metrics();

        let listener = tokio::net::TcpListener::bind(metrics_addr).await?;
        tracing::info!(%metrics_addr, endpoint = metrics_endpoint.as_str(), "metrics server bound");

        tokio::spawn(async move {
            if let Err(e) =
                pb_metrics::serve_metrics_on_listener(handle, listener, &metrics_endpoint).await
            {
                tracing::error!(error = %e, "metrics server failed");
            }
        });
    }

    // --- Persistent event channel ---
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
    let (active_assets_tx, active_assets_rx) = tokio::sync::watch::channel(Vec::<String>::new());

    let checkpoints_enabled = settings
        .get_bool("storage.checkpoints_enabled")
        .unwrap_or(true);
    let checkpoint_interval_secs = settings
        .get_int("storage.checkpoint_interval_secs")
        .unwrap_or(60) as u64;
    let checkpoint_handle = if checkpoints_enabled {
        let checkpoint_config = crate::commands::checkpoint_producer::CheckpointProducerConfig {
            rest_url: settings
                .get_string("feed.rest_url")
                .unwrap_or_else(|_| "https://clob.polymarket.com".to_string()),
            interval: Duration::from_secs(checkpoint_interval_secs),
            rate_limit_pause: Duration::from_millis(100),
        };
        Some(crate::commands::checkpoint_producer::spawn(
            checkpoint_config,
            active_assets_rx,
            event_tx.clone(),
            shutdown.child_token(),
        ))
    } else {
        None
    };

    // --- Storage sinks (persistent) ---
    let mut sink_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    let parquet_tx = if enable_parquet {
        let base_path = settings
            .get_string("storage.parquet_base_path")
            .unwrap_or_else(|_| "./data".to_string());
        let base_path = std::path::Path::new(&base_path)
            .canonicalize()
            .or_else(|_| {
                std::fs::create_dir_all(&base_path)?;
                std::path::Path::new(&base_path).canonicalize()
            })?
            .to_string_lossy()
            .to_string();
        let flush_secs = settings
            .get_int("storage.parquet_flush_interval_secs")
            .unwrap_or(300) as u64;

        let (ptx, prx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(10_000);
        let store: Arc<dyn object_store::ObjectStore> =
            Arc::new(object_store::local::LocalFileSystem::new());
        let sink = pb_store::ParquetSink::new(prx, store, base_path)
            .with_flush_interval(Duration::from_secs(flush_secs));
        sink_handles.push(tokio::spawn(async move {
            if let Err(e) = sink.run().await {
                tracing::error!(error = %e, "parquet sink failed");
            }
        }));
        Some(ptx)
    } else {
        None
    };

    let clickhouse_tx = if enable_clickhouse {
        let ch_url = settings
            .get_string("storage.clickhouse_url")
            .unwrap_or_else(|_| "http://localhost:8123".to_string());
        let ch_db = settings
            .get_string("storage.clickhouse_database")
            .unwrap_or_else(|_| "poly_book".to_string());

        let (ctx, crx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(10_000);
        let client = clickhouse::Client::default()
            .with_url(&ch_url)
            .with_database(&ch_db);
        let sink = pb_store::ClickHouseSink::new(crx, client);
        if let Err(e) = sink.ensure_table().await {
            tracing::warn!(error = %e, "failed to ensure ClickHouse table (will retry on insert)");
        }
        sink_handles.push(tokio::spawn(async move {
            if let Err(e) = sink.run().await {
                tracing::error!(error = %e, "clickhouse sink failed");
            }
        }));
        Some(ctx)
    } else {
        None
    };

    // --- Fan-out: per-sink forwarding tasks with bounded buffers absorb transient
    // sink slowdowns. Sustained backpressure from any sink will eventually stall
    // the event loop — this is intentional to preserve lossless delivery. ---
    let mut fanout_txs: Vec<tokio::sync::mpsc::Sender<pb_types::PersistedRecord>> = Vec::new();
    let mut fanout_fwd_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    if let Some(ptx) = parquet_tx.clone() {
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

    if let Some(ctx) = clickhouse_tx.clone() {
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
                Some(event) => match fanout_txs.as_slice() {
                    [] => {}
                    [a] => {
                        if let Err(e) = a.send(event).await {
                            tracing::warn!("fan-out channel closed: {e}");
                        }
                    }
                    [a, b] => {
                        let ev_a = event.clone();
                        let (ra, rb) = tokio::join!(a.send(ev_a), b.send(event));
                        if let Err(e) = ra {
                            tracing::warn!("fan-out channel 0 closed: {e}");
                        }
                        if let Err(e) = rb {
                            tracing::warn!("fan-out channel 1 closed: {e}");
                        }
                    }
                    _ => unreachable!("at most 2 sinks"),
                },
                None => {
                    tracing::info!("event channel closed, fan-out stopping");
                    break;
                }
            }
        }
    });

    // --- Build REST client for discovery ---
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

    // --- WS config ---
    let ws_config = pb_feed::WsConfig {
        ws_url: settings
            .get_string("feed.ws_url")
            .unwrap_or_else(|_| pb_feed::WsConfig::default().ws_url),
        ping_interval_secs: settings.get_int("feed.ping_interval_secs").unwrap_or(10) as u64,
        reconnect_base_delay_ms: settings
            .get_int("feed.reconnect_base_delay_ms")
            .unwrap_or(100) as u64,
        reconnect_max_delay_ms: settings
            .get_int("feed.reconnect_max_delay_ms")
            .unwrap_or(30000) as u64,
    };

    // --- Rotation loop ---
    let mut front_token: Option<CancellationToken> = None;
    let mut active_bucket: Option<u64> = None;

    tracing::info!("auto-ingest pipeline running, press Ctrl+C to stop");

    loop {
        let now_secs = current_unix_secs();
        let current_bucket = now_secs - (now_secs % 300);
        let next_bucket = current_bucket + 300;

        // Determine target: first iteration uses current, subsequent use next
        let target_bucket = if active_bucket.is_none() {
            current_bucket
        } else {
            next_bucket
        };

        // If already ingesting this bucket, sleep until the next boundary after it
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

        // On subsequent iterations, sleep until ~10s before the target boundary
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

        // Discover with retry
        let token_ids = match discover_with_retry(&rest, &target_slug, &shutdown).await {
            DiscoverOutcome::Found(ids) => ids,
            DiscoverOutcome::Shutdown => break,
            DiscoverOutcome::Failed => continue,
        };

        // Cancel old front-half
        if let Some(old) = front_token.take() {
            old.cancel();
            tokio::task::yield_now().await;
        }

        // Spawn new front-half (WsClient + Dispatcher) with new token IDs
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

    // --- Shutdown ---
    tracing::info!("shutting down auto-ingest pipeline");

    // Cancel front-half
    if let Some(old) = front_token.take() {
        old.cancel();
    }
    let _ = active_assets_tx.send(Vec::new());

    // Drop event_tx so fan-out task sees channel closed
    drop(event_tx);
    // Drop sink senders so sinks can finish draining
    drop(parquet_tx);
    drop(clickhouse_tx);

    // Wait for fan-out dispatcher to finish, then forwarding tasks, then sinks
    let timeout = Duration::from_secs(10);
    if tokio::time::timeout(timeout, fanout_handle).await.is_err() {
        tracing::warn!("fan-out task did not shut down within timeout");
    }
    for handle in fanout_fwd_handles {
        if tokio::time::timeout(timeout, handle).await.is_err() {
            tracing::warn!("fan-out forwarding task did not shut down within timeout");
        }
    }
    for handle in sink_handles {
        if tokio::time::timeout(timeout, handle).await.is_err() {
            tracing::warn!("sink did not shut down within timeout");
        }
    }
    if let Some(handle) = checkpoint_handle {
        if tokio::time::timeout(timeout, handle).await.is_err() {
            tracing::warn!("checkpoint producer did not shut down within timeout");
        }
    }

    tracing::info!("graceful shutdown complete");
    Ok(())
}
