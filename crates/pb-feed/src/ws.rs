use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async_tls_with_config, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::error::FeedError;

const DEFAULT_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
const DEFAULT_PING_INTERVAL_SECS: u64 = 10;
const DEFAULT_BASE_BACKOFF_MS: u64 = 100;
const DEFAULT_MAX_BACKOFF_MS: u64 = 30_000;
/// A session that stayed connected at least this long is considered stable, and
/// resets the reconnect backoff so the next disconnect retries quickly instead
/// of inheriting an ever-growing delay (audit finding A.106).
const STABLE_SESSION_MS: u64 = 30_000;

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
}

impl WsClient {
    pub fn new(asset_ids: Vec<String>, tx: mpsc::Sender<FeedMessage>) -> Result<Self, FeedError> {
        Ok(Self {
            asset_ids,
            tx,
            config: WsConfig::default(),
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
            let session_started = Instant::now();
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
            // A session that stayed up long enough is "stable": reset the
            // backoff so a later disconnect reconnects promptly instead of
            // inheriting a 30s delay accumulated over the process lifetime.
            let session_was_stable =
                session_started.elapsed() >= Duration::from_millis(STABLE_SESSION_MS);

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
                _ = tokio::time::sleep(Duration::from_millis(backoff)) => {}
                _ = token.cancelled() => {
                    info!("ws client shutdown during backoff");
                    return Ok(());
                }
            }
            attempt = if session_was_stable {
                0
            } else {
                attempt.saturating_add(1)
            };
        }
    }

