use reqwest::Client;
use tracing::debug;

use crate::error::FeedError;
use crate::rate_limiter::RateLimiter;
use pb_types::wire::{GammaEvent, RestBookResponse};

const CLOB_BASE: &str = "https://clob.polymarket.com";
const GAMMA_BASE: &str = "https://gamma-api.polymarket.com";

pub struct RestClient {
    client: Client,
    rate_limiter: RateLimiter,
}

impl RestClient {
    pub fn new(rate_limiter: RateLimiter) -> Self {
        Self {
            client: Client::new(),
            rate_limiter,
        }
    }

    pub async fn fetch_book(&self, token_id: &str) -> Result<RestBookResponse, FeedError> {
        self.rate_limiter.acquire().await;
        let url = format!("{CLOB_BASE}/book?token_id={token_id}");
        debug!(url, "fetching book");
        let resp = self.client.get(&url).send().await?.json().await?;
        Ok(resp)
    }

    pub async fn discover_markets(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<GammaEvent>, FeedError> {
        self.rate_limiter.acquire().await;
        let url = format!(
            "{GAMMA_BASE}/events?active=true&closed=false&tag=crypto&offset={offset}&limit={limit}"
        );
        debug!(url, "discovering markets");
        let resp = self.client.get(&url).send().await?.json().await?;
        Ok(resp)
    }
}
