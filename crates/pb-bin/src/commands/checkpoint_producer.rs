use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use pb_types::PersistedRecord;

#[derive(Debug, Clone)]
pub struct CheckpointProducerConfig {
    pub rest_url: String,
    pub interval: Duration,
    pub rate_limit_pause: Duration,
}

pub fn spawn(
    config: CheckpointProducerConfig,
    active_assets: watch::Receiver<Vec<String>>,
    tx: mpsc::Sender<PersistedRecord>,
    token: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run(config, active_assets, tx, token).await;
    })
}

async fn run(
    config: CheckpointProducerConfig,
    active_assets: watch::Receiver<Vec<String>>,
    tx: mpsc::Sender<PersistedRecord>,
    token: CancellationToken,
) {
    let client = reqwest::Client::new();
    let mut ticker = tokio::time::interval(config.interval);
    ticker.tick().await;

    tracing::info!(
        interval_secs = config.interval.as_secs(),
        "starting live checkpoint producer"
    );

    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                tracing::info!("checkpoint producer shutdown requested");
                return;
            }
            _ = ticker.tick() => {}
        }

        let token_ids = active_assets.borrow().clone();
        if token_ids.is_empty() {
            tracing::debug!("checkpoint producer found no active assets");
            continue;
        }

        for token_id in token_ids {
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    tracing::info!("checkpoint producer shutdown requested");
                    return;
                }
                result = pb_replay::backfill::fetch_snapshot(&client, &config.rest_url, &token_id) => {
                    match result {
                        Ok(book) => match pb_replay::backfill::checkpoint_from_rest(&book) {
                            Ok(checkpoint) => {
                                if tx.send(PersistedRecord::Checkpoint(checkpoint)).await.is_err() {
                                    tracing::info!("checkpoint channel closed, stopping");
                                    return;
                                }
                            }
                            Err(error) => {
                                tracing::warn!(asset_id = %token_id, error = %error, "checkpoint conversion failed");
                            }
                        },
                        Err(error) => {
                            tracing::warn!(asset_id = %token_id, error = %error, "checkpoint snapshot fetch failed");
                        }
                    }
                }
            }

            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    tracing::info!("checkpoint producer shutdown requested");
                    return;
                }
                _ = tokio::time::sleep(config.rate_limit_pause) => {}
            }
        }
    }
}
