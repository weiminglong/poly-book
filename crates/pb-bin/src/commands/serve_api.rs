use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{bail, Result};
use config::Config;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::market_discovery::{
    current_unix_secs, discover_with_retry, now_us, populate_registry, DiscoverOutcome,
};
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
    slug_registry: pb_types::SlugRegistry,
) -> Result<()> {
    let mode = parse_mode(tokens, auto_rotate)?;

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

    let feed_mode = match mode {
        LiveMode::Fixed(_) => pb_api::FeedMode::FixedTokens,
        LiveMode::AutoRotate => pb_api::FeedMode::AutoRotate,
    };
    let live = pb_api::LiveReadModel::new(feed_mode);
    let (event_tx, event_rx) = mpsc::channel::<pb_types::PersistedRecord>(2_048);
    let broadcast = pb_api::BookBroadcast::new();
    let consumer_handle = live.spawn_consumer_with_broadcast(
        event_rx,
        broadcast.clone(),
        default_depth,
        shutdown.child_token(),
    );

    // Feed-only mode: no checkpoint/WAL hydration, mark ready immediately.
    live.mark_hydrated().await;

    let runtime_handle = match mode {
        LiveMode::Fixed(token_ids) => spawn_fixed_runtime(
            settings.clone(),
            token_ids,
            event_tx.clone(),
            live.clone(),
            broadcast.clone(),
            shutdown.child_token(),
        ),
        LiveMode::AutoRotate => spawn_auto_rotate_runtime(
            settings.clone(),
            event_tx.clone(),
            live.clone(),
            broadcast.clone(),
            shutdown.child_token(),
            slug_registry.clone(),
        ),
    };

    let (replay_service, integrity_service, execution_service) =
        pipeline::build_services(&settings).await?;
    let effective_backend_is_clickhouse =
        matches!(&replay_service, pb_service::AnyReplayService::ClickHouse(_));
    let query_service =
        pipeline::build_query_service(&settings, effective_backend_is_clickhouse).await;
    let (query_max_rows, query_timeout_secs) = pipeline::query_config_from_settings(&settings);
    let auth_token = pipeline::api_auth_token_from_settings(&settings);

    // Optionally start gRPC server.
    let (grpc_enabled, grpc_addr) = pipeline::grpc_config_from_settings(&settings);
    pipeline::validate_api_auth_boundary(
        api_listen_addr,
        grpc_enabled,
        grpc_addr,
        auth_token.as_deref(),
    )?;
    let grpc_handle = if grpc_enabled {
        Some(
            pb_grpc::start_grpc_server(
                grpc_addr,
                replay_service.clone(),
                integrity_service.clone(),
                execution_service.clone(),
                max_depth,
                auth_token.clone(),
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
        broadcast: Some(broadcast.clone()),
        slug_registry,
        replay_service,
        integrity_service,
        execution_service,
        query_service,
        wal_lag_bytes: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        needs_resync: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let listener = tokio::net::TcpListener::bind(api_listen_addr).await?;
    tracing::info!(%api_listen_addr, "api server bound");

    let serve_result = pb_api::serve(listener, state, shutdown.child_token()).await;
    drop(event_tx);
    let mut handles = vec![runtime_handle, consumer_handle];
    if let Some(h) = grpc_handle {
        handles.push(h);
    }
    pipeline::shutdown_handles(handles, "serve-api task").await;
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
    broadcast: pb_api::PerAssetBroadcast,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        broadcast.set_active_assets(&token_ids);
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

        // Drain only after shutdown is requested: shutdown_handles applies a
        // 10s deadline per handle, so calling it while the runtime is healthy
        // logs spurious "did not shut down within timeout" warnings and stops
        // supervising the still-running tasks after the deadline passes.
        shutdown.cancelled().await;
        pipeline::shutdown_handles(vec![ws_handle, dispatcher_handle], "live runtime").await;
    })
}

fn spawn_auto_rotate_runtime(
    settings: Config,
    event_tx: mpsc::Sender<pb_types::PersistedRecord>,
    live: pb_api::LiveReadModel,
    broadcast: pb_api::PerAssetBroadcast,
    shutdown: CancellationToken,
    slug_registry: pb_types::SlugRegistry,
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
        let rest = match pb_feed::RestClient::new(rate_limiter) {
            Ok(c) => c.with_config(rest_config),
            Err(e) => {
                tracing::error!(error = %e, "failed to build REST client; auto-rotate cannot start");
                return;
            }
        };
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
            let discovery = match discover_with_retry(&rest, &target_slug, &shutdown).await {
                DiscoverOutcome::Found(result) => result,
                DiscoverOutcome::Shutdown => break,
                DiscoverOutcome::Failed => continue,
            };
            populate_registry(&slug_registry, &discovery);
            let token_ids = discovery.token_ids;

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
            broadcast.set_active_assets(&token_ids);
            live.set_active_assets(token_ids.clone()).await;
            live.set_last_rotation_us(now_us()).await;
            pb_metrics::record_rotation();
            tracing::info!(slug = %target_slug, tokens = ?token_ids, "rotated serve-api market");
        }

        if let Some(old) = front_token.take() {
            old.cancel();
        }
        broadcast.set_active_assets(&[]);
        live.set_active_assets(Vec::new()).await;
        pipeline::shutdown_handles(child_handles, "auto-rotate child task").await;
    })
}
