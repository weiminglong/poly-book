use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async_tls_with_config, tungstenite::Message, Connector};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::error::FeedError;

const DEFAULT_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
const DEFAULT_PING_INTERVAL_SECS: u64 = 10;
const DEFAULT_BASE_BACKOFF_MS: u64 = 100;
const DEFAULT_MAX_BACKOFF_MS: u64 = 30_000;

#[derive(Debug, Clone)]
pub struct WsConfig {
    pub ws_url: String,
    pub ping_interval_secs: u64,
    pub reconnect_base_delay_ms: u64,
    pub reconnect_max_delay_ms: u64,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            ws_url: DEFAULT_WS_URL.to_string(),
            ping_interval_secs: DEFAULT_PING_INTERVAL_SECS,
            reconnect_base_delay_ms: DEFAULT_BASE_BACKOFF_MS,
            reconnect_max_delay_ms: DEFAULT_MAX_BACKOFF_MS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WsRawMessage {
    pub text: String,
    pub recv_timestamp_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsLifecycleKind {
    ReconnectStart,
    ReconnectSuccess,
}

#[derive(Debug, Clone)]
pub struct WsLifecycleEvent {
    pub kind: WsLifecycleKind,
    pub recv_timestamp_us: u64,
    pub session_id: String,
    pub details: Option<String>,
}

#[derive(Debug, Clone)]
pub enum FeedMessage {
    Raw(WsRawMessage),
    Lifecycle(WsLifecycleEvent),
}

pub struct WsClient {
    asset_ids: Vec<String>,
    tx: mpsc::Sender<FeedMessage>,
    config: WsConfig,
    /// Reused across reconnections for TLS session caching.
    tls_connector: native_tls::TlsConnector,
}

impl WsClient {
    pub fn new(asset_ids: Vec<String>, tx: mpsc::Sender<FeedMessage>) -> Result<Self, FeedError> {
        let tls_connector = native_tls::TlsConnector::new()?;
        Ok(Self {
            asset_ids,
            tx,
            config: WsConfig::default(),
            tls_connector,
        })
    }

    pub fn with_config(mut self, config: WsConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn run(&self) -> Result<(), FeedError> {
        self.run_with_token(CancellationToken::new()).await
    }

    pub async fn run_with_token(&self, token: CancellationToken) -> Result<(), FeedError> {
        let mut attempt: u32 = 0;
        loop {
            if token.is_cancelled() {
                info!("ws client shutdown requested");
                return Ok(());
            }

            let session_id = format!("ws-session-{}", attempt + 1);
            match self
                .connect_and_listen_with_token(&token, &session_id)
                .await
            {
                Ok(()) => {
                    info!("ws connection closed gracefully");
                }
                Err(FeedError::ChannelSend) => {
                    info!("receiver dropped, exiting ws client");
                    return Ok(());
                }
                Err(e) => {
                    warn!("ws connection error: {e}");
                }
            }

            if token.is_cancelled() {
                info!("ws client shutdown requested");
                return Ok(());
            }

            self.send_lifecycle(
                WsLifecycleKind::ReconnectStart,
                &session_id,
                Some(format!("attempt={attempt}")),
            )
            .await?;
            pb_metrics::record_reconnection();

            let backoff = self.backoff_ms(attempt);
            info!(backoff_ms = backoff, attempt, "reconnecting");
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(backoff)) => {}
                _ = token.cancelled() => {
                    info!("ws client shutdown during backoff");
                    return Ok(());
                }
            }
            attempt = attempt.saturating_add(1);
        }
    }

    async fn connect_and_listen_with_token(
        &self,
        token: &CancellationToken,
        session_id: &str,
    ) -> Result<(), FeedError> {
        let connector = Connector::NativeTls(self.tls_connector.clone());
        let (ws_stream, _) =
            connect_async_tls_with_config(&self.config.ws_url, None, true, Some(connector)).await?;
        let (mut sink, mut stream) = ws_stream.split();
        info!(url = %self.config.ws_url, session_id, "ws connected");

        self.send_lifecycle(WsLifecycleKind::ReconnectSuccess, session_id, None)
            .await?;

        let sub = serde_json::json!({
            "assets_ids": &self.asset_ids,
            "type": "market",
        });
        sink.send(Message::Text(sub.to_string().into())).await?;
        debug!(assets = ?self.asset_ids, "subscribed");

        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(
            self.config.ping_interval_secs,
        ));

        loop {
            tokio::select! {
                _ = token.cancelled() => {
                    info!("ws shutdown requested, sending close frame");
                    let _ = sink.send(Message::Close(None)).await;
                    return Ok(());
                }
                _ = ping_interval.tick() => {
                    sink.send(Message::Ping(vec![].into())).await?;
                    debug!("sent ping");
                }
                msg = stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            let raw = WsRawMessage {
                                text: text.to_string(),
                                recv_timestamp_us: now_us(),
                            };
                            if self.tx.send(FeedMessage::Raw(raw)).await.is_err() {
                                error!("receiver dropped, stopping ws client");
                                return Err(FeedError::ChannelSend);
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

    async fn send_lifecycle(
        &self,
        kind: WsLifecycleKind,
        session_id: &str,
        details: Option<String>,
    ) -> Result<(), FeedError> {
        let event = WsLifecycleEvent {
            kind,
            recv_timestamp_us: now_us(),
            session_id: session_id.to_string(),
            details,
        };
        self.tx
            .send(FeedMessage::Lifecycle(event))
            .await
            .map_err(|_| FeedError::ChannelSend)
    }

    fn backoff_ms(&self, attempt: u32) -> u64 {
        let exp = self
            .config
            .reconnect_base_delay_ms
            .saturating_mul(1u64 << attempt.min(15));
        let jitter = fastrand_jitter(exp / 4);
        exp.saturating_add(jitter)
            .min(self.config.reconnect_max_delay_ms)
    }
}

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
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
