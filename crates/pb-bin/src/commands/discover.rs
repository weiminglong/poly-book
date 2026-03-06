use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use config::Config;

/// Compute the slug for the current live BTC 5-minute market.
/// These markets use slug `btc-updown-5m-{ts}` where `ts` is the current
/// unix timestamp floored to the nearest 300-second boundary.
fn current_btc_5m_slug() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before epoch")
        .as_secs();
    let bucket = now - (now % 300);
    format!("btc-updown-5m-{bucket}")
}

fn print_events(events: &[pb_types::wire::GammaEvent]) -> u64 {
    let mut found = 0u64;
    for event in events {
        let title = event.title.as_deref().unwrap_or("");
        println!("Event: {title}");
        if let Some(markets) = &event.markets {
            for market in markets {
                let question = market.question.as_deref().unwrap_or("N/A");
                let token_ids = market.clob_token_ids.as_deref().unwrap_or("N/A");
                let active = market.active.unwrap_or(false);
                println!("  Market: {question}");
                println!("    Token IDs: {token_ids}");
                println!("    Active: {active}");
            }
        }
        found += 1;
    }
    found
}

pub async fn run(settings: Config, filter: Option<String>, limit: u64) -> Result<()> {
    tracing::info!(limit, "Discovering active BTC 5-minute markets...");

    let rate_requests = settings.get_int("feed.rate_limit_requests").unwrap_or(1500) as u32;
    let rate_window = settings
        .get_int("feed.rate_limit_window_secs")
        .unwrap_or(10) as u32;
    let rate_limiter = pb_feed::RateLimiter::with_window(rate_requests, rate_window);

    let rest_config = pb_feed::RestConfig {
        clob_base_url: settings
            .get_string("feed.rest_url")
            .unwrap_or_else(|_| pb_feed::RestConfig::default().clob_base_url),
        gamma_base_url: settings
            .get_string("feed.gamma_url")
            .unwrap_or_else(|_| pb_feed::RestConfig::default().gamma_base_url),
    };

    let rest = pb_feed::RestClient::new(rate_limiter).with_config(rest_config);
    let keyword = filter.as_deref().unwrap_or("btc").to_lowercase();

    // For BTC queries, first try to find the live 5-minute market by slug
    if keyword == "btc" {
        let slug = current_btc_5m_slug();
        tracing::info!(slug, "looking up live BTC 5-minute market");
        let events = rest.discover_by_slug(&slug).await?;
        if !events.is_empty() {
            println!("--- Live BTC 5-minute market ---");
            print_events(&events);
            println!();
        } else {
            tracing::warn!(slug, "no live BTC 5-minute market found for current window");
        }
    }

    // Then paginate through general crypto events with keyword filter
    let page_size: u64 = 100;
    let mut offset: u64 = 0;
    let mut found = 0u64;

    loop {
        if offset >= limit {
            break;
        }

        let batch_size = page_size.min(limit - offset);
        let events = rest.discover_markets(offset, batch_size).await?;
        let page_count = events.len() as u64;

        tracing::debug!(offset, page_count, "fetched event page");

        for event in &events {
            let title = event.title.as_deref().unwrap_or("");
            let title_matches = title.to_lowercase().contains(&keyword);

            let market_matches = event.markets.as_ref().is_some_and(|markets| {
                markets.iter().any(|m| {
                    m.question
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&keyword)
                })
            });

            if !title_matches && !market_matches {
                continue;
            }

            println!("Event: {title}");
            if let Some(markets) = &event.markets {
                for market in markets {
                    let question = market.question.as_deref().unwrap_or("N/A");
                    let token_ids = market.clob_token_ids.as_deref().unwrap_or("N/A");
                    let active = market.active.unwrap_or(false);
                    println!("  Market: {question}");
                    println!("    Token IDs: {token_ids}");
                    println!("    Active: {active}");
                }
            }
            found += 1;
        }

        offset += page_count;

        if page_count < batch_size {
            break;
        }
    }

    tracing::info!(found, offset, "discovery complete");
    Ok(())
}
