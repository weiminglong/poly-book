use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::settings::Settings;
use anyhow::{bail, Result};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Current wall-clock time in microseconds since the Unix epoch, for measuring
/// recv→durable latency. Saturates to 0 before the epoch (never in practice).
pub fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

pub async fn start_metrics_server(settings: &Settings) -> Result<()> {
    let metrics_addr = settings.metrics.listen_addr;
    let metrics_endpoint = settings.metrics.endpoint.clone();

    let handle = pb_metrics::install_recorder()
        .map_err(|e| anyhow::anyhow!("failed to install metrics recorder: {e}"))?;
    pb_metrics::register_metrics();

    // Drain idle histogram state periodically so bucket memory stays bounded even
    // if scraping stalls.
    pb_metrics::spawn_upkeep(handle.clone(), Duration::from_secs(5));

    let listener = tokio::net::TcpListener::bind(metrics_addr).await?;
    tracing::info!(%metrics_addr, endpoint = metrics_endpoint.as_str(), "metrics server bound");

    tokio::spawn(async move {
        if let Err(e) =
            pb_metrics::serve_metrics_on_listener(handle, listener, &metrics_endpoint).await
        {
            tracing::error!(error = %e, "metrics server failed");
        }
    });

    Ok(())
}

pub struct SinkHandles {
    pub parquet_tx: Option<mpsc::Sender<pb_types::PersistedRecord>>,
}

/// Supervises long-lived background tasks. Each task is tagged with a stable
/// name; if any exits — returns, errors, or panics — before a coordinated
/// shutdown, the supervisor reports its name so the owning command can treat the
/// exit as fatal (cancel the shutdown token and return a non-zero error)
/// instead of silently continuing with a dead component or exiting 0.
///
/// This addresses a gap where there was no task supervision anywhere:
/// a single transient sink error caused its task to end while the ingest loop
/// kept running and the process ultimately exited 0, masking real data loss.
#[derive(Default)]
pub struct Supervisor {
    tasks: tokio::task::JoinSet<&'static str>,
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            tasks: tokio::task::JoinSet::new(),
        }
    }

    /// Spawn a supervised task. The task runs `fut` to completion; the
    /// supervisor records `name` so it can report which component exited.
    pub fn spawn<F>(&mut self, name: &'static str, fut: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn(async move {
            fut.await;
            name
        });
    }

    /// True when no supervised tasks remain. Use as a `select!` precondition so
    /// an empty supervisor never busy-loops returning `None`.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Wait for the next supervised task to exit. Returns the task's name, or
    /// `"<panicked>"` if it panicked or was cancelled. Returns `None` only when
    /// the supervisor holds no tasks.
    pub async fn next_exit(&mut self) -> Option<&'static str> {
        match self.tasks.join_next().await {
            Some(Ok(name)) => Some(name),
            Some(Err(e)) => {
                tracing::error!(error = %e, "supervised task panicked");
                Some("<panicked>")
            }
            None => None,
        }
    }

    /// Await all remaining tasks during a coordinated shutdown, logging any that
    /// panicked. Call after cancelling the shutdown token. Bounded so a hung
    /// task cannot block shutdown forever.
    pub async fn join_all(mut self, label: &str) {
        let deadline = Duration::from_secs(10);
        loop {
            match tokio::time::timeout(deadline, self.tasks.join_next()).await {
                Ok(Some(Ok(_name))) => {}
                Ok(Some(Err(e))) if !e.is_cancelled() => {
                    tracing::error!(error = %e, "{label} task panicked during shutdown");
                }
                Ok(Some(Err(_))) => {}
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!("{label} tasks did not all shut down within timeout");
                    self.tasks.abort_all();
                    break;
                }
            }
        }
    }
}

