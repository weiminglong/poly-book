use anyhow::{bail, Result};
use config::Config;
use std::net::SocketAddr;
use std::sync::Arc;

pub async fn run(
    settings: Config,
    tokens: Option<String>,
    enable_parquet: bool,
    enable_clickhouse: bool,
    enable_metrics: bool,
) -> Result<()> {
    let token_ids: Vec<String> = match tokens {
        Some(t) => t.split(',').map(|s| s.trim().to_string()).collect(),
        None => bail!("--tokens is required. Use 'discover' command to find token IDs."),
    };

    if token_ids.is_empty() {
        bail!("No token IDs provided");
    }

    tracing::info!(tokens = ?token_ids, "starting ingestion pipeline");

    // Start metrics server if enabled
    if enable_metrics {
        let metrics_addr: SocketAddr = settings
            .get_string("metrics.listen_addr")
            .unwrap_or_else(|_| "0.0.0.0:9090".to_string())
            .parse()?;
        let metrics_endpoint = settings
            .get_string("metrics.endpoint")
            .unwrap_or_else(|_| "/metrics".to_string());

        // Install recorder BEFORE registering metrics
        let handle = pb_metrics::install_recorder()
            .map_err(|e| anyhow::anyhow!("failed to install metrics recorder: {e}"))?;
        pb_metrics::register_metrics();

        // Bind listener synchronously so bind failures propagate to caller
        let listener = tokio::net::TcpListener::bind(metrics_addr).await?;
        tracing::info!(%metrics_addr, endpoint = metrics_endpoint.as_str(), "metrics server bound");

        tokio::spawn(async move {
            if let Err(e) =
                pb_metrics::serve_metrics_on_listener(handle, listener, &metrics_endpoint).await
            {
                tracing::error!(error = %e, "metrics server failed");
            }
        });
    }

    // Build WS config from settings
    let ws_config = pb_feed::WsConfig {
        ws_url: settings
            .get_string("feed.ws_url")
            .unwrap_or_else(|_| pb_feed::WsConfig::default().ws_url),
        ping_interval_secs: settings
            .get_int("feed.ping_interval_secs")
            .unwrap_or(10) as u64,
        reconnect_base_delay_ms: settings
            .get_int("feed.reconnect_base_delay_ms")
            .unwrap_or(100) as u64,
        reconnect_max_delay_ms: settings
            .get_int("feed.reconnect_max_delay_ms")
            .unwrap_or(30000) as u64,
    };

    // Channels: WS raw -> dispatcher -> events
    let (raw_tx, raw_rx) = tokio::sync::mpsc::channel(10_000);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel::<pb_types::OrderbookEvent>(10_000);

    // Spawn WebSocket client
    let ws_client = pb_feed::WsClient::new(token_ids.clone(), raw_tx).with_config(ws_config);
    tokio::spawn(async move {
        if let Err(e) = ws_client.run().await {
            tracing::error!(error = %e, "websocket client failed");
        }
    });

    // Spawn dispatcher
    let mut dispatcher = pb_feed::Dispatcher::new(raw_rx, event_tx);
    tokio::spawn(async move {
        if let Err(e) = dispatcher.run().await {
            tracing::error!(error = %e, "dispatcher failed");
        }
    });

    // Start storage sinks
    let parquet_tx = if enable_parquet {
        let base_path = settings
            .get_string("storage.parquet_base_path")
            .unwrap_or_else(|_| "./data".to_string());
        let flush_secs = settings
            .get_int("storage.parquet_flush_interval_secs")
            .unwrap_or(300) as u64;

        let (ptx, prx) = tokio::sync::mpsc::channel::<pb_types::OrderbookEvent>(10_000);
        let store: Arc<dyn object_store::ObjectStore> =
            Arc::new(object_store::local::LocalFileSystem::new());
        let sink = pb_store::ParquetSink::new(prx, store, base_path)
            .with_flush_interval(std::time::Duration::from_secs(flush_secs));
        tokio::spawn(async move {
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

        let (ctx, crx) = tokio::sync::mpsc::channel::<pb_types::OrderbookEvent>(10_000);
        let client = clickhouse::Client::default()
            .with_url(&ch_url)
            .with_database(&ch_db);
        let sink = pb_store::ClickHouseSink::new(crx, client);
        if let Err(e) = sink.ensure_table().await {
            tracing::warn!(error = %e, "failed to ensure ClickHouse table (will retry on insert)");
        }
        tokio::spawn(async move {
            if let Err(e) = sink.run().await {
                tracing::error!(error = %e, "clickhouse sink failed");
            }
        });
        Some(ctx)
    } else {
        None
    };

    // Fan-out events to storage sinks
    tracing::info!("ingestion pipeline running, press Ctrl+C to stop");
    loop {
        match event_rx.recv().await {
            Some(event) => {
                if let Some(ref ptx) = parquet_tx {
                    if let Err(e) = ptx.send(event.clone()).await {
                        tracing::warn!("parquet sink send failed: {e}");
                    }
                }
                if let Some(ref ctx) = clickhouse_tx {
                    if let Err(e) = ctx.send(event).await {
                        tracing::warn!("clickhouse sink send failed: {e}");
                    }
                }
            }
            None => {
                tracing::info!("event channel closed, shutting down");
                break;
            }
        }
    }

    Ok(())
}
