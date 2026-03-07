use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{bail, Result};
use config::Config;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::market_discovery::{current_unix_secs, discover_with_retry, now_us, DiscoverOutcome};
use super::pipeline;

enum LiveMode {
    Fixed(Vec<String>),
    AutoRotate,
}

pub async fn run(
    settings: Config,
    tokens: Option<String>,
    auto_rotate: bool,
    enable_metrics: bool,
    shutdown: CancellationToken,
) -> Result<()> {
    let mode = parse_mode(tokens, auto_rotate)?;

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

    let feed_mode = match mode {
        LiveMode::Fixed(_) => pb_api::FeedMode::FixedTokens,
        LiveMode::AutoRotate => pb_api::FeedMode::AutoRotate,
    };
    let live = pb_api::LiveReadModel::new(feed_mode);
    let (event_tx, event_rx) = mpsc::channel::<pb_types::PersistedRecord>(2_048);
    let consumer_handle = live.spawn_consumer(event_rx, shutdown.child_token());

    let runtime_handle = match mode {
        LiveMode::Fixed(token_ids) => spawn_fixed_runtime(
            settings.clone(),
            token_ids,
            event_tx.clone(),
            live.clone(),
            shutdown.child_token(),
        ),
        LiveMode::AutoRotate => spawn_auto_rotate_runtime(
            settings.clone(),
            event_tx.clone(),
            live.clone(),
            shutdown.child_token(),
        ),
    };

    let state = pb_api::AppState {
        live,
        config: pb_api::ApiConfig {
            parquet_base_path,
            default_depth,
            max_depth,
            stale_after_secs,
        },
    };
    let listener = tokio::net::TcpListener::bind(api_listen_addr).await?;
    tracing::info!(%api_listen_addr, "api server bound");

    let serve_result = pb_api::serve(listener, state, shutdown.child_token()).await;
    drop(event_tx);
    pipeline::shutdown_handles(vec![runtime_handle, consumer_handle], "serve-api task").await;
    serve_result?;
    Ok(())
}

fn parse_mode(tokens: Option<String>, auto_rotate: bool) -> Result<LiveMode> {
    match (tokens, auto_rotate) {
        (Some(raw), false) => {
            let token_ids: Vec<String> = raw
                .split(',')
                .map(|token| token.trim())
                .filter(|token| !token.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            if token_ids.is_empty() {
                bail!("--tokens must contain at least one token ID");
            }
            Ok(LiveMode::Fixed(token_ids))
        }
        (None, true) => Ok(LiveMode::AutoRotate),
        (Some(_), true) => bail!("choose either --tokens or --auto-rotate, not both"),
        (None, false) => bail!("either --tokens or --auto-rotate is required"),
    }
}

fn spawn_fixed_runtime(
    settings: Config,
    token_ids: Vec<String>,
    event_tx: mpsc::Sender<pb_types::PersistedRecord>,
    live: pb_api::LiveReadModel,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        live.set_active_assets(token_ids.clone()).await;
        let ws_config = pipeline::ws_config_from_settings(&settings);
        let (raw_tx, raw_rx) = mpsc::channel::<pb_feed::FeedMessage>(2_048);

        let ws_client = match pb_feed::WsClient::new(token_ids, raw_tx) {
            Ok(client) => client.with_config(ws_config),
            Err(error) => {
                tracing::error!(error = %error, "failed to create websocket client");
                return;
            }
        };
        let ws_token = shutdown.child_token();
        let ws_handle = tokio::spawn(async move {
            if let Err(error) = ws_client.run_with_token(ws_token).await {
                tracing::error!(error = %error, "websocket client failed");
            }
        });

        let mut dispatcher = pb_feed::Dispatcher::new(raw_rx, event_tx);
        let dispatcher_token = shutdown.child_token();
        let dispatcher_handle = tokio::spawn(async move {
            if let Err(error) = dispatcher.run_with_token(dispatcher_token).await {
                tracing::error!(error = %error, "dispatcher failed");
            }
        });

        pipeline::shutdown_handles(vec![ws_handle, dispatcher_handle], "live runtime").await;
    })
}

fn spawn_auto_rotate_runtime(
    settings: Config,
    event_tx: mpsc::Sender<pb_types::PersistedRecord>,
    live: pb_api::LiveReadModel,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
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
        let mut child_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

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
                if !child_handles.is_empty() {
                    pipeline::shutdown_handles(
                        std::mem::take(&mut child_handles),
                        "auto-rotate child task",
                    )
                    .await;
                }
            }

            let (raw_tx, raw_rx) = mpsc::channel::<pb_feed::FeedMessage>(2_048);
            let new_token = shutdown.child_token();

            let ws_client = match pb_feed::WsClient::new(token_ids.clone(), raw_tx) {
                Ok(client) => client.with_config(ws_config.clone()),
                Err(error) => {
                    tracing::error!(error = %error, "failed to create websocket client");
                    continue;
                }
            };
            let ws_cancel = new_token.child_token();
            child_handles.push(tokio::spawn(async move {
                if let Err(error) = ws_client.run_with_token(ws_cancel).await {
                    tracing::error!(error = %error, "websocket client failed");
                }
            }));

            let mut dispatcher = pb_feed::Dispatcher::new(raw_rx, event_tx.clone());
            let dispatcher_cancel = new_token.child_token();
            child_handles.push(tokio::spawn(async move {
                if let Err(error) = dispatcher.run_with_token(dispatcher_cancel).await {
                    tracing::error!(error = %error, "dispatcher failed");
                }
            }));

            front_token = Some(new_token);
            active_bucket = Some(target_bucket);
            live.set_active_assets(token_ids.clone()).await;
            live.set_last_rotation_us(now_us()).await;
            pb_metrics::record_rotation();
            tracing::info!(slug = %target_slug, tokens = ?token_ids, "rotated serve-api market");
        }

        if let Some(old) = front_token.take() {
            old.cancel();
        }
        live.set_active_assets(Vec::new()).await;
        pipeline::shutdown_handles(child_handles, "auto-rotate child task").await;
    })
}
