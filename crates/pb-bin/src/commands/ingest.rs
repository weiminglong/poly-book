use anyhow::{bail, Result};
use config::Config;
use pb_types::SlugRegistry;
use tokio_util::sync::CancellationToken;

use super::pipeline;
use super::pipeline::now_micros;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    settings: Config,
    tokens: Option<String>,
    enable_parquet: bool,
    enable_clickhouse: bool,
    enable_metrics: bool,
    standby: bool,
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
    // Channel for resnapshot requests the dispatcher raises on a book divergence
    // (A.74). Small + non-blocking: a backlog means a resnapshot is already
    // pending, so dropped requests are harmless.
    let (resnapshot_tx, resnapshot_rx) = tokio::sync::mpsc::channel::<std::sync::Arc<str>>(64);

    // All long-lived background tasks are registered with a supervisor so that
    // an unexpected exit (a sink failing, the feed dying, a panic) is detected
    // and turned into a coordinated, non-zero-exit shutdown instead of leaving
    // the pipeline running with a dead component or exiting 0 (task supervision
    // was previously absent — the #2 audit finding).
    let mut supervisor = pipeline::Supervisor::new();

    let ws_client = pb_feed::WsClient::new(token_ids, raw_tx.clone())?.with_config(ws_config);
    let ws_token = shutdown.child_token();
    supervisor.spawn("websocket", async move {
        if let Err(e) = ws_client.run_with_token(ws_token).await {
            tracing::error!(error = %e, "websocket client failed");
        }
    });

    let mut dispatcher =
        pb_feed::Dispatcher::new(raw_rx, event_tx.clone()).with_resnapshot_tx(resnapshot_tx);
    let dispatcher_token = shutdown.child_token();
    supervisor.spawn("dispatcher", async move {
        if let Err(e) = dispatcher.run_with_token(dispatcher_token).await {
            tracing::error!(error = %e, "dispatcher failed");
        }
    });

    // Self-healing worker: on a detected book divergence, fetch a fresh REST
    // snapshot and re-inject it into the feed (A.74). It reuses the dispatcher's
    // normal snapshot path via the shared raw channel.
    let resnapshot_rest = pb_feed::RestClient::new(pipeline::rest_rate_limiter(&settings))
        .with_config(pipeline::rest_config_from_settings(&settings));
    let resnapshot_token = shutdown.child_token();
    supervisor.spawn("resnapshot-worker", async move {
        pb_feed::run_resnapshot_worker(resnapshot_rest, raw_tx, resnapshot_rx, resnapshot_token)
            .await;
    });

    pipeline::start_checkpoint_producer(
        &settings,
        active_assets_rx,
        event_tx.clone(),
        &shutdown,
        &mut supervisor,
    );

    let sinks = pipeline::start_storage_sinks(
        &settings,
        enable_parquet,
        enable_clickhouse,
        &mut supervisor,
    )
    .await?;

    // Open WAL writer for durable event streaming.
    // The writer is only accessed on this task's event loop so no Arc/Mutex needed.
    let wal_config = pipeline::wal_config_from_settings(&settings);
    let wal_flush_interval = std::time::Duration::from_millis(wal_config.flush_interval_ms);
    let wal_sync_interval = std::time::Duration::from_millis(wal_config.sync_interval_ms);
    let wal_base_path = wal_config.base_path.clone();
    // The WAL is the durability backbone: a failure to open it is fatal, not a
    // "continue without WAL" warning that silently disables durability (A.129).
    // With --standby, a process started against a WAL already held by an active
    // writer waits and auto-promotes when that writer exits (P3-HA-1) instead of
    // failing fast on the single-writer flock.
    if standby {
        tracing::info!("standby mode: will wait for the active writer's WAL lock before ingesting");
    }
    let Some(mut wal_writer) = pipeline::open_wal_writer_with_standby(
        wal_config,
        standby,
        std::time::Duration::from_secs(1),
        &shutdown,
    )
    .await?
    else {
        // shutdown fired while a standby was still waiting to promote
        return Ok(());
    };
    tracing::info!("WAL writer opened");

    tracing::info!("ingestion pipeline running, press Ctrl+C to stop");

    let mut fanout_txs: Vec<tokio::sync::mpsc::Sender<pb_types::PersistedRecord>> = Vec::new();

    if let Some(ptx) = sinks.parquet_tx {
        let (ftx, mut frx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
        fanout_txs.push(ftx);
        supervisor.spawn("parquet-fanout", async move {
            while let Some(event) = frx.recv().await {
                if let Err(e) = ptx.send(event).await {
                    tracing::warn!("parquet sink send failed: {e}");
                    break;
                }
            }
        });
    }

    if let Some(ctx) = sinks.clickhouse_tx {
        let (ftx, mut frx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(2_048);
        fanout_txs.push(ftx);
        supervisor.spawn("clickhouse-fanout", async move {
            while let Some(event) = frx.recv().await {
                if let Err(e) = ctx.send(event).await {
                    tracing::warn!("clickhouse sink send failed: {e}");
                    break;
                }
            }
        });
    }

    // Drop the original sender so the channel closes when dispatcher stops.
    drop(event_tx);

    // Steady-state durability cadence: flush the BufWriter frequently so a
    // tailing serve reader sees records promptly, and fdatasync less often so
    // the OS-crash data-loss window is bounded to ~one sync interval (A.11/A.29).
    let mut flush_tick = tokio::time::interval(wal_flush_interval);
    let mut sync_tick = tokio::time::interval(wal_sync_interval);
    // Periodically reclaim WAL segments all consumers have advanced past so disk
    // usage stays bounded under 24/7 ingest (A.17/A.20/A.47).
    let mut prune_tick = tokio::time::interval(std::time::Duration::from_secs(60));
    flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    sync_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    prune_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut wal_unflushed = false;
    let mut wal_unsynced = false;
    // Monotonic ingest ordinal stamped on every record at this single
    // serialization point, giving replay a total order that respects true
    // arrival even when per-asset `sequence` resets on snapshots (A.116).
    let mut ingest_ordinal: u64 = 0;
    // Set if a supervised background task exits unexpectedly: ingestion cannot
    // safely continue with a dead feed/dispatcher/sink, so we shut down and
    // propagate a non-zero exit instead of returning Ok.
    let mut supervision_failure: Option<&'static str> = None;
    // Receive timestamp (µs) of the most recent record, used to publish the feed
    // staleness gauge on the periodic flush tick so it RISES during feed silence
    // (HFT-review: the gauge was defined + alerted on but never populated, so the
    // FeedStale alert could never fire). 0 until the first record, so a slow
    // startup connect does not report false staleness.
    let mut last_recv_us: u64 = 0;

    loop {
        let mut event = tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            exited = supervisor.next_exit(), if !supervisor.is_empty() => {
                let name = exited.unwrap_or("<unknown>");
                tracing::error!(
                    component = name,
                    "supervised task exited unexpectedly; shutting down ingest"
                );
                supervision_failure = Some(name);
                shutdown.cancel();
                break;
            }
            _ = flush_tick.tick() => {
                // Sample the event-channel backpressure depth (A.79).
                pb_metrics::set_channel_depth("ingest_events", event_rx.len());
                // Publish feed staleness so the FeedStale alert can fire. Only
                // once a record has been seen, so startup is not flagged stale.
                if last_recv_us > 0 {
                    let staleness_s =
                        now_micros().saturating_sub(last_recv_us) as f64 / 1_000_000.0;
                    pb_metrics::set_feed_staleness_seconds(staleness_s);
                }
                if wal_unflushed {
                    wal_writer.flush()
                        .map_err(|e| anyhow::anyhow!("WAL flush failed: {e}"))?;
                    wal_unflushed = false;
                }
                continue;
            }
            _ = sync_tick.tick() => {
                if wal_unsynced {
                    // fdatasync is a blocking syscall (ms-scale, worse on busy
                    // disks/EFS). block_in_place tells the multi-threaded runtime to
                    // relocate other tasks off this worker for the syscall's
                    // duration so it does not tie up a runtime thread (HFT-review
                    // #8). The single-writer flock invariant is preserved — the
                    // writer is still owned solely by this task.
                    tokio::task::block_in_place(|| wal_writer.sync())
                        .map_err(|e| anyhow::anyhow!("WAL sync failed: {e}"))?;
                    wal_unsynced = false;
                    wal_unflushed = false;
                }
                continue;
            }
            _ = prune_tick.tick() => {
                let consumers = pipeline::wal_consumer_position_files(&wal_base_path);
                // prune does read_dir + per-segment metadata + remove_file (blocking
                // FS syscalls); run it via block_in_place so it does not stall the
                // runtime worker (HFT-review #9).
                if let Err(e) =
                    tokio::task::block_in_place(|| wal_writer.prune_with_backpressure(&consumers))
                {
                    tracing::warn!(error = %e, "WAL prune failed");
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
        // Track the most recent receive time for the feed-staleness gauge.
        if let Some(recv) = event.recv_timestamp_us() {
            last_recv_us = last_recv_us.max(recv);
        }
        // Stamp the monotonic ingest ordinal before persistence so replay can
        // order same-microsecond events by true arrival (A.116).
        if let Some(prov) = event.provenance_mut() {
            prov.ingest_ordinal = Some(ingest_ordinal);
            ingest_ordinal += 1;
        }
        // Stamp the WAL offset onto checkpoints just before they are written, so
        // a serve cold start can resume WAL tailing from the checkpoint instead
        // of replaying the entire retained WAL (A.13/A.52 — checkpoints were
        // always persisted with wal_offset = NULL).
        if let pb_types::PersistedRecord::Checkpoint(ref mut checkpoint) = event {
            checkpoint.wal_offset = Some(wal_writer.global_offset());
        }
        // Write to WAL before fan-out to sinks. Encode failure skips the whole
        // record (both WAL and sinks) so the WAL and the storage datasets never
        // diverge; append failure is fatal — the durability backbone is gone.
        match pb_wal::codec::encode(&event) {
            Ok(payload) => {
                if let Err(e) = wal_writer.append(&payload) {
                    pb_metrics::record_wal_append_failure();
                    return Err(anyhow::anyhow!("WAL append failed: {e}"));
                }
                wal_unflushed = true;
                wal_unsynced = true;
                // Record end-to-end recv→durable(WAL append) latency (A.113).
                if let Some(recv) = event.recv_timestamp_us() {
                    if recv > 0 {
                        pb_metrics::record_recv_to_durable_us(now_micros().saturating_sub(recv));
                    }
                }
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

    // Closing the fan-out senders lets the fan-out tasks drain and exit; the
    // shutdown token (cancelled on either Ctrl+C or a supervised-task death)
    // stops the sinks, checkpoint producer, feed, and dispatcher.
    drop(fanout_txs);
    if supervision_failure.is_none() {
        shutdown.cancel();
    }

    supervisor.join_all("ingest").await;

    if let Some(component) = supervision_failure {
        return Err(anyhow::anyhow!(
            "ingest aborting: supervised task '{component}' exited unexpectedly"
        ));
    }

    tracing::info!("graceful shutdown complete");
    Ok(())
}
