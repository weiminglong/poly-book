//! Shared live-feed runtime: one or more market subscriptions, each a
//! WebSocket client, a dispatcher, and a REST resnapshot self-heal worker,
//! driven either by a fixed token list or by the rotating BTC 5-minute market.
//! Used by `ingest` (persisting) and `serve-api` (view-only) so both follow the
//! same rotation rules.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{bail, Result};
use config::Config;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::market_discovery::{
    current_unix_secs, discover_with_retry, populate_registry, DiscoverOutcome,
};
use super::pipeline;

/// Which markets a live process subscribes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedPlan {
    /// A fixed set of token IDs for the life of the process.
    Fixed(Vec<String>),
    /// Follow the live BTC 5-minute market, rotating on each 5-minute bucket.
    AutoRotate,
}

impl FeedPlan {
    /// Build a plan from the `--tokens` / `--auto-rotate` CLI flags.
    pub fn from_flags(tokens: Option<String>, auto_rotate: bool) -> Result<Self> {
        match (tokens, auto_rotate) {
            (Some(raw), false) => {
                let token_ids: Vec<String> = raw
                    .split(',')
                    .map(str::trim)
                    .filter(|token| !token.is_empty())
                    .map(ToOwned::to_owned)
                    .collect();
                if token_ids.is_empty() {
                    bail!("--tokens must contain at least one token ID");
                }
                Ok(Self::Fixed(token_ids))
            }
            (None, true) => Ok(Self::AutoRotate),
            (Some(_), true) => bail!("choose either --tokens or --auto-rotate, not both"),
            (None, false) => {
                bail!("either --tokens or --auto-rotate is required (use 'discover' to find token IDs)")
            }
        }
    }
}

/// Length of one BTC 5-minute market bucket.
const BUCKET_SECS: u64 = 300;
/// Subscribe to the next market this long before its bucket starts so the first
/// seconds of the new market are captured.
const PRE_ROTATION_SECS: u64 = 10;

