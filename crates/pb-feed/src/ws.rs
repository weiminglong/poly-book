use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use crate::error::FeedError;

const WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
const PING_INTERVAL_SECS: u64 = 10;
const BASE_BACKOFF_MS: u64 = 100;
const MAX_BACKOFF_MS: u64 = 30_000;

#[derive(Debug, Clone)]
pub struct WsRawMessage {
    pub text: String,
    pub recv_timestamp_us: u64,
}

pub struct WsClient {
    asset_ids: Vec<String>,
    tx: mpsc::Sender<WsRawMessage>,
}

impl WsClient {
    pub fn new(asset_ids: Vec<String>, tx: mpsc::Sender<WsRawMessage>) -> Self {
        Self { asset_ids, tx }
    }

    pub async fn run(&self) -> Result<(), FeedError> {
        let mut attempt: u32 = 0;
        loop {
            match self.connect_and_listen().await {
                Ok(()) => {
                    info!("ws connection closed gracefully");
                    attempt = 0;
                }
                Err(e) => {
                    warn!("ws connection error: {e}");
                }
            }

            let backoff = Self::backoff_ms(attempt);
            info!(backoff_ms = backoff, attempt, "reconnecting");
            tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
            attempt = attempt.saturating_add(1);
        }
    }

    async fn connect_and_listen(&self) -> Result<(), FeedError> {
        let (ws_stream, _) = connect_async(WS_URL).await?;
        let (mut sink, mut stream) = ws_stream.split();
        info!("ws connected to {WS_URL}");

        for asset_id in &self.asset_ids {
            let sub = serde_json::json!({
                "type": "subscribe",
                "channel": "market",
                "assets_id": asset_id,
            });
            sink.send(Message::Text(sub.to_string())).await?;
            debug!(asset_id, "subscribed");
        }

        let mut ping_interval =
            tokio::time::interval(std::time::Duration::from_secs(PING_INTERVAL_SECS));

        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    sink.send(Message::Ping(vec![])).await?;
                    debug!("sent ping");
                }
                msg = stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_micros() as u64;
                            let raw = WsRawMessage {
                                text: text.to_string(),
                                recv_timestamp_us: now,
                            };
                            if self.tx.send(raw).await.is_err() {
                                error!("receiver dropped, stopping ws client");
                                return Ok(());
                            }
                        }
                        Some(Ok(Message::Pong(_))) => {
                            debug!("received pong");
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("received close frame");
                            return Ok(());
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            return Err(e.into());
                        }
                        None => {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    fn backoff_ms(attempt: u32) -> u64 {
        let exp = BASE_BACKOFF_MS.saturating_mul(1u64 << attempt.min(15));
        let jitter = fastrand_jitter(exp / 4);
        exp.saturating_add(jitter).min(MAX_BACKOFF_MS)
    }
}

fn fastrand_jitter(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let raw = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    raw % max
}