/// Build the object store and path prefix for a configured storage base path.
///
/// An `s3://bucket/prefix` or `file://...` URL is wired to the matching compiled
/// `object_store` backend; a plain path is a local filesystem directory. This
/// fixes a bug where an
/// `s3://...` base path was silently handled by `LocalFileSystem` and written to
/// a local directory literally named `s3:` on ephemeral container storage.
///
/// For `s3://`, configuration is taken from process
/// environment variables via `object_store::parse_url_opts(url, std::env::vars())`.
/// This is what wires in region and credentials: `AWS_REGION`, static
/// `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, or — when no static keys are set —
/// the default AWS provider chain (ECS task role / instance profile). It also
/// honors `AWS_ENDPOINT` (+ `AWS_ALLOW_HTTP`, `AWS_VIRTUAL_HOSTED_STYLE_REQUEST`)
/// so an S3-compatible endpoint (MinIO/LocalStack) can be targeted. Plain
/// `object_store::parse_url` passes *no* options, so the builder would have no
/// region/credentials/endpoint and fail against real S3 — hence `parse_url_opts`.
pub fn build_object_store(base_path: &str) -> Result<(Arc<dyn object_store::ObjectStore>, String)> {
    if base_path.contains("://") {
        let url = url::Url::parse(base_path)
            .map_err(|e| anyhow::anyhow!("invalid storage URL {base_path}: {e}"))?;
        let (store, prefix) = object_store::parse_url_opts(&url, std::env::vars())
            .map_err(|e| anyhow::anyhow!("failed to build object store for {base_path}: {e}"))?;
        Ok((Arc::from(store), prefix.to_string()))
    } else {
        // Local filesystem: canonicalize/create the dir and use the absolute
        // path as the object-path prefix on a root LocalFileSystem.
        let abs = std::path::Path::new(base_path)
            .canonicalize()
            .or_else(|_| {
                std::fs::create_dir_all(base_path)?;
                std::path::Path::new(base_path).canonicalize()
            })?
            .to_string_lossy()
            .to_string();
        Ok((Arc::new(object_store::local::LocalFileSystem::new()), abs))
    }
}

pub(crate) fn object_store_error_summary(error: &object_store::Error) -> &'static str {
    match error {
        object_store::Error::NotFound { .. } => "object or bucket not found",
        object_store::Error::PermissionDenied { .. } => "permission denied",
        object_store::Error::Unauthenticated { .. } => "authentication failed",
        object_store::Error::InvalidPath { .. } => "invalid object path",
        object_store::Error::NotSupported { .. } | object_store::Error::NotImplemented { .. } => {
            "operation not supported"
        }
        object_store::Error::AlreadyExists { .. } => "object already exists",
        object_store::Error::Precondition { .. } => "request precondition failed",
        object_store::Error::NotModified { .. } => "object not modified",
        object_store::Error::UnknownConfigurationKey { .. } => "invalid backend configuration",
        _ => "backend request failed",
    }
}

/// Fail startup when the configured historical object store cannot even list
/// its prefix. An empty prefix is valid; auth, endpoint, or bucket failures are
/// not. Reads will otherwise look like an empty dataset and readiness can turn
/// green on a misconfigured deployment.
async fn probe_object_store_read(
    store: &Arc<dyn object_store::ObjectStore>,
    prefix: &str,
) -> Result<()> {
    let path = object_store::path::Path::parse(prefix)
        .map_err(|error| anyhow::anyhow!("invalid object-store prefix {prefix}: {error}"))?;
    let mut objects = store.list(Some(&path));
    match tokio::time::timeout(Duration::from_secs(10), objects.next()).await {
        Ok(Some(Ok(_))) | Ok(None) => Ok(()),
        Ok(Some(Err(error))) => Err(anyhow::anyhow!(
            "cannot list configured Parquet prefix {prefix}: {}",
            object_store_error_summary(&error)
        )),
        Err(_) => Err(anyhow::anyhow!(
            "timed out listing configured Parquet prefix {prefix} after 10s"
        )),
    }
}

pub async fn start_storage_sinks(
    settings: &Settings,
    enable_parquet: bool,
    supervisor: &mut Supervisor,
) -> Result<SinkHandles> {
    let parquet_tx = if enable_parquet {
        let (store, base_path) = build_object_store(&settings.storage.parquet_base_path)?;
        let flush_secs = settings.storage.parquet_flush_interval_secs;

        let (ptx, prx) = mpsc::channel::<pb_types::PersistedRecord>(10_000);
        let sink = pb_store::ParquetSink::new(prx, store, base_path)
            .with_flush_interval(Duration::from_secs(flush_secs));
        supervisor.spawn("parquet-sink", async move {
            if let Err(e) = sink.run().await {
                tracing::error!(error = %e, "parquet sink failed");
            }
        });
        Some(ptx)
    } else {
        None
    };

    Ok(SinkHandles { parquet_tx })
}

