use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use pb_types::event::{EventType, OrderbookEvent};
use pb_types::wire::RestBookResponse;
use pb_types::{AssetId, FixedPrice, FixedSize, Sequence};

use crate::error::ReplayError;

/// Configuration for the REST-based backfill process.
#[derive(Debug, Clone)]
pub struct BackfillConfig {
    /// Base URL for the REST API (e.g., "https://clob.polymarket.com").
    pub rest_url: String,
    /// Token IDs to periodically snapshot.
    pub token_ids: Vec<String>,
    /// Interval between snapshot fetches.
    pub interval: Duration,
    /// Rate limit: minimum pause between consecutive HTTP requests.
    pub rate_limit_pause: Duration,
}

/// Run the backfill loop: periodically fetch REST snapshots and send them as events.
///
/// Runs indefinitely until the receiver is dropped or the task is cancelled.
pub async fn run_backfill(
    config: BackfillConfig,
    tx: mpsc::Sender<OrderbookEvent>,
) -> Result<(), ReplayError> {
    let client = reqwest::Client::new();
    let mut sequence = 0u64;

    info!(
        url = %config.rest_url,
        num_tokens = config.token_ids.len(),
        interval_secs = config.interval.as_secs(),
        "starting backfill loop"
    );

    loop {
        for token_id in &config.token_ids {
            match fetch_snapshot(&client, &config.rest_url, token_id).await {
                Ok(book) => {
                    let now_us = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as u64;

                    let asset_id = AssetId::new(token_id.clone());

                    // Convert bids to events
                    for entry in &book.bids {
                        let price = match parse_price(&entry.price) {
                            Ok(p) => p,
                            Err(e) => {
                                warn!(token_id, price = %entry.price, "skipping invalid bid price: {e}");
                                continue;
                            }
                        };
                        let size = parse_size(&entry.size);
                        sequence += 1;

                        let event = OrderbookEvent {
                            recv_timestamp_us: now_us,
                            exchange_timestamp_us: now_us,
                            asset_id: asset_id.clone(),
                            event_type: EventType::Snapshot,
                            side: Some(pb_types::Side::Bid),
                            price,
                            size,
                            sequence: Sequence::new(sequence),
                        };

                        if tx.send(event).await.is_err() {
                            info!("backfill channel closed, stopping");
                            return Ok(());
                        }
                    }

                    // Convert asks to events
                    for entry in &book.asks {
                        let price = match parse_price(&entry.price) {
                            Ok(p) => p,
                            Err(e) => {
                                warn!(token_id, price = %entry.price, "skipping invalid ask price: {e}");
                                continue;
                            }
                        };
                        let size = parse_size(&entry.size);
                        sequence += 1;

                        let event = OrderbookEvent {
                            recv_timestamp_us: now_us,
                            exchange_timestamp_us: now_us,
                            asset_id: asset_id.clone(),
                            event_type: EventType::Snapshot,
                            side: Some(pb_types::Side::Ask),
                            price,
                            size,
                            sequence: Sequence::new(sequence),
                        };

                        if tx.send(event).await.is_err() {
                            info!("backfill channel closed, stopping");
                            return Ok(());
                        }
                    }

                    debug!(
                        token_id,
                        bids = book.bids.len(),
                        asks = book.asks.len(),
                        "backfill snapshot sent"
                    );
                }
                Err(e) => {
                    error!(token_id, error = %e, "backfill fetch failed");
                }
            }

            tokio::time::sleep(config.rate_limit_pause).await;
        }

        tokio::time::sleep(config.interval).await;
    }
}

/// Fetch a book snapshot from the REST API.
async fn fetch_snapshot(
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

fn parse_size(s: &str) -> FixedSize {
    FixedSize::try_from(s).unwrap_or(FixedSize::ZERO)
}
