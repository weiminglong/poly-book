use std::net::SocketAddr;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::feed::{run_feed, FeedPlan};
use super::market_discovery::now_us;
use super::pipeline;
use crate::settings::Settings;

pub async fn run(
    settings: Settings,
    plan: FeedPlan,
    enable_metrics: bool,
    shutdown: CancellationToken,
    slug_registry: pb_types::SlugRegistry,
) -> Result<()> {
    if enable_metrics {
        pipeline::start_metrics_server(&settings).await?;
    }

    let api_listen_addr: SocketAddr = settings.api.listen_addr;
    let default_depth = settings.api.default_depth;
    let parquet_base_path = settings.storage.parquet_base_path.clone();

    let feed_mode = match plan {
        FeedPlan::Fixed(_) => pb_api::FeedMode::FixedTokens,
        FeedPlan::AutoRotate => pb_api::FeedMode::AutoRotate,
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

    // The live feed runs as its own task; every active-set change is pushed
    // into the read model and the WS broadcast registry.
    let runtime_handle = {
        let settings = settings.clone();
        let event_tx = event_tx.clone();
        let live = live.clone();
        let broadcast = broadcast.clone();
        let shutdown = shutdown.child_token();
        let slug_registry = slug_registry.clone();
        tokio::spawn(async move {
            let on_active = Box::new(move |assets: Vec<String>| {
                let live = live.clone();
                let broadcast = broadcast.clone();
                Box::pin(async move {
                    broadcast.set_active_assets(&assets);
                    live.set_active_assets(assets).await;
                    live.set_last_rotation_us(now_us()).await;
                })
                    as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
            });
            if let Err(e) = run_feed(
                &settings,
                plan,
                event_tx,
                shutdown,
                slug_registry,
                on_active,
            )
            .await
            {
                tracing::error!(error = %e, "live feed runtime failed");
            }
        })
    };

    let historical = pipeline::build_historical_service(&settings).await?;
    let auth_token = settings.api_auth_token();
    pipeline::validate_api_auth_boundary(api_listen_addr, auth_token.as_deref())?;

    let state = pb_api::AppState {
        live,
        config: settings.api_config(parquet_base_path),
        broadcast: Some(broadcast.clone()),
        slug_registry,
        historical,
        wal_lag_bytes: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        needs_resync: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let listener = tokio::net::TcpListener::bind(api_listen_addr).await?;
    tracing::info!(%api_listen_addr, "api server bound");

    let serve_result = pb_api::serve(listener, state, shutdown.child_token()).await;
    drop(event_tx);
    pipeline::shutdown_handles(vec![runtime_handle, consumer_handle], "serve-api task").await;
    serve_result?;
    Ok(())
}
