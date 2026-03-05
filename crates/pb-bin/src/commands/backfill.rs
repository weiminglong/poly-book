use anyhow::Result;
use config::Config;
use std::sync::Arc;

pub async fn run(
    settings: Config,
    tokens: String,
    interval_secs: u64,
    _duration_mins: u64,
) -> Result<()> {
    let token_ids: Vec<String> = tokens.split(',').map(|s| s.trim().to_string()).collect();

    tracing::info!(
        tokens = ?token_ids,
        interval_secs,
        "starting backfill"
    );

    let rest_url = settings
        .get_string("feed.rest_url")
        .unwrap_or_else(|_| "https://clob.polymarket.com".to_string());

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<pb_types::OrderbookEvent>(10_000);

    // Start Parquet sink for backfilled data
    let base_path = settings
        .get_string("storage.parquet_base_path")
        .unwrap_or_else(|_| "./data".to_string());
    let flush_secs = settings
        .get_int("storage.parquet_flush_interval_secs")
        .unwrap_or(300) as u64;

    let store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::local::LocalFileSystem::new());
    let sink = pb_store::ParquetSink::new(event_rx, store, base_path)
        .with_flush_interval(std::time::Duration::from_secs(flush_secs));
    tokio::spawn(async move {
        if let Err(e) = sink.run().await {
            tracing::error!(error = %e, "parquet sink failed during backfill");
        }
    });

    // Start backfill
    let config = pb_replay::BackfillConfig {
        rest_url,
        token_ids,
        interval: std::time::Duration::from_secs(interval_secs),
        rate_limit_pause: std::time::Duration::from_millis(100),
    };

    pb_replay::backfill::run_backfill(config, event_tx).await?;

    tracing::info!("backfill complete");
    Ok(())
}
