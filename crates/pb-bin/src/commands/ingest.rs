use anyhow::{bail, Result};
use config::Config;
use tokio_util::sync::CancellationToken;

use super::pipeline;

pub async fn run(
    settings: Config,
    tokens: Option<String>,
    enable_parquet: bool,
    enable_clickhouse: bool,
    enable_metrics: bool,
    shutdown: CancellationToken,
) -> Result<()> {
    let token_ids: Vec<String> = match tokens {
        Some(t) => t.split(',').map(|s| s.trim().to_string()).collect(),
        None => bail!("--tokens is required. Use 'discover' command to find token IDs."),
    };

    if token_ids.is_empty() {
        bail!("No token IDs provided");
    }

    tracing::info!(tokens = ?token_ids, "starting ingestion pipeline");

    if enable_metrics {
        pipeline::start_metrics_server(&settings).await?;
    }

    let ws_config = pipeline::ws_config_from_settings(&settings);

    let (raw_tx, raw_rx) = tokio::sync::mpsc::channel::<pb_feed::FeedMessage>(2_048);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
    let (_active_assets_tx, active_assets_rx) = tokio::sync::watch::channel(token_ids.clone());

    let ws_client = pb_feed::WsClient::new(token_ids, raw_tx)?.with_config(ws_config);
    let ws_token = shutdown.child_token();
    tokio::spawn(async move {
        if let Err(e) = ws_client.run_with_token(ws_token).await {
            tracing::error!(error = %e, "websocket client failed");
        }
    });

    let mut dispatcher = pb_feed::Dispatcher::new(raw_rx, event_tx.clone());
    let dispatcher_token = shutdown.child_token();
    tokio::spawn(async move {
        if let Err(e) = dispatcher.run_with_token(dispatcher_token).await {
            tracing::error!(error = %e, "dispatcher failed");
        }
    });

    let checkpoint_handle = pipeline::start_checkpoint_producer(
        &settings,
        active_assets_rx,
        event_tx.clone(),
        &shutdown,
    );

    let sinks = pipeline::start_storage_sinks(&settings, enable_parquet, enable_clickhouse).await?;

    tracing::info!("ingestion pipeline running, press Ctrl+C to stop");

    let mut fanout_txs: Vec<tokio::sync::mpsc::Sender<pb_types::PersistedRecord>> = Vec::new();
    let mut fanout_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    if let Some(ptx) = sinks.parquet_tx {
        let (ftx, mut frx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
        fanout_txs.push(ftx);
        let fwd_token = shutdown.child_token();
        fanout_handles.push(tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    biased;
                    _ = fwd_token.cancelled() => break,
                    event = frx.recv() => match event {
                        Some(e) => e,
                        None => break,
                    },
                };
                tokio::select! {
                    biased;
                    result = ptx.send(event) => {
                        if let Err(e) = result {
                            tracing::warn!("parquet sink send failed: {e}");
                            break;
                        }
                    }
                    _ = fwd_token.cancelled() => break,
                }
            }
        }));
    }

    if let Some(ctx) = sinks.clickhouse_tx {
        let (ftx, mut frx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
        fanout_txs.push(ftx);
        let fwd_token = shutdown.child_token();
        fanout_handles.push(tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    biased;
                    _ = fwd_token.cancelled() => break,
                    event = frx.recv() => match event {
                        Some(e) => e,
                        None => break,
                    },
                };
                tokio::select! {
                    biased;
                    result = ctx.send(event) => {
                        if let Err(e) = result {
                            tracing::warn!("clickhouse sink send failed: {e}");
                            break;
                        }
                    }
                    _ = fwd_token.cancelled() => break,
                }
            }
        }));
    }

    loop {
        match event_rx.recv().await {
            Some(event) => match fanout_txs.as_slice() {
                [] => {}
                [a] => {
                    if a.send(event).await.is_err() {
                        tracing::warn!("fan-out channel closed, stopping");
                        break;
                    }
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
                        break;
                    }
                }
                _ => unreachable!("at most 2 sinks"),
            },
            None => {
                tracing::info!("event channel closed, shutting down");
                break;
            }
        }
    }

    drop(fanout_txs);

    pipeline::shutdown_handles(fanout_handles, "fan-out task").await;
    pipeline::shutdown_handles(sinks.task_handles, "sink").await;
    if let Some(handle) = checkpoint_handle {
        pipeline::shutdown_handles(vec![handle], "checkpoint producer").await;
    }

    tracing::info!("graceful shutdown complete");
    Ok(())
}