/// Spawn the checkpoint producer into the supervisor if enabled. Returns `true`
/// if it was started. Supervising it means an unexpected exit (rather than a
/// shutdown-token cancellation) is surfaced as a fatal component death.
pub fn start_checkpoint_producer(
    settings: &Settings,
    active_assets_rx: tokio::sync::watch::Receiver<Vec<String>>,
    event_tx: mpsc::Sender<pb_types::PersistedRecord>,
    shutdown: &CancellationToken,
    supervisor: &mut Supervisor,
) -> bool {
    if !settings.storage.checkpoints_enabled {
        return false;
    }
    let config = settings.checkpoint_config();
    let token = shutdown.child_token();
    supervisor.spawn("checkpoint-producer", async move {
        super::checkpoint_producer::run(config, active_assets_rx, event_tx, token).await;
    });
    true
}

/// List committed consumer position files (`consumer_*.pos`) in the WAL
/// directory. These tell the pruner how far each consumer has read so it never
/// prunes a segment a live reader still needs.
pub fn wal_consumer_position_files(wal_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(wal_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("consumer_") && name.ends_with(".pos"))
                .unwrap_or(false)
        })
        .collect()
}

/// Open the WAL writer, optionally as a hot standby that waits to take over.
///
/// Without `standby`, this is the fail-fast behavior: a second writer on the same
/// WAL directory returns the `WriterLocked` error immediately (the single-writer
/// flock). With `standby`, a `WriterLocked` is treated as "the
/// primary is alive" — the process polls the lock on `poll` and promotes itself to
/// primary writer the moment the lock is released (the primary exits/dies),
/// resuming on the shared WAL with no data loss (the takeover semantics are
/// covered by `pb_wal`'s `standby_writer_takes_over_shared_wal_after_primary_exit`
/// test). Returns `Ok(None)` if `shutdown` fires while still waiting.
///
/// NOTE: this is automatic *writer* promotion only. Redundant
/// feed connectivity with arbitration, and a measured wall-clock failover RTO,
/// require a real multi-replica deployment and are not exercised here.
pub async fn open_wal_writer_with_standby(
    config: pb_wal::WalConfig,
    standby: bool,
    poll: Duration,
    shutdown: &CancellationToken,
) -> Result<Option<pb_wal::WalWriter>> {
    loop {
        match pb_wal::WalWriter::open(config.clone()) {
            Ok(writer) => return Ok(Some(writer)),
            Err(pb_wal::WalError::WriterLocked { path }) if standby => {
                tracing::info!(
                    ?path,
                    "standby: WAL is held by the active writer; waiting to promote"
                );
                tokio::select! {
                    _ = tokio::time::sleep(poll) => {}
                    _ = shutdown.cancelled() => {
                        tracing::info!("standby: shutdown requested before promotion");
                        return Ok(None);
                    }
                }
            }
            Err(e) => return Err(anyhow::anyhow!("failed to open WAL writer: {e}")),
        }
    }
}

