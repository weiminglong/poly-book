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
        .unwrap_or_else(|_| "0.0.0.0:9090".to_string())
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

pub async fn start_storage_sinks(
    settings: &Config,
    enable_parquet: bool,
    enable_clickhouse: bool,
) -> Result<SinkHandles> {
    let mut task_handles = Vec::new();

    let parquet_tx = if enable_parquet {
        let base_path = settings
            .get_string("storage.parquet_base_path")
            .unwrap_or_else(|_| "./data".to_string());
        let base_path = std::path::Path::new(&base_path)
            .canonicalize()
            .or_else(|_| {
                std::fs::create_dir_all(&base_path)?;
                std::path::Path::new(&base_path).canonicalize()
            })?
            .to_string_lossy()
            .to_string();
        let flush_secs = settings
            .get_int("storage.parquet_flush_interval_secs")
            .unwrap_or(300) as u64;

        let (ptx, prx) = mpsc::channel::<pb_types::PersistedRecord>(10_000);
        let store: Arc<dyn object_store::ObjectStore> =
            Arc::new(object_store::local::LocalFileSystem::new());
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
    pb_wal::WalConfig {
        base_path: std::path::PathBuf::from(base_path),
        segment_size: segment_size_mb * 1024 * 1024,
        max_segments,
        max_consumer_lag_bytes,
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
        pb_service::AnyReplayService::Parquet(
            pb_service::ParquetReplayService::new(&parquet_base_path),
        ),
        pb_service::AnyIntegrityService::Parquet(
            pb_service::ParquetIntegrityService::new(&parquet_base_path),
        ),
        pb_service::AnyExecutionService::Parquet(
            pb_service::ParquetExecutionService::new(&parquet_base_path),
        ),
    )
}

pub fn grpc_config_from_settings(settings: &Config) -> (bool, SocketAddr) {
    let enabled = settings.get_bool("grpc.enabled").unwrap_or(false);
    let addr: SocketAddr = settings
        .get_string("grpc.listen_addr")
        .unwrap_or_else(|_| "0.0.0.0:50051".to_string())
        .parse()
        .unwrap_or_else(|_| "0.0.0.0:50051".parse().unwrap());
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