    async fn connect_and_listen_with_token(
        &self,
        token: &CancellationToken,
        session_id: &str,
    ) -> Result<(), FeedError> {
        // Pass no explicit connector: with the `rustls-tls-webpki-roots` feature
        // tokio-tungstenite builds a default rustls connector using the bundled
        // Mozilla root store, so we depend only on rustls (no openssl/native-tls
        // C dependency — audit finding P2-BUILD-3).
        let (ws_stream, _) =
            connect_async_tls_with_config(&self.config.ws_url, None, true, None).await?;
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

        let ping_secs = self.config.ping_interval_secs.max(1);
        let mut ping_interval = tokio::time::interval(Duration::from_secs(ping_secs));
        // Liveness watchdog: if no frame (data OR pong) arrives within this
        // window, the TCP connection is likely half-open and would otherwise
        // stall the feed silently for many minutes. Force a reconnect instead
        // (audit finding A.107).
        let read_idle_timeout = Duration::from_secs(ping_secs.saturating_mul(3));
        let mut watchdog = tokio::time::interval(Duration::from_secs(ping_secs));
        let mut last_activity = Instant::now();

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
                _ = watchdog.tick() => {
                    let idle = last_activity.elapsed();
                    if idle >= read_idle_timeout {
                        warn!(
                            idle_secs = idle.as_secs(),
                            timeout_secs = read_idle_timeout.as_secs(),
                            "ws read-idle timeout; connection appears half-open, forcing reconnect"
                        );
                        return Err(FeedError::ConnectionStalled);
                    }
                }
                msg = stream.next() => {
                    // Any received frame proves the connection is alive.
                    last_activity = Instant::now();
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
        backoff_ms(&self.config, attempt)
    }
}

/// Exponential backoff with jitter. The exponential term is capped to leave
/// headroom below `reconnect_max_delay_ms` so that jitter still varies the delay
/// at the cap — otherwise every client reconnects at exactly the max, defeating
/// jitter precisely when a thundering herd matters most (audit finding A.150).
fn backoff_ms(config: &WsConfig, attempt: u32) -> u64 {
    let max = config.reconnect_max_delay_ms;
    let exp = config
        .reconnect_base_delay_ms
        .saturating_mul(1u64 << attempt.min(15));
    // Cap the exponential part to 3/4 of max, reserving the top quarter for jitter.
    let ceiling = max.saturating_sub(max / 4);
    let capped = exp.min(ceiling);
    let jitter = fastrand_jitter(capped / 4 + 1);
    capped.saturating_add(jitter).min(max)
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
    // Use a simple hash of the current nanosecond timestamp plus thread id
    // for lightweight jitter. This avoids pulling in a full PRNG crate while
    // providing better distribution than raw subsec_nanos alone.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    // Multiplicative hash (Knuth's) to spread adjacent nanosecond values.
    let hash = nanos.wrapping_mul(2654435761);
    hash % max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_attempt_zero_equals_base() {
        let config = WsConfig {
            reconnect_base_delay_ms: 100,
            reconnect_max_delay_ms: 30_000,
            ..WsConfig::default()
        };
        let result = backoff_ms(&config, 0);
        // base=100, jitter up to 25 -> result in [100, 125)
        assert!(result >= 100);
        assert!(result < 125);
    }

    #[test]
    fn backoff_grows_exponentially() {
        let config = WsConfig {
            reconnect_base_delay_ms: 100,
            reconnect_max_delay_ms: 1_000_000,
            ..WsConfig::default()
        };

        // Without jitter, attempt 0=100, 1=200, 2=400, 3=800.
        // With jitter up to exp/4, we verify monotonic lower bounds.
        let b0 = config.reconnect_base_delay_ms; // 100
        let b1 = b0 * 2; // 200
        let b2 = b0 * 4; // 400
        let b3 = b0 * 8; // 800

        let r0 = backoff_ms(&config, 0);
        let r1 = backoff_ms(&config, 1);
        let r2 = backoff_ms(&config, 2);
        let r3 = backoff_ms(&config, 3);

        assert!(r0 >= b0, "attempt 0: {r0} < {b0}");
        assert!(r1 >= b1, "attempt 1: {r1} < {b1}");
        assert!(r2 >= b2, "attempt 2: {r2} < {b2}");
        assert!(r3 >= b3, "attempt 3: {r3} < {b3}");
    }

    #[test]
    fn backoff_capped_at_max() {
        let config = WsConfig {
            reconnect_base_delay_ms: 100,
            reconnect_max_delay_ms: 500,
            ..WsConfig::default()
        };

        // At attempt 10, base * 2^10 = 102,400 >> max=500. The result must stay
        // at/below max but still be jittered (not collapsed to exactly max), so
        // it falls in the reserved top-quarter band [0.75*max, max] (A.150).
        let result = backoff_ms(&config, 10);
        assert!(result <= 500, "backoff exceeded max: {result}");
        assert!(result >= 375, "backoff not in jitter band: {result}");
    }

    #[test]
    fn backoff_high_attempt_does_not_overflow() {
        let config = WsConfig {
            reconnect_base_delay_ms: 100,
            reconnect_max_delay_ms: 30_000,
            ..WsConfig::default()
        };

        // Attempt 100 would overflow without the .min(15) guard.
        let result = backoff_ms(&config, 100);
        assert!(result <= 30_000);
    }

    #[test]
    fn backoff_u32_max_attempt_does_not_panic() {
        let config = WsConfig {
            reconnect_base_delay_ms: 100,
            reconnect_max_delay_ms: 30_000,
            ..WsConfig::default()
        };

        let result = backoff_ms(&config, u32::MAX);
        assert!(result <= 30_000);
    }

    #[test]
    fn jitter_returns_zero_when_max_is_zero() {
        assert_eq!(fastrand_jitter(0), 0);
    }

    #[test]
    fn jitter_stays_within_bounds() {
        // Run multiple times to exercise the hash path.
        for _ in 0..100 {
            let j = fastrand_jitter(1000);
            assert!(j < 1000, "jitter {j} >= 1000");
        }
    }

    #[test]
    fn jitter_max_one_returns_zero() {
        // hash % 1 == 0 always.
        assert_eq!(fastrand_jitter(1), 0);
    }

    #[test]
    fn ws_config_default_values() {
        let config = WsConfig::default();
        assert_eq!(config.reconnect_base_delay_ms, 100);
        assert_eq!(config.reconnect_max_delay_ms, 30_000);
        assert_eq!(config.ping_interval_secs, 10);
        assert!(config.ws_url.starts_with("wss://"));
    }
}
