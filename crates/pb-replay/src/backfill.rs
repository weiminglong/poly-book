use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use pb_types::wire::RestBookResponse;
use pb_types::{
    AssetId, BookCheckpoint, DataSource, FixedPrice, FixedSize, PersistedRecord, PriceLevel,
};

use crate::error::ReplayError;

/// Configuration for the REST-based checkpoint backfill process.
#[derive(Debug, Clone)]
pub struct BackfillConfig {
    pub rest_url: String,
    pub token_ids: Vec<String>,
    pub interval: Duration,
    pub rate_limit_pause: Duration,
}

pub async fn run_backfill(
    config: BackfillConfig,
    tx: mpsc::Sender<PersistedRecord>,
) -> Result<(), ReplayError> {
    run_backfill_with_token(config, tx, CancellationToken::new()).await
}

pub async fn run_backfill_with_token(
    config: BackfillConfig,
    tx: mpsc::Sender<PersistedRecord>,
    token: CancellationToken,
) -> Result<(), ReplayError> {
    let client = reqwest::Client::new();

    info!(
        url = %config.rest_url,
        num_tokens = config.token_ids.len(),
        interval_secs = config.interval.as_secs(),
        "starting checkpoint backfill loop"
    );

    loop {
        for token_id in &config.token_ids {
            match fetch_checkpoint_with_client(&client, &config.rest_url, token_id).await {
                Ok(checkpoint) => {
                    if tx
                        .send(PersistedRecord::Checkpoint(checkpoint))
                        .await
                        .is_err()
                    {
                        info!("backfill channel closed, stopping");
                        return Ok(());
                    }
                    debug!(token_id, "checkpoint snapshot sent");
                }
                Err(e) => {
                    error!(token_id, error = %e, "backfill fetch failed");
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(config.rate_limit_pause) => {}
                _ = token.cancelled() => {
                    info!("backfill shutdown requested");
                    return Ok(());
                }
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(config.interval) => {}
            _ = token.cancelled() => {
                info!("backfill shutdown requested");
                return Ok(());
            }
        }
    }
}

pub async fn fetch_checkpoint(
    base_url: &str,
    token_id: &str,
) -> Result<BookCheckpoint, ReplayError> {
    let client = reqwest::Client::new();
    fetch_checkpoint_with_client(&client, base_url, token_id).await
}

pub async fn fetch_checkpoint_with_client(
    client: &reqwest::Client,
    base_url: &str,
    token_id: &str,
) -> Result<BookCheckpoint, ReplayError> {
    let book = fetch_snapshot(client, base_url, token_id).await?;
    checkpoint_from_rest(&book)
}

pub fn checkpoint_from_rest(book: &RestBookResponse) -> Result<BookCheckpoint, ReplayError> {
    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let exchange_ts = parse_timestamp_us(book.timestamp.as_deref()).unwrap_or(now_us);

    let bids = book
        .bids
        .iter()
        .map(|entry| {
            Ok(PriceLevel {
                price: parse_price(&entry.price)?,
                size: parse_size(&entry.size)?,
            })
        })
        .collect::<Result<Vec<_>, pb_types::TypesError>>()?;
    let asks = book
        .asks
        .iter()
        .map(|entry| {
            Ok(PriceLevel {
                price: parse_price(&entry.price)?,
                size: parse_size(&entry.size)?,
            })
        })
        .collect::<Result<Vec<_>, pb_types::TypesError>>()?;

    Ok(BookCheckpoint {
        asset_id: AssetId::new(book.asset_id.clone()),
        checkpoint_timestamp_us: exchange_ts,
        recv_timestamp_us: now_us,
        exchange_timestamp_us: exchange_ts,
        source: DataSource::RestSnapshot,
        source_event_id: book.hash.clone(),
        source_session_id: None,
        bids,
        asks,
    })
}

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    base_url: &str,
    token_id: &str,
) -> Result<RestBookResponse, ReplayError> {
    let url = format!("{}/book?token_id={}", base_url, token_id);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| ReplayError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(ReplayError::Http(format!(
            "HTTP {} for token_id={}",
            resp.status(),
            token_id
        )));
    }

    let book: RestBookResponse = resp
        .json()
        .await
        .map_err(|e| ReplayError::Http(e.to_string()))?;

    Ok(book)
}

fn parse_price(s: &str) -> Result<FixedPrice, pb_types::TypesError> {
    FixedPrice::try_from(s)
}

fn parse_size(s: &str) -> Result<FixedSize, pb_types::TypesError> {
    FixedSize::try_from(s)
}

fn parse_timestamp_us(ts: Option<&str>) -> Option<u64> {
    let raw = ts?.parse::<u64>().ok()?;
    if raw < 10_000_000_000_000 {
        Some(raw.saturating_mul(1000))
    } else {
        Some(raw)
    }
}
