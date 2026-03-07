use std::time::Duration;

use tokio_util::sync::CancellationToken;

pub enum DiscoverOutcome {
    Found(Vec<String>),
    Shutdown,
    Failed,
}

pub fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs()
}

pub fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_micros() as u64
}

pub fn extract_token_ids(events: &[pb_types::wire::GammaEvent]) -> Vec<String> {
    let mut ids = Vec::new();
    for event in events {
        let markets = match &event.markets {
            Some(m) => m,
            None => continue,
        };
        for market in markets {
            let raw = match &market.clob_token_ids {
                Some(s) => s,
                None => continue,
            };
            if let Ok(parsed) = serde_json::from_str::<Vec<String>>(raw) {
                ids.extend(parsed);
            } else {
                ids.extend(raw.split(',').map(|s| s.trim().to_string()));
            }
        }
    }
    ids
}

pub async fn discover_with_retry(
    rest: &pb_feed::RestClient,
    slug: &str,
    shutdown: &CancellationToken,
) -> DiscoverOutcome {
    let mut delay_ms = 2000u64;
    for attempt in 1..=5 {
        if shutdown.is_cancelled() {
            return DiscoverOutcome::Shutdown;
        }
        match rest.discover_by_slug(slug).await {
            Ok(events) => {
                let ids = extract_token_ids(&events);
                if !ids.is_empty() {
                    return DiscoverOutcome::Found(ids);
                }
                tracing::warn!(slug, attempt, "slug returned no token IDs, retrying");
            }
            Err(e) => {
                tracing::warn!(slug, attempt, error = %e, "discovery request failed, retrying");
            }
        }
        pb_metrics::record_discovery_failure();
        if attempt < 5 {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                _ = shutdown.cancelled() => return DiscoverOutcome::Shutdown,
            }
            delay_ms = (delay_ms * 2).min(8_000);
        }
    }
    tracing::error!(slug, "discovery failed after 5 attempts, skipping window");
    DiscoverOutcome::Failed
}
