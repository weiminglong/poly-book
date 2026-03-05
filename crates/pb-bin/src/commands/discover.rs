use anyhow::Result;
use config::Config;

pub async fn run(_settings: Config, filter: Option<String>) -> Result<()> {
    tracing::info!("Discovering active BTC 5-minute markets...");

    let rate_limiter = pb_feed::RateLimiter::new();
    let rest = pb_feed::RestClient::new(rate_limiter);
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
