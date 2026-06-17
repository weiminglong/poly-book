use anyhow::Result;
use config::Config;
use pb_types::SlugRegistry;
use tokio_util::sync::CancellationToken;

pub async fn run(
    settings: Config,
    tokens: String,
    interval_secs: u64,
    duration_mins: u64,
    shutdown: CancellationToken,
    slug_registry: SlugRegistry,
) -> Result<()> {
    let token_ids: Vec<String> = tokens
        .split(',')
        .map(|s| s.trim())
        .map(|input| {
            slug_registry
                .resolve(input)
                .map(|id| id.to_string())
                .unwrap_or_else(|| input.to_string())
        })
        .collect();

    tracing::info!(
        tokens = ?token_ids,
        interval_secs,
        "starting backfill"
    );

    let rest_url = settings
        .get_string("feed.rest_url")
        .unwrap_or_else(|_| "https://clob.polymarket.com".to_string());

    let (event_tx, event_rx) = tokio::sync::mpsc::channel::<pb_types::PersistedRecord>(10_000);

    // Start Parquet sink for backfilled data. build_object_store canonicalizes a
    // local base path (fixing the relative-path bug A.28 where the raw "./data"
    // was percent-encoded by object_store and every flush failed) and wires an
    // s3:// path to a real S3 store (A.1).
    let configured = settings
        .get_string("storage.parquet_base_path")
        .unwrap_or_else(|_| "./data".to_string());
    let (store, base_path) = super::pipeline::build_object_store(&configured)?;
    let flush_secs = settings
        .get_int("storage.parquet_flush_interval_secs")
        .unwrap_or(300) as u64;

    let sink = pb_store::ParquetSink::new(event_rx, store, base_path)
        .with_flush_interval(std::time::Duration::from_secs(flush_secs));
    let sink_token = shutdown.child_token();
    let sink_handle = tokio::spawn(async move {
        if let Err(e) = sink.run_with_token(sink_token).await {
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

    let backfill_token = shutdown.child_token();
    if duration_mins > 0 {
        let dur = std::time::Duration::from_secs(duration_mins * 60);
        match tokio::time::timeout(
            dur,
            pb_replay::backfill::run_backfill_with_token(config, event_tx, backfill_token),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => tracing::info!(duration_mins, "backfill duration reached, stopping"),
        }
    } else {
        pb_replay::backfill::run_backfill_with_token(config, event_tx, backfill_token).await?;
    }

    // event_tx is consumed by run_backfill_with_token, sink will see channel close
    let timeout = std::time::Duration::from_secs(10);
    if tokio::time::timeout(timeout, sink_handle).await.is_err() {
        tracing::warn!("sink did not shut down within timeout");
    }

    tracing::info!("backfill complete");
    Ok(())
}
