//! `ingest`: venue feed → dispatcher → WAL → Parquet sink, for a fixed token set
//! or the rotating BTC 5-minute market.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use pb_types::SlugRegistry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::feed::{run_feed, FeedPlan};
use super::pipeline;
use super::pipeline::now_micros;
use crate::settings::Settings;

pub async fn run(
    settings: Settings,
    plan: FeedPlan,
    enable_parquet: bool,
    enable_metrics: bool,
    standby: bool,
    shutdown: CancellationToken,
    slug_registry: SlugRegistry,
) -> Result<()> {
    tracing::info!(?plan, "starting ingestion pipeline");

    if enable_metrics {
        pipeline::start_metrics_server(&settings).await?;
    }

    let (event_tx, event_rx) = mpsc::channel::<pb_types::PersistedRecord>(2_048);
    let (active_assets_tx, active_assets_rx) = tokio::sync::watch::channel(Vec::<String>::new());

    // Long-lived infrastructure tasks (sink, checkpoint producer, WAL drain) are
    // supervised so an unexpected exit becomes a coordinated, non-zero-exit
    // shutdown instead of a silent storage gap. The per-market feed tasks are
    // not supervised: their cycling is the expected steady state under rotation.
    let mut supervisor = pipeline::Supervisor::new();

    pipeline::start_checkpoint_producer(
        &settings,
        active_assets_rx,
        event_tx.clone(),
        &shutdown,
        &mut supervisor,
    );
    let sinks = pipeline::start_storage_sinks(&settings, enable_parquet, &mut supervisor).await?;

    // The WAL is the durability backbone: failing to open it is fatal. With
    // --standby, a process started against a WAL held by an active writer waits
    // and auto-promotes when that writer exits instead of failing fast.
    let wal_config = settings.wal_config();
    if standby {
        tracing::info!("standby mode: will wait for the active writer's WAL lock before ingesting");
    }
    let Some(wal_writer) = pipeline::open_wal_writer_with_standby(
        wal_config.clone(),
        standby,
        Duration::from_secs(1),
        &shutdown,
    )
    .await?
    else {
        // Shutdown fired while a standby was still waiting to promote.
        return Ok(());
    };
    tracing::info!("WAL writer opened");

    // Set when the drain task hits a fatal WAL error so run() exits non-zero and
    // a process supervisor restarts us instead of limping on without durability.
    let wal_failed = Arc::new(AtomicBool::new(false));
    supervisor.spawn(
        "wal-drain",
        wal_drain(
            wal_writer,
            wal_config,
            event_rx,
            sinks.parquet_tx.into_iter().collect(),
            shutdown.clone(),
            wal_failed.clone(),
        ),
    );

    tracing::info!("ingestion pipeline running, press Ctrl+C to stop");

    let feed = run_feed(
        &settings,
        plan,
        event_tx.clone(),
        shutdown.clone(),
        slug_registry,
        Box::new(move |assets| {
            let _ = active_assets_tx.send(assets);
            Box::pin(async {})
        }),
    );
    tokio::pin!(feed);

    let mut supervision_failure: Option<&'static str> = None;
    let feed_result = tokio::select! {
        result = &mut feed => result,
        exited = supervisor.next_exit(), if !supervisor.is_empty() => {
            let name = exited.unwrap_or("<unknown>");
            tracing::error!(component = name, "supervised task exited unexpectedly; shutting down ingest");
            supervision_failure = Some(name);
            shutdown.cancel();
            feed.await
        }
    };

    // Close the event channel so the drain reaches EOF after writing the backlog,
    // then join every supervised task with a bounded timeout.
    shutdown.cancel();
    drop(event_tx);
    supervisor.join_all("ingest").await;

    feed_result?;
    if wal_failed.load(Ordering::SeqCst) {
        anyhow::bail!("ingest aborted: WAL durability failure");
    }
    if let Some(component) = supervision_failure {
        anyhow::bail!("ingest aborting: supervised task '{component}' exited unexpectedly");
    }
    tracing::info!("graceful shutdown complete");
    Ok(())
}