fn is_loopback(addr: SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Enforce the workstation trust boundary before binding externally.
///
/// Loopback remains open for local development. Any non-loopback HTTP/WS bind
/// must carry the shared API bearer token so an accidental `0.0.0.0` in a
/// container does not expose the read/query surfaces unauthenticated.
pub fn validate_api_auth_boundary(api_addr: SocketAddr, auth_token: Option<&str>) -> Result<()> {
    if auth_token.is_some() {
        return Ok(());
    }
    if !is_loopback(api_addr) {
        bail!(
            "api.auth_token is required when api.listen_addr binds a non-loopback address ({api_addr})"
        );
    }
    Ok(())
}

/// Build the Parquet-backed historical service from config.
pub async fn build_historical_service(
    settings: &Settings,
) -> Result<pb_service::HistoricalService> {
    let parquet_base_path = &settings.storage.parquet_base_path;
    tracing::info!(path = %parquet_base_path, "using Parquet historical backend");
    let (store, base_path) = build_object_store(parquet_base_path)?;
    probe_object_store_read(&store, &base_path).await?;
    let reader = pb_replay::ParquetReader::from_store(store, base_path);
    Ok(pb_service::HistoricalService::from_reader(reader))
}

pub async fn shutdown_handles(handles: Vec<JoinHandle<()>>, label: &str) {
    let timeout = Duration::from_secs(10);
    for handle in handles {
        if tokio::time::timeout(timeout, handle).await.is_err() {
            tracing::warn!("{label} did not shut down within timeout");
        }
    }
}

/// Fan out a persisted record to every sink channel. Returns `true` if all
/// sends succeeded, `false` if any channel closed (the caller should stop).
pub async fn fanout_event(
    event: pb_types::PersistedRecord,
    sink_txs: &[mpsc::Sender<pb_types::PersistedRecord>],
) -> bool {
    let mut ok = true;
    for (i, tx) in sink_txs.iter().enumerate() {
        if let Err(e) = tx.send(event.clone()).await {
            tracing::warn!(sink = i, "fan-out channel closed: {e}");
            ok = false;
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Supervisor ---

    #[tokio::test]
    async fn supervisor_reports_exited_task_name() {
        let mut sup = Supervisor::new();
        sup.spawn("short-lived", async {});
        sup.spawn("long-lived", async {
            // Stay alive long enough that the short-lived task is observed first.
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let exited = sup.next_exit().await;
        assert_eq!(exited, Some("short-lived"));
    }

    // --- Standby writer promotion ---

    fn standby_wal_config(dir: &std::path::Path) -> pb_wal::WalConfig {
        pb_wal::WalConfig {
            base_path: dir.to_path_buf(),
            segment_size: 4096,
            max_segments: 4,
            ..pb_wal::WalConfig::default()
        }
    }

    #[tokio::test]
    async fn non_standby_open_fails_fast_when_wal_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let config = standby_wal_config(dir.path());
        let _primary = pb_wal::WalWriter::open(config.clone()).unwrap();
        let shutdown = CancellationToken::new();
        // standby=false must surface the WriterLocked as an error, not wait.
        let result =
            open_wal_writer_with_standby(config, false, Duration::from_millis(10), &shutdown).await;
        assert!(
            result.is_err(),
            "non-standby open must fail fast when locked"
        );
    }

    #[tokio::test]
    async fn standby_open_waits_then_promotes_when_lock_released() {
        let dir = tempfile::tempdir().unwrap();
        let config = standby_wal_config(dir.path());
        let primary = pb_wal::WalWriter::open(config.clone()).unwrap();
        let shutdown = CancellationToken::new();

        let cfg = config.clone();
        let sd = shutdown.clone();
        let standby = tokio::spawn(async move {
            open_wal_writer_with_standby(cfg, true, Duration::from_millis(20), &sd).await
        });

        // Give the standby several poll cycles; while the primary holds the lock
        // it must not promote.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !standby.is_finished(),
            "standby must wait while primary holds the lock"
        );

        // Primary exits -> the standby promotes on its next poll.
        drop(primary);
        let promoted = tokio::time::timeout(Duration::from_secs(2), standby)
            .await
            .expect("standby should promote within the timeout")
            .expect("join ok")
            .expect("open ok");
        assert!(
            promoted.is_some(),
            "standby must acquire the WAL after the primary releases it"
        );
    }

    #[tokio::test]
    async fn standby_open_returns_none_on_shutdown_while_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let config = standby_wal_config(dir.path());
        let _primary = pb_wal::WalWriter::open(config.clone()).unwrap();
        let shutdown = CancellationToken::new();

        let cfg = config.clone();
        let sd = shutdown.clone();
        let standby = tokio::spawn(async move {
            open_wal_writer_with_standby(cfg, true, Duration::from_millis(20), &sd).await
        });

        // Cancel while it is still waiting for the (still-held) lock.
        tokio::time::sleep(Duration::from_millis(60)).await;
        shutdown.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), standby)
            .await
            .expect("should return promptly after shutdown")
            .expect("join ok")
            .expect("open ok");
        assert!(result.is_none(), "shutdown while waiting must yield None");
    }

    #[tokio::test]
    async fn supervisor_detects_panicked_task() {
        let mut sup = Supervisor::new();
        sup.spawn("panicker", async {
            panic!("boom");
        });
        let exited = sup.next_exit().await;
        assert_eq!(exited, Some("<panicked>"));
    }

    #[tokio::test]
    async fn supervisor_empty_is_empty() {
        let mut sup = Supervisor::new();
        assert!(sup.is_empty());
        sup.spawn("t", async {});
        assert!(!sup.is_empty());
        // Drain it.
        let _ = sup.next_exit().await;
        assert!(sup.is_empty());
    }

    #[tokio::test]
    async fn supervisor_join_all_completes_when_tasks_finish() {
        let mut sup = Supervisor::new();
        sup.spawn("a", async {});
        sup.spawn("b", async {});
        // Should return promptly once both tasks complete (well under the 10s cap).
        tokio::time::timeout(Duration::from_secs(5), sup.join_all("test"))
            .await
            .expect("join_all should complete promptly");
    }

    // --- build_object_store ---

    #[test]
    fn build_object_store_s3_url_does_not_create_local_dir() {
        // An s3:// path must be parsed as an S3 store, never silently turned into
        // a local directory named "s3:".
        let result = build_object_store("s3://test-bucket/orderbook");
        assert!(
            result.is_ok(),
            "s3:// url should construct an S3 object store: {result:?}"
        );
        let (_store, prefix) = result.unwrap();
        assert_eq!(prefix, "orderbook");
        assert!(
            !std::path::Path::new("s3:").exists(),
            "must not create a local directory named 's3:'"
        );
    }

    #[test]
    fn build_object_store_local_path_canonicalizes() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("nested/data");
        let (_store, prefix) =
            build_object_store(sub.to_str().unwrap()).expect("local store should build");
        // The directory is created and the prefix is an absolute canonical path.
        assert!(std::path::Path::new(&prefix).is_absolute());
        assert!(sub.exists(), "local base path should be created");
    }

    #[test]
    fn wal_consumer_position_files_lists_only_pos_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("consumer_serve-live.pos"), "0:0").unwrap();
        std::fs::write(dir.path().join("consumer_other.pos"), "1:0").unwrap();
        std::fs::write(dir.path().join("segment_00000000000000000000.wal"), b"x").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();

        let mut files = wal_consumer_position_files(dir.path());
        files.sort();
        assert_eq!(files.len(), 2, "should list only consumer_*.pos files");
        assert!(files.iter().all(|p| p
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("consumer_")));
    }

    #[test]
    fn wal_consumer_position_files_missing_dir_is_empty() {
        let files = wal_consumer_position_files(std::path::Path::new("/no/such/wal/dir"));
        assert!(files.is_empty());
    }

    #[test]
    fn validate_api_auth_boundary_requires_token_for_external_binds() {
        let api_loopback = "127.0.0.1:3000".parse::<SocketAddr>().unwrap();
        let api_external = "0.0.0.0:3000".parse::<SocketAddr>().unwrap();

        assert!(validate_api_auth_boundary(api_loopback, None).is_ok());
        assert!(validate_api_auth_boundary(api_external, None).is_err());
        assert!(validate_api_auth_boundary(api_external, Some("secret")).is_ok());
    }

    // --- fanout_event ---

    fn sample_record() -> pb_types::PersistedRecord {
        use pb_types::{
            DataSource, EventProvenance, IngestEvent, IngestEventKind, PersistedRecord,
        };
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
                ingest_ordinal: None,
            },
            expected_sequence: None,
            observed_sequence: None,
            details: None,
        })
    }

    #[tokio::test]
    async fn fanout_event_delivers_to_every_open_channel() {
        let (tx1, mut rx1) = mpsc::channel(4);
        let (tx2, mut rx2) = mpsc::channel(4);
        assert!(fanout_event(sample_record(), &[]).await);
        assert!(fanout_event(sample_record(), &[tx1, tx2]).await);
        assert!(rx1.recv().await.is_some());
        assert!(rx2.recv().await.is_some());
    }

    #[tokio::test]
    async fn fanout_event_fails_closed_when_any_channel_is_closed() {
        let (open_tx, mut open_rx) = mpsc::channel(4);
        let (closed_tx, closed_rx) = mpsc::channel(4);
        drop(closed_rx);
        assert!(!fanout_event(sample_record(), &[open_tx, closed_tx]).await);
        // The open channel still received the record.
        assert!(open_rx.recv().await.is_some());
    }
}
