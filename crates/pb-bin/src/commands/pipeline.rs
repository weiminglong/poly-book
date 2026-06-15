use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use config::Config;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub async fn start_metrics_server(settings: &Config) -> Result<()> {
    let metrics_addr: SocketAddr = settings
        .get_string("metrics.listen_addr")
        .unwrap_or_else(|_| "127.0.0.1:9090".to_string())
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

    Ok(())
}

pub struct SinkHandles {
    pub parquet_tx: Option<mpsc::Sender<pb_types::PersistedRecord>>,
    pub clickhouse_tx: Option<mpsc::Sender<pb_types::PersistedRecord>>,
}

/// Supervises long-lived background tasks. Each task is tagged with a stable
/// name; if any exits — returns, errors, or panics — before a coordinated
/// shutdown, the supervisor reports its name so the owning command can treat the
/// exit as fatal (cancel the shutdown token and return a non-zero error)
/// instead of silently continuing with a dead component or exiting 0.
///
/// This addresses the audit finding that there was no task supervision anywhere:
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
/// A path with a URL scheme (`s3://bucket/prefix`, `gs://...`, `file://...`) is
/// wired to the matching `object_store` backend; a plain path is a local
/// filesystem directory. This is the fix for the critical finding (A.1) where an
/// `s3://...` base path was silently handled by `LocalFileSystem` and written to
/// a local directory literally named `s3:` on ephemeral container storage.
///
/// For `s3://`, credentials and region come from the standard AWS provider chain
/// (env vars / ECS task role / instance profile).
pub fn build_object_store(base_path: &str) -> Result<(Arc<dyn object_store::ObjectStore>, String)> {
    if base_path.contains("://") {
        let url = url::Url::parse(base_path)
            .map_err(|e| anyhow::anyhow!("invalid storage URL {base_path}: {e}"))?;
        let (store, prefix) = object_store::parse_url(&url)
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

pub async fn start_storage_sinks(
    settings: &Config,
    enable_parquet: bool,
    enable_clickhouse: bool,
    supervisor: &mut Supervisor,
) -> Result<SinkHandles> {
    let parquet_tx = if enable_parquet {
        let configured = settings
            .get_string("storage.parquet_base_path")
            .unwrap_or_else(|_| "./data".to_string());
        let (store, base_path) = build_object_store(&configured)?;
        let flush_secs = settings
            .get_int("storage.parquet_flush_interval_secs")
            .unwrap_or(300) as u64;

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

    let clickhouse_tx = if enable_clickhouse {
        let ch_url = settings
            .get_string("storage.clickhouse_url")
            .unwrap_or_else(|_| "http://localhost:8123".to_string());
        let ch_db = settings
            .get_string("storage.clickhouse_database")
            .unwrap_or_else(|_| "poly_book".to_string());

        let (ctx, crx) = mpsc::channel::<pb_types::PersistedRecord>(10_000);
        let client = clickhouse::Client::default()
            .with_url(&ch_url)
            .with_database(&ch_db);
        let batch_size = settings
            .get_int("storage.clickhouse_batch_size")
            .unwrap_or(10_000)
            .max(0) as usize;
        let batch_interval_secs = settings
            .get_int("storage.clickhouse_batch_interval_secs")
            .unwrap_or(1)
            .max(0) as u64;
        let sink = pb_store::ClickHouseSink::new(crx, client)
            .with_batch_config(batch_size, Duration::from_secs(batch_interval_secs));
        if let Err(e) = sink.ensure_table().await {
            tracing::warn!(error = %e, "failed to ensure ClickHouse table (will retry on insert)");
        }
        supervisor.spawn("clickhouse-sink", async move {
            if let Err(e) = sink.run().await {
                tracing::error!(error = %e, "clickhouse sink failed");
            }
        });
        Some(ctx)
    } else {
        None
    };

    Ok(SinkHandles {
        parquet_tx,
        clickhouse_tx,
    })
}

pub fn checkpoint_config_from_settings(
    settings: &Config,
) -> super::checkpoint_producer::CheckpointProducerConfig {
    let checkpoint_interval_secs = settings
        .get_int("storage.checkpoint_interval_secs")
        .unwrap_or(60) as u64;
    super::checkpoint_producer::CheckpointProducerConfig {
        rest_url: settings
            .get_string("feed.rest_url")
            .unwrap_or_else(|_| "https://clob.polymarket.com".to_string()),
        interval: Duration::from_secs(checkpoint_interval_secs),
        rate_limit_pause: Duration::from_millis(100),
    }
}

/// Spawn the checkpoint producer into the supervisor if enabled. Returns `true`
/// if it was started. Supervising it means an unexpected exit (rather than a
/// shutdown-token cancellation) is surfaced as a fatal component death.
pub fn start_checkpoint_producer(
    settings: &Config,
    active_assets_rx: tokio::sync::watch::Receiver<Vec<String>>,
    event_tx: mpsc::Sender<pb_types::PersistedRecord>,
    shutdown: &CancellationToken,
    supervisor: &mut Supervisor,
) -> bool {
    let enabled = settings
        .get_bool("storage.checkpoints_enabled")
        .unwrap_or(true);
    if !enabled {
        return false;
    }
    let config = checkpoint_config_from_settings(settings);
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

pub fn wal_config_from_settings(settings: &Config) -> pb_wal::WalConfig {
    let base_path = settings
        .get_string("wal.base_path")
        .unwrap_or_else(|_| "./data/wal".to_string());
    let segment_size_mb = settings.get_int("wal.segment_size_mb").unwrap_or(64) as u64;
    let max_segments = settings.get_int("wal.max_segments").unwrap_or(16) as usize;
    let max_consumer_lag_bytes = settings
        .get_int("wal.max_consumer_lag_bytes")
        .unwrap_or(256 * 1024 * 1024) as u64;
    let position_commit_interval_ms = settings
        .get_int("wal.position_commit_interval_ms")
        .unwrap_or(1_000)
        .max(1) as u64;
    let flush_interval_ms = settings
        .get_int("wal.flush_interval_ms")
        .unwrap_or(20)
        .max(1) as u64;
    let sync_interval_ms = settings
        .get_int("wal.sync_interval_ms")
        .unwrap_or(200)
        .max(1) as u64;
    pb_wal::WalConfig {
        base_path: std::path::PathBuf::from(base_path),
        segment_size: segment_size_mb * 1024 * 1024,
        max_segments,
        max_consumer_lag_bytes,
        position_commit_interval_ms,
        flush_interval_ms,
        sync_interval_ms,
    }
}

pub fn ws_config_from_settings(settings: &Config) -> pb_feed::WsConfig {
    pb_feed::WsConfig {
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
    }
}

/// Build service backends from config.
///
/// Reads `api.historical_backend` (default: "parquet") to select the backend.
/// If "clickhouse" is selected but the connection cannot be established, falls
/// back to Parquet with a warning.
pub async fn build_services(
    settings: &Config,
) -> (
    pb_service::AnyReplayService,
    pb_service::AnyIntegrityService,
    pb_service::AnyExecutionService,
) {
    let backend = settings
        .get_string("api.historical_backend")
        .unwrap_or_else(|_| "parquet".to_string());
    let parquet_base_path = settings
        .get_string("storage.parquet_base_path")
        .unwrap_or_else(|_| "./data".to_string());

    match backend.as_str() {
        "clickhouse" => {
            let ch_url = settings
                .get_string("storage.clickhouse_url")
                .unwrap_or_else(|_| "http://localhost:8123".to_string());
            let ch_db = settings
                .get_string("storage.clickhouse_database")
                .unwrap_or_else(|_| "poly_book".to_string());

            // Probe ClickHouse connectivity before committing to it.
            let probe_client = clickhouse::Client::default()
                .with_url(&ch_url)
                .with_database(&ch_db);
            let probe = tokio::time::timeout(
                Duration::from_secs(3),
                probe_client.query("SELECT 1").fetch_one::<u8>(),
            )
            .await;

            match probe {
                Ok(Ok(_)) => {
                    tracing::info!(url = %ch_url, database = %ch_db, "using ClickHouse historical backend");
                    return (
                        pb_service::AnyReplayService::ClickHouse(
                            pb_service::ClickHouseReplayService::new(&ch_url, &ch_db),
                        ),
                        pb_service::AnyIntegrityService::ClickHouse(
                            pb_service::ClickHouseIntegrityService::new(&ch_url, &ch_db),
                        ),
                        pb_service::AnyExecutionService::ClickHouse(
                            pb_service::ClickHouseExecutionService::new(&ch_url, &ch_db),
                        ),
                    );
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        url = %ch_url,
                        "ClickHouse unavailable, falling back to Parquet"
                    );
                }
                Err(_) => {
                    tracing::warn!(
                        url = %ch_url,
                        "ClickHouse probe timed out, falling back to Parquet"
                    );
                }
            }
        }
        other if other != "parquet" => {
            tracing::warn!(
                backend = %other,
                "unknown historical_backend, falling back to parquet"
            );
        }
        _ => {}
    }

    tracing::info!(path = %parquet_base_path, "using Parquet historical backend");
    (
        pb_service::AnyReplayService::Parquet(pb_service::ParquetReplayService::new(
            &parquet_base_path,
        )),
        pb_service::AnyIntegrityService::Parquet(pb_service::ParquetIntegrityService::new(
            &parquet_base_path,
        )),
        pb_service::AnyExecutionService::Parquet(pb_service::ParquetExecutionService::new(
            &parquet_base_path,
        )),
    )
}

/// Build the query service from config.
///
/// Returns `None` if `api.query_workbench_enabled` is not set to `true`.
pub async fn build_query_service(settings: &Config) -> Option<pb_service::AnyQueryService> {
    let enabled = settings
        .get_bool("api.query_workbench_enabled")
        .unwrap_or(false);
    if !enabled {
        tracing::info!("query workbench disabled");
        return None;
    }

    let backend = settings
        .get_string("api.historical_backend")
        .unwrap_or_else(|_| "parquet".to_string());

    match backend.as_str() {
        "clickhouse" => {
            let ch_url = settings
                .get_string("storage.clickhouse_url")
                .unwrap_or_else(|_| "http://localhost:8123".to_string());
            let ch_db = settings
                .get_string("storage.clickhouse_database")
                .unwrap_or_else(|_| "poly_book".to_string());
            tracing::info!(url = %ch_url, database = %ch_db, "query workbench enabled (ClickHouse)");
            Some(pb_service::AnyQueryService::ClickHouse(
                pb_service::ClickHouseQueryService::new(&ch_url, &ch_db),
            ))
        }
        _ => {
            tracing::warn!(
                "query workbench requires clickhouse backend, currently using {backend}"
            );
            None
        }
    }
}

/// Read query guard settings from config.
pub fn query_config_from_settings(settings: &Config) -> (usize, u64) {
    let max_rows = settings.get_int("api.query_max_rows").unwrap_or(10_000) as usize;
    let timeout_secs = settings.get_int("api.query_timeout_secs").unwrap_or(30) as u64;
    (max_rows, timeout_secs)
}

pub fn grpc_config_from_settings(settings: &Config) -> (bool, SocketAddr) {
    let enabled = settings.get_bool("grpc.enabled").unwrap_or(false);
    let addr: SocketAddr = settings
        .get_string("grpc.listen_addr")
        .unwrap_or_else(|_| "127.0.0.1:50051".to_string())
        .parse()
        .unwrap_or_else(|_| "127.0.0.1:50051".parse().unwrap());
    (enabled, addr)
}

pub async fn shutdown_handles(handles: Vec<JoinHandle<()>>, label: &str) {
    let timeout = Duration::from_secs(10);
    for handle in handles {
        if tokio::time::timeout(timeout, handle).await.is_err() {
            tracing::warn!("{label} did not shut down within timeout");
        }
    }
}

/// Fan out a persisted record to sink channels. Returns `true` if all sends
/// succeeded, `false` if any channel closed (caller should stop).
pub async fn fanout_event(
    event: pb_types::PersistedRecord,
    fanout_txs: &[mpsc::Sender<pb_types::PersistedRecord>],
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

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_config() -> Config {
        Config::builder().build().unwrap()
    }

    fn config_with(key: &str, value: &str) -> Config {
        Config::builder()
            .set_override(key, value)
            .unwrap()
            .build()
            .unwrap()
    }

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
        // a local directory named "s3:" (critical finding A.1).
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

    // --- wal_config_from_settings ---

    #[test]
    fn wal_config_defaults() {
        let cfg = wal_config_from_settings(&empty_config());
        assert_eq!(cfg.base_path, std::path::PathBuf::from("./data/wal"));
        assert_eq!(cfg.segment_size, 64 * 1024 * 1024);
        assert_eq!(cfg.max_segments, 16);
        assert_eq!(cfg.max_consumer_lag_bytes, 256 * 1024 * 1024);
        assert_eq!(cfg.position_commit_interval_ms, 1_000);
    }

    #[test]
    fn wal_config_overrides() {
        let settings = Config::builder()
            .set_override("wal.base_path", "/tmp/wal")
            .unwrap()
            .set_override("wal.segment_size_mb", 32)
            .unwrap()
            .set_override("wal.max_segments", 8)
            .unwrap()
            .set_override("wal.position_commit_interval_ms", 250)
            .unwrap()
            .build()
            .unwrap();
        let cfg = wal_config_from_settings(&settings);
        assert_eq!(cfg.base_path, std::path::PathBuf::from("/tmp/wal"));
        assert_eq!(cfg.segment_size, 32 * 1024 * 1024);
        assert_eq!(cfg.max_segments, 8);
        assert_eq!(cfg.position_commit_interval_ms, 250);
    }

    // --- ws_config_from_settings ---

    #[test]
    fn ws_config_defaults() {
        let cfg = ws_config_from_settings(&empty_config());
        let default = pb_feed::WsConfig::default();
        assert_eq!(cfg.ws_url, default.ws_url);
        assert_eq!(cfg.ping_interval_secs, 10);
        assert_eq!(cfg.reconnect_base_delay_ms, 100);
        assert_eq!(cfg.reconnect_max_delay_ms, 30000);
    }

    #[test]
    fn ws_config_overrides() {
        let settings = Config::builder()
            .set_override("feed.ping_interval_secs", 20)
            .unwrap()
            .set_override("feed.reconnect_base_delay_ms", 500)
            .unwrap()
            .set_override("feed.reconnect_max_delay_ms", 60000)
            .unwrap()
            .build()
            .unwrap();
        let cfg = ws_config_from_settings(&settings);
        assert_eq!(cfg.ping_interval_secs, 20);
        assert_eq!(cfg.reconnect_base_delay_ms, 500);
        assert_eq!(cfg.reconnect_max_delay_ms, 60000);
    }

    // --- query_config_from_settings ---

    #[test]
    fn query_config_defaults() {
        let (max_rows, timeout) = query_config_from_settings(&empty_config());
        assert_eq!(max_rows, 10_000);
        assert_eq!(timeout, 30);
    }

    #[test]
    fn query_config_overrides() {
        let settings = Config::builder()
            .set_override("api.query_max_rows", 5000)
            .unwrap()
            .set_override("api.query_timeout_secs", 60)
            .unwrap()
            .build()
            .unwrap();
        let (max_rows, timeout) = query_config_from_settings(&settings);
        assert_eq!(max_rows, 5000);
        assert_eq!(timeout, 60);
    }

    // --- grpc_config_from_settings ---

    #[test]
    fn grpc_config_defaults() {
        let (enabled, addr) = grpc_config_from_settings(&empty_config());
        assert!(!enabled);
        assert_eq!(addr, "127.0.0.1:50051".parse::<SocketAddr>().unwrap());
    }

    #[test]
    fn grpc_config_enabled_with_custom_addr() {
        let settings = Config::builder()
            .set_override("grpc.enabled", true)
            .unwrap()
            .set_override("grpc.listen_addr", "127.0.0.1:9999")
            .unwrap()
            .build()
            .unwrap();
        let (enabled, addr) = grpc_config_from_settings(&settings);
        assert!(enabled);
        assert_eq!(addr, "127.0.0.1:9999".parse::<SocketAddr>().unwrap());
    }

    // --- checkpoint_config_from_settings ---

    #[test]
    fn checkpoint_config_defaults() {
        let cfg = checkpoint_config_from_settings(&empty_config());
        assert_eq!(cfg.rest_url, "https://clob.polymarket.com");
        assert_eq!(cfg.interval, Duration::from_secs(60));
        assert_eq!(cfg.rate_limit_pause, Duration::from_millis(100));
    }

    #[test]
    fn checkpoint_config_overrides() {
        let settings = config_with("storage.checkpoint_interval_secs", "120");
        let cfg = checkpoint_config_from_settings(&settings);
        assert_eq!(cfg.interval, Duration::from_secs(120));
    }
}