/// The single serialization point for every record: stamp the ingest ordinal,
/// append to the WAL, then fan out to the storage sinks. Runs until the event
/// channel closes, so a graceful stop drains the backlog the dispatcher already
/// produced.
async fn wal_drain(
    mut wal_writer: pb_wal::WalWriter,
    wal_config: pb_wal::WalConfig,
    mut event_rx: mpsc::Receiver<pb_types::PersistedRecord>,
    sink_txs: Vec<mpsc::Sender<pb_types::PersistedRecord>>,
    shutdown: CancellationToken,
    wal_failed: Arc<AtomicBool>,
) {
    // Flush the BufWriter frequently so a tailing serve reader sees records
    // promptly, and fdatasync less often so the OS-crash loss window is bounded
    // to about one sync interval. Prune reclaims segments all consumers have
    // advanced past so disk stays bounded under 24/7 ingest.
    let mut flush_tick = tokio::time::interval(Duration::from_millis(wal_config.flush_interval_ms));
    let mut sync_tick = tokio::time::interval(Duration::from_millis(wal_config.sync_interval_ms));
    let mut prune_tick = tokio::time::interval(Duration::from_secs(60));
    for tick in [&mut flush_tick, &mut sync_tick, &mut prune_tick] {
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    }
    let mut wal_unflushed = false;
    let mut wal_unsynced = false;
    // Monotonic ingest ordinal: this loop is the single serialization point for
    // all market generations, so replay gets a true-arrival total order even
    // though per-asset `sequence` resets on snapshots.
    let mut ingest_ordinal: u64 = 0;
    // Receive timestamp of the newest record, for the feed-staleness gauge.
    let mut last_recv_us: u64 = 0;

    let fail = |e: &dyn std::fmt::Display, what: &str| {
        tracing::error!(error = %e, "WAL {what} failed; aborting ingest");
        wal_failed.store(true, Ordering::SeqCst);
        shutdown.cancel();
    };

    loop {
        let mut event = tokio::select! {
            biased;
            _ = prune_tick.tick() => {
                // Blocking FS syscalls run via block_in_place so they do not
                // stall the runtime worker.
                let consumers = pipeline::wal_consumer_position_files(&wal_config.base_path);
                if let Err(e) = tokio::task::block_in_place(|| {
                    wal_writer.prune_with_backpressure(&consumers)
                }) {
                    tracing::warn!(error = %e, "WAL prune failed");
                }
                continue;
            }
            _ = flush_tick.tick() => {
                pb_metrics::set_channel_depth("ingest_events", event_rx.len());
                if last_recv_us > 0 {
                    let staleness_s =
                        now_micros().saturating_sub(last_recv_us) as f64 / 1_000_000.0;
                    pb_metrics::set_feed_staleness_seconds(staleness_s);
                }
                if wal_unflushed {
                    if let Err(e) = wal_writer.flush() {
                        fail(&e, "flush");
                        break;
                    }
                    wal_unflushed = false;
                }
                continue;
            }
            _ = sync_tick.tick() => {
                if wal_unsynced {
                    // fdatasync is a ms-scale blocking syscall.
                    if let Err(e) = tokio::task::block_in_place(|| wal_writer.sync()) {
                        fail(&e, "sync");
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
                    tracing::info!("event channel closed, WAL drain stopping");
                    break;
                }
            },
        };

        if let Some(recv) = event.recv_timestamp_us() {
            last_recv_us = last_recv_us.max(recv);
        }
        if let Some(prov) = event.provenance_mut() {
            prov.ingest_ordinal = Some(ingest_ordinal);
            ingest_ordinal += 1;
        }
        // Stamp the WAL offset onto checkpoints just before writing so serve can
        // resume tailing from the checkpoint instead of the whole retained WAL.
        if let pb_types::PersistedRecord::Checkpoint(ref mut checkpoint) = event {
            checkpoint.wal_offset = Some(wal_writer.global_offset());
        }
        // WAL first, then fan-out. An encode failure drops the record from both
        // so the WAL and the storage datasets never diverge; an append failure
        // is fatal because the durability backbone is gone.
        match pb_wal::codec::encode(&event) {
            Ok(payload) => {
                if let Err(e) = wal_writer.append(&payload) {
                    pb_metrics::record_wal_append_failure();
                    fail(&e, "append");
                    break;
                }
                wal_unflushed = true;
                wal_unsynced = true;
                if let Some(recv) = event.recv_timestamp_us() {
                    if recv > 0 {
                        pb_metrics::record_recv_to_durable_us(now_micros().saturating_sub(recv));
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "WAL encode failed, dropping record from WAL and sinks");
                continue;
            }
        }
        if !pipeline::fanout_event(event, &sink_txs).await {
            break;
        }
    }

    // Durably flush + fsync whatever is buffered before the process stops.
    if let Err(e) = wal_writer.sync() {
        tracing::error!(error = %e, "WAL sync failed during shutdown");
    }
}
