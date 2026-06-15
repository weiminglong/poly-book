use anyhow::{bail, Result};
use config::Config;
use pb_types::SlugRegistry;
use tokio_util::sync::CancellationToken;

use super::pipeline;

pub async fn run(
    settings: Config,
    tokens: Option<String>,
    enable_parquet: bool,
    enable_clickhouse: bool,
    enable_metrics: bool,
    shutdown: CancellationToken,
    slug_registry: SlugRegistry,
) -> Result<()> {
    let raw_inputs: Vec<String> = match tokens {
        Some(t) => t.split(',').map(|s| s.trim().to_string()).collect(),
        None => bail!("--tokens is required. Use 'discover' command to find token IDs."),
    };
    let token_ids: Vec<String> = raw_inputs
        .into_iter()
        .map(|input| {
            slug_registry
                .resolve(&input)
                .map(|id| id.to_string())
                .unwrap_or(input)
        })
        .collect();

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

    // Open WAL writer for durable event streaming.
    // The writer is only accessed on this task's event loop so no Arc/Mutex needed.
    let wal_config = pipeline::wal_config_from_settings(&settings);
    let wal_flush_interval = std::time::Duration::from_millis(wal_config.flush_interval_ms);
    let wal_sync_interval = std::time::Duration::from_millis(wal_config.sync_interval_ms);
    // The WAL is the durability backbone: a failure to open it is fatal, not a
    // "continue without WAL" warning that silently disables durability (A.129).
    let mut wal_writer = pb_wal::WalWriter::open(wal_config)
        .map_err(|e| anyhow::anyhow!("failed to open WAL writer: {e}"))?;
    tracing::info!("WAL writer opened");

    tracing::info!("ingestion pipeline running, press Ctrl+C to stop");

    let mut fanout_txs: Vec<tokio::sync::mpsc::Sender<pb_types::PersistedRecord>> = Vec::new();
    let mut fanout_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    if let Some(ptx) = sinks.parquet_tx {
        let (ftx, mut frx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
        fanout_txs.push(ftx);
        fanout_handles.push(tokio::spawn(async move {
            while let Some(event) = frx.recv().await {
                if let Err(e) = ptx.send(event).await {
                    tracing::warn!("parquet sink send failed: {e}");
                    break;
                }
            }
        }));
    }

    if let Some(ctx) = sinks.clickhouse_tx {
        let (ftx, mut frx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
        fanout_txs.push(ftx);
        fanout_handles.push(tokio::spawn(async move {
            while let Some(event) = frx.recv().await {
                if let Err(e) = ctx.send(event).await {
                    tracing::warn!("clickhouse sink send failed: {e}");
                    break;
                }
            }
        }));
    }

    // Drop the original sender so the channel closes when dispatcher stops.
    drop(event_tx);

    // Steady-state durability cadence: flush the BufWriter frequently so a
    // tailing serve reader sees records promptly, and fdatasync less often so
    // the OS-crash data-loss window is bounded to ~one sync interval (A.11/A.29).
    let mut flush_tick = tokio::time::interval(wal_flush_interval);
    let mut sync_tick = tokio::time::interval(wal_sync_interval);
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    sync_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut wal_unflushed = false;
    let mut wal_unsynced = false;

    loop {
        let event = tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = flush_tick.tick() => {
                if wal_unflushed {
                    wal_writer.flush()
                        .map_err(|e| anyhow::anyhow!("WAL flush failed: {e}"))?;
                    wal_unflushed = false;
                }
                continue;
            }
            _ = sync_tick.tick() => {
                if wal_unsynced {
                    wal_writer.sync()
                        .map_err(|e| anyhow::anyhow!("WAL sync failed: {e}"))?;
                    wal_unsynced = false;
                    wal_unflushed = false;
                }
                continue;
            }
            event = event_rx.recv() => match event {
                Some(e) => e,
                None => {
                    tracing::info!("event channel closed, shutting down");
                    break;
                }
            },
        };
        // Write to WAL before fan-out to sinks. Encode failure skips the whole
        // record (both WAL and sinks) so the WAL and the storage datasets never
        // diverge; append failure is fatal — the durability backbone is gone.
        match pb_wal::codec::encode(&event) {
            Ok(payload) => {
                wal_writer
                    .append(&payload)
                    .map_err(|e| anyhow::anyhow!("WAL append failed: {e}"))?;
                wal_unflushed = true;
                wal_unsynced = true;
            }
            Err(e) => {
                tracing::error!(error = %e, "WAL encode failed, dropping record from both WAL and sinks");
                continue;
            }
        }
        if !pipeline::fanout_event(event, &fanout_txs).await {
            break;
        }
    }

    // Drain events still buffered in the channel at shutdown so a graceful stop
    // does not discard records the dispatcher already produced (A.44/A.97). The
    // feed and dispatcher share the shutdown token and are stopping, so this
    // drains the backlog rather than waiting for new events.
    // try_recv returns Err (Empty or Disconnected) once the backlog is drained,
    // which ends the loop.
    while let Ok(event) = event_rx.try_recv() {
        match pb_wal::codec::encode(&event) {
            Ok(payload) => {
                wal_writer
                    .append(&payload)
                    .map_err(|e| anyhow::anyhow!("WAL append failed during drain: {e}"))?;
                let _ = pipeline::fanout_event(event, &fanout_txs).await;
            }
            Err(e) => {
                tracing::error!(error = %e, "WAL encode failed during drain, dropping record");
            }
        }
    }

    // Durably flush + fsync the WAL before shutdown so no acknowledged record
    // is lost on a clean stop.
    if let Err(e) = wal_writer.sync() {
        tracing::error!(error = %e, "WAL sync failed during shutdown");
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
