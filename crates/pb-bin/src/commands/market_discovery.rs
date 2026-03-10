use std::time::Duration;

use pb_types::{AssetId, SlugRegistry};
use tokio_util::sync::CancellationToken;

pub enum DiscoverOutcome {
    Found(DiscoveryResult),
    Shutdown,
    Failed,
}

/// Result from market discovery, including token IDs and slug mappings.
pub struct DiscoveryResult {
    pub token_ids: Vec<String>,
    pub slug_mappings: Vec<SlugMapping>,
}

/// A slug-to-token-IDs mapping extracted from Gamma API market data.
pub struct SlugMapping {
    pub slug: String,
    pub token_ids: Vec<String>,
    pub label: Option<String>,
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

/// Extract token IDs and slug mappings from Gamma API events.
pub fn extract_discovery(events: &[pb_types::wire::GammaEvent]) -> DiscoveryResult {
    let mut token_ids = Vec::new();
    let mut slug_mappings = Vec::new();

    for event in events {
        let markets = match &event.markets {
            Some(m) => m,
            None => continue,
        };
        // Use event-level slug as fallback for market-level slug
        let event_slug = event.slug.as_deref();

        for market in markets {
            let raw = match &market.clob_token_ids {
                Some(s) => s,
                None => continue,
            };
            let market_token_ids: Vec<String> =
                if let Ok(parsed) = serde_json::from_str::<Vec<String>>(raw) {
                    parsed
                } else {
                    raw.split(',').map(|s| s.trim().to_string()).collect()
                };

            token_ids.extend(market_token_ids.clone());

            // Build slug mapping from market or event slug
            let slug = market.slug.as_deref().or(event_slug);
            if let Some(slug) = slug {
                slug_mappings.push(SlugMapping {
                    slug: slug.to_string(),
                    token_ids: market_token_ids,
                    label: market.question.clone(),
                });
            }
        }
    }

    DiscoveryResult {
        token_ids,
        slug_mappings,
    }
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
                let result = extract_discovery(&events);
                if !result.token_ids.is_empty() {
                    return DiscoverOutcome::Found(result);
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

/// Populate a `SlugRegistry` from discovery results.
pub fn populate_registry(registry: &SlugRegistry, result: &DiscoveryResult) {
    for mapping in &result.slug_mappings {
        registry.register_market(&mapping.slug, &mapping.token_ids);
        // Register labels for each token
        if let Some(label) = &mapping.label {
            for token_id in &mapping.token_ids {
                registry.register_label(&AssetId::new(token_id.as_str()), label);
            }
        }
    }
}
