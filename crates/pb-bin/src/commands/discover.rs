use anyhow::Result;
use config::Config;

pub async fn run(settings: Config, filter: Option<String>) -> Result<()> {
    tracing::info!("Discovering active BTC 5-minute markets...");

    let rate_requests = settings
        .get_int("feed.rate_limit_requests")
        .unwrap_or(1500) as u32;
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
    let events = rest.discover_markets(0, 100).await?;

    let keyword = filter.as_deref().unwrap_or("btc").to_lowercase();

    let mut found = 0;
    for event in &events {
        let title = event.title.as_deref().unwrap_or("");
        if !title.to_lowercase().contains(&keyword) {
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

    tracing::info!(found, "discovery complete");
    Ok(())
}