/// Callback invoked whenever the active asset set changes.
pub type OnActive = Box<dyn FnMut(Vec<String>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// A single market-subscription generation: its cancellation token and task
/// handles, kept together so it can be cancelled and joined as a unit.
struct Generation {
    token: CancellationToken,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl Generation {
    /// Spawn the WebSocket client, dispatcher, and resnapshot worker for one
    /// set of tokens. Records flow into `event_tx`.
    fn spawn(
        settings: &Config,
        token_ids: Vec<String>,
        event_tx: mpsc::Sender<pb_types::PersistedRecord>,
        parent: &CancellationToken,
    ) -> Result<Self> {
        let token = parent.child_token();
        let (raw_tx, raw_rx) = mpsc::channel::<pb_feed::FeedMessage>(2_048);
        // Resnapshot requests the dispatcher raises on a book divergence. Small
        // and non-blocking: a backlog means a resnapshot is already pending, so
        // dropped requests are harmless.
        let (resnapshot_tx, resnapshot_rx) = mpsc::channel::<std::sync::Arc<str>>(64);

        let ws_client = pb_feed::WsClient::new(token_ids, raw_tx.clone())?
            .with_config(pipeline::ws_config_from_settings(settings));
        let ws_token = token.child_token();
        let ws_handle = tokio::spawn(async move {
            if let Err(e) = ws_client.run_with_token(ws_token).await {
                tracing::error!(error = %e, "websocket client failed");
            }
        });

        let mut dispatcher =
            pb_feed::Dispatcher::new(raw_rx, event_tx).with_resnapshot_tx(resnapshot_tx);
        let dispatcher_token = token.child_token();
        let dispatcher_handle = tokio::spawn(async move {
            if let Err(e) = dispatcher.run_with_token(dispatcher_token).await {
                tracing::error!(error = %e, "dispatcher failed");
            }
        });

        // Self-healing: on a detected book divergence, fetch a fresh REST
        // snapshot and re-inject it through the dispatcher's normal snapshot path.
        let rest = pb_feed::RestClient::new(pipeline::rest_rate_limiter(settings))?
            .with_config(pipeline::rest_config_from_settings(settings));
        let resnapshot_token = token.child_token();
        let resnapshot_handle = tokio::spawn(async move {
            pb_feed::run_resnapshot_worker(rest, raw_tx, resnapshot_rx, resnapshot_token).await;
        });

        Ok(Self {
            token,
            handles: vec![ws_handle, dispatcher_handle, resnapshot_handle],
        })
    }

    async fn stop(self, label: &str) {
        self.token.cancel();
        pipeline::shutdown_handles(self.handles, label).await;
    }
}

/// Drive market subscriptions until `shutdown` fires. `on_active` is called with
/// the full active token set whenever it changes (once for a fixed plan, on
/// every rotation for auto-rotate, and with an empty set on shutdown).
///
/// Auto-rotate subscribes to the next market `PRE_ROTATION_SECS` before its
/// bucket starts and keeps the outgoing market alive until its bucket boundary,
/// so neither the first nor the last seconds of a market are dropped.
pub async fn run_feed(
    settings: &Config,
    plan: FeedPlan,
    event_tx: mpsc::Sender<pb_types::PersistedRecord>,
    shutdown: CancellationToken,
    slug_registry: pb_types::SlugRegistry,
    mut on_active: OnActive,
) -> Result<()> {
    match plan {
        FeedPlan::Fixed(token_ids) => {
            tracing::info!(tokens = ?token_ids, "subscribing to fixed token set");
            let generation = Generation::spawn(settings, token_ids.clone(), event_tx, &shutdown)?;
            on_active(token_ids).await;
            shutdown.cancelled().await;
            generation.stop("feed task").await;
        }
        FeedPlan::AutoRotate => {
            tracing::info!("following the rotating BTC 5-minute market");
            run_rotation(settings, event_tx, &shutdown, slug_registry, &mut on_active).await?;
        }
    }
    on_active(Vec::new()).await;
    Ok(())
}

async fn run_rotation(
    settings: &Config,
    event_tx: mpsc::Sender<pb_types::PersistedRecord>,
    shutdown: &CancellationToken,
    slug_registry: pb_types::SlugRegistry,
    on_active: &mut OnActive,
) -> Result<()> {
    let rest = pb_feed::RestClient::new(pipeline::rest_rate_limiter(settings))?
        .with_config(pipeline::rest_config_from_settings(settings));

    let mut front: Option<Generation> = None;
    let mut active_bucket: Option<u64> = None;

    loop {
        let now_secs = current_unix_secs();
        let current_bucket = now_secs - (now_secs % BUCKET_SECS);
        let target_bucket = if active_bucket.is_none() {
            current_bucket
        } else {
            current_bucket + BUCKET_SECS
        };

        // Sleep until the pre-rotation window of the next bucket. On the very
        // first pass there is no active market, so subscribe immediately.
        if active_bucket.is_some() {
            let wake_at = (target_bucket - PRE_ROTATION_SECS).max(now_secs);
            let sleep_secs = wake_at.saturating_sub(current_unix_secs());
            if sleep_secs > 0 {
                tracing::debug!(sleep_secs, target_bucket, "waiting for pre-rotation window");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(sleep_secs)) => {}
                    _ = shutdown.cancelled() => break,
                }
            }
        }
        if shutdown.is_cancelled() {
            break;
        }

        let target_slug = format!("btc-updown-5m-{target_bucket}");
        let discovery = match discover_with_retry(&rest, &target_slug, shutdown).await {
            DiscoverOutcome::Found(result) => result,
            DiscoverOutcome::Shutdown => break,
            DiscoverOutcome::Failed => {
                // Skip this bucket; the loop targets the following one.
                active_bucket = Some(target_bucket);
                continue;
            }
        };
        populate_registry(&slug_registry, &discovery);
        let token_ids = discovery.token_ids;

        // Subscribe to the new market while the old one keeps running.
        let next = Generation::spawn(settings, token_ids.clone(), event_tx.clone(), shutdown)?;

        // Cut the previous market over only once its bucket has actually ended.
        if let Some(old) = front.take() {
            let wait = target_bucket.saturating_sub(current_unix_secs());
            if wait > 0 {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
                    _ = shutdown.cancelled() => {}
                }
            }
            old.stop("rotated-out feed task").await;
        }

        front = Some(next);
        active_bucket = Some(target_bucket);
        on_active(token_ids.clone()).await;
        pb_metrics::record_rotation();
        tracing::info!(slug = %target_slug, tokens = ?token_ids, "rotated to new market");

        if shutdown.is_cancelled() {
            break;
        }
    }

    if let Some(old) = front.take() {
        old.stop("feed task").await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_plan_from_flags() {
        assert_eq!(
            FeedPlan::from_flags(Some("a, b,,c".to_string()), false).unwrap(),
            FeedPlan::Fixed(vec!["a".into(), "b".into(), "c".into()])
        );
        assert_eq!(
            FeedPlan::from_flags(None, true).unwrap(),
            FeedPlan::AutoRotate
        );
        assert!(FeedPlan::from_flags(Some(" , ".to_string()), false).is_err());
        assert!(FeedPlan::from_flags(Some("a".to_string()), true).is_err());
        assert!(FeedPlan::from_flags(None, false).is_err());
    }
}
