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
    pub task_handles: Vec<JoinHandle<()>>,
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
) -> Result<SinkHandles> {
    let mut task_handles = Vec::new();

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
        task_handles.push(tokio::spawn(async move {
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

        let (ctx, crx) = mpsc::channel::<pb_types::PersistedRecord>(10_000);
        let client = clickhouse::Client::default()
            .with_url(&ch_url)
            .with_database(&ch_db);
        let sink = pb_store::ClickHouseSink::new(crx, client);
        if let Err(e) = sink.ensure_table().await {
            tracing::warn!(error = %e, "failed to ensure ClickHouse table (will retry on insert)");
        }
        task_handles.push(tokio::spawn(async move {
            if let Err(e) = sink.run().await {
                tracing::error!(error = %e, "clickhouse sink failed");
            }
        }));
        Some(ctx)
    } else {
        None
    };

    Ok(SinkHandles {
        parquet_tx,
        clickhouse_tx,
        task_handles,
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

pub fn start_checkpoint_producer(
    settings: &Config,
    active_assets_rx: tokio::sync::watch::Receiver<Vec<String>>,
    event_tx: mpsc::Sender<pb_types::PersistedRecord>,
    shutdown: &CancellationToken,
) -> Option<JoinHandle<()>> {
    let enabled = settings
        .get_bool("storage.checkpoints_enabled")
        .unwrap_or(true);
    if !enabled {
        return None;
    }
    let config = checkpoint_config_from_settings(settings);
    Some(super::checkpoint_producer::spawn(
        config,
        active_assets_rx,
        event_tx,
        shutdown.child_token(),
    ))
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
